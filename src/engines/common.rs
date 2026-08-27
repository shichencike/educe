//! 各适配器共享的解析 / URL 助手。

use std::collections::HashSet;

use scraper::{Html, Selector};
use url::Url;

use crate::models::SearchResult;

/// 错误链渲染：外层上下文 + 完整原因链，用 ` ← ` 连成单行。
/// 网络类错误（如"请求失败(baidu): url"）只显示最外层时根因被吞掉，
/// 串上整条链便于前端展示与定位（DNS 超时 / 连接超时 / TLS 握手失败等）。
///
/// anyhow::Error 不实现 `std::error::Error`（自带私有 StdError），无法直接
/// trait-object 化，故用具体类型宏实现统一抽象，避免 blanket impl 与上游
/// trait 的未来兼容冲突。
pub fn error_detail(err: &impl ErrorChain) -> String {
    err.render_chain()
}

/// 错误链遍历的统一抽象。
pub trait ErrorChain {
    fn render_chain(&self) -> String;
}

/// 通用 std 错误链遍历（`source()` 逐层下钻）。
fn render_std_chain(err: &dyn std::error::Error) -> String {
    let mut parts = Vec::new();
    let mut cur: Option<&dyn std::error::Error> = Some(err);
    while let Some(e) = cur {
        parts.push(e.to_string());
        cur = e.source();
    }
    parts.join(" ← ")
}

impl ErrorChain for anyhow::Error {
    fn render_chain(&self) -> String {
        self.chain()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(" ← ")
    }
}

macro_rules! impl_error_chain_std {
    ($($t:ty),* $(,)?) => {
        $(
            impl ErrorChain for $t {
                fn render_chain(&self) -> String {
                    render_std_chain(self)
                }
            }
        )*
    };
}

impl_error_chain_std!(reqwest::Error, std::io::Error);

impl<'a> ErrorChain for scraper::error::SelectorErrorKind<'a> {
    fn render_chain(&self) -> String {
        render_std_chain(self)
    }
}

/// 查询串编码：空格转 %20（适用于大多数引擎）。
pub fn encode_query_pct(q: &str) -> String {
    use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
    // NON_ALPHANUMERIC 会把空格也编码为 %20，安全且通用
    utf8_percent_encode(q, NON_ALPHANUMERIC).to_string()
}

/// 折叠空白：去首尾、连续空白压缩为单空格。
pub fn clean_text(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 截断到 max_chars 字符（按 char 边界）。
pub fn clip(s: &str, max_chars: usize) -> String {
    let trimmed = clean_text(s);
    if trimmed.chars().count() <= max_chars {
        return trimmed;
    }
    let mut out: String = trimmed.chars().take(max_chars).collect();
    out.push('…');
    out
}

/// 把页面内相对链接解析为绝对 URL。
pub fn absolute_url(base: &str, href: &str) -> Option<String> {
    let base = Url::parse(base).ok()?;
    base.join(href).ok().map(|u| u.to_string())
}

/// 还原常见搜索引擎跳转链（百度 link?url=、搜狗 link?url=、360 link?url=、
/// Google /url?q=、Bing /ck/a?url=、知乎 link?target=、CSDN link?target=、
/// 简书 go-wild?url= 等），取出真实目标 URL；非跳转链原样返回。
pub fn unredirect_url(raw: &str) -> String {
    let Ok(u) = Url::parse(raw) else {
        return raw.to_string();
    };
    let host = u.host_str().unwrap_or("").to_lowercase();
    let pairs: Vec<(String, String)> = u
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();

    // 目标参数的候选键（按引擎习惯）
    let target_key = |keys: &[&str]| -> Option<String> {
        pairs.iter().find_map(|(k, v)| {
            if keys.contains(&k.as_str()) && !v.is_empty() {
                Some(v.clone())
            } else {
                None
            }
        })
    };

    let decoded: Option<String> =
        if host.ends_with("baidu.com") || host.ends_with("sogou.com") || host.ends_with("so.com") {
            // baidu/sogou/360 的 link?url= 是 base64url 编码的 URL
            target_key(&["url"]).and_then(|v| decode_base64_url(&v))
        } else if host == "www.google.com"
            || host == "www.google.com.hk"
            || host.ends_with("google.com")
            || host.ends_with("google.co.jp")
        {
            target_key(&["url", "q"]).map(|v| percent_decode(&v).unwrap_or(v))
        } else if host.ends_with("bing.com") {
            target_key(&["url"]).map(|v| percent_decode(&v).unwrap_or(v))
        } else if host.ends_with("zhihu.com") || host.ends_with("csdn.net") {
            target_key(&["target"]).map(|v| percent_decode(&v).unwrap_or(v))
        } else if host.ends_with("jianshu.com") {
            target_key(&["url"]).map(|v| percent_decode(&v).unwrap_or(v))
        } else if host.ends_with("duckduckgo.com") {
            target_key(&["uddg"]).map(|v| percent_decode(&v).unwrap_or(v))
        } else {
            None
        };

    match decoded {
        Some(t) if t.starts_with("http://") || t.starts_with("https://") => t,
        _ => raw.to_string(),
    }
}

/// 解码 base64url（百度/搜狗/360 跳转链的 url 参数是 base64url，无填充）。
fn decode_base64_url(s: &str) -> Option<String> {
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.is_empty() {
        return None;
    }
    use base64::Engine as _;
    // 补 padding 后按 URL_SAFE 解码；失败再按标准 base64 试一次
    let mut b64 = cleaned.replace('-', "+").replace('_', "/");
    while !b64.len().is_multiple_of(4) {
        b64.push('=');
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&b64)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(&b64))
        .ok()?;
    String::from_utf8(bytes).ok()
}

fn percent_decode(s: &str) -> Option<String> {
    use percent_encoding::percent_decode_str;
    percent_decode_str(s)
        .decode_utf8()
        .ok()
        .map(|c| c.into_owned())
}

/// 去除 URL 中的跟踪参数与片段，用于跨源去重。
pub fn normalize_url(raw: &str) -> String {
    let Ok(mut u) = Url::parse(raw) else {
        return raw.to_string();
    };
    let drop: &[&str] = &[
        "utm_source",
        "utm_medium",
        "utm_campaign",
        "utm_term",
        "utm_content",
        "fbclid",
        "gclid",
        "yclid",
        "ref",
        "spm",
        "share_source",
        "share_medium",
    ];
    // 快速路径：无片段、无尾斜杠、无跟踪参数时直接返回原始串，避免重建
    let path = u.path();
    let has_tracking = u.query_pairs().any(|(k, _)| drop.contains(&k.as_ref()));
    if !has_tracking && u.fragment().is_none() && !(path.len() > 1 && path.ends_with('/')) {
        return raw.to_string();
    }
    // 去掉片段
    u.set_fragment(None);
    // 去掉常见跟踪参数
    let pairs: Vec<(String, String)> = {
        let mut kept = Vec::new();
        for (k, v) in u.query_pairs() {
            if !drop.contains(&k.as_ref()) {
                kept.push((k.into_owned(), v.into_owned()));
            }
        }
        kept
    };
    u.set_query(None);
    if !pairs.is_empty() {
        u.query_pairs_mut()
            .extend_pairs(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }
    // 去掉末尾斜杠（保留根路径 "/"）
    let path = u.path().to_string();
    if path.len() > 1 && path.ends_with('/') {
        u.set_path(&path[..path.len() - 1]);
    }
    u.to_string()
}

/// 从 HTML 片段中抽取纯文本（去标签）。简单实现，供个别引擎摘要补全用。
pub fn strip_html(s: &str) -> String {
    // 去掉标签，保留可见字符；足够用于摘要
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    clean_text(&out)
}

/// 通用兜底提取器：从任意（JS 渲染后的）页面里捞取结果。
/// 规则：收集站外 `a[href^=http]` 链接，链接文字够长且不是导航噪音的当作结果。
/// 用于 JS 渲染源的专用选择器失效时，保证"有结果可用"（兼容性兜底）。
pub fn generic_extract(html: &str, source: &str, max: usize) -> Vec<SearchResult> {
    let doc = Html::parse_document(html);
    let Ok(a_sel) = Selector::parse("a[href^='http']") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for a in doc.select(&a_sel) {
        if out.len() >= max {
            break;
        }
        let href = a.value().attr("href").unwrap_or("");
        if is_junk_url(href) {
            continue;
        }
        let title = clean_text(&a.text().collect::<String>());
        if title.chars().count() < 6 || is_junk_title(&title) {
            continue;
        }
        let key = normalize_url(href);
        if !seen.insert(key) {
            continue;
        }
        // 摘要：父元素文本去掉标题
        let snippet = a
            .parent()
            .and_then(scraper::ElementRef::wrap)
            .map(|p| clean_text(&p.text().collect::<String>()))
            .unwrap_or_default()
            .replacen(&title, "", 1);
        out.push(SearchResult::new(
            clip(&title, 200),
            href.to_string(),
            clip(&snippet, 300),
            source,
            out.len(),
        ));
    }
    out
}

fn is_junk_url(href: &str) -> bool {
    href.contains("/login")
        || href.contains("passport")
        || href.contains("/register")
        || href.contains("javascript:")
        || href.contains("#")
        || href.ends_with("/search")
        || href.contains("?page=")
        || href.contains("/settings")
}

fn is_junk_title(t: &str) -> bool {
    const JUNK: &[&str] = &[
        "首页",
        "登录",
        "注册",
        "更多",
        "上一页",
        "下一页",
        "打开App",
        "下载",
        "意见反馈",
    ];
    JUNK.contains(&t) || t.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_text_collapses_whitespace() {
        assert_eq!(clean_text("  a\n\t b  c "), "a b c");
    }

    #[test]
    fn clip_truncates_with_ellipsis() {
        let s = clip("一二三四五六七八九十", 5);
        assert_eq!(s.chars().count(), 6); // 5 字符 + 省略号
        assert!(s.ends_with('…'));
        assert_eq!(clip("short", 100), "short");
    }

    #[test]
    fn absolute_url_resolves_relative() {
        assert_eq!(
            absolute_url("https://example.com/a/b", "../c").as_deref(),
            Some("https://example.com/c")
        );
        assert_eq!(
            absolute_url("https://example.com/a/", "/root").as_deref(),
            Some("https://example.com/root")
        );
    }

    #[test]
    fn strip_html_removes_tags() {
        assert_eq!(strip_html("<p>Hello <b>world</b></p>"), "Hello world");
    }

    #[test]
    fn encode_query_pct_encodes_space_and_unicode() {
        assert_eq!(encode_query_pct("a b"), "a%20b");
        assert!(encode_query_pct("中").starts_with('%'));
    }

    #[test]
    fn normalize_url_strips_tracking_and_fragment() {
        assert_eq!(
            normalize_url("https://x.com/p?utm_source=1&id=2#top"),
            "https://x.com/p?id=2"
        );
    }

    #[test]
    fn normalize_url_keeps_clean_url_unchanged() {
        let u = "https://x.com/path?q=1";
        assert_eq!(normalize_url(u), u);
    }

    #[test]
    fn normalize_url_strips_trailing_slash() {
        assert_eq!(normalize_url("https://x.com/a/b/"), "https://x.com/a/b");
        // 根路径保留
        assert_eq!(normalize_url("https://x.com/"), "https://x.com/");
    }

    #[test]
    fn unredirect_url_resolves_google() {
        assert_eq!(
            unredirect_url("https://www.google.com/url?q=https%3A%2F%2Frust-lang.org%2F&sa=U"),
            "https://rust-lang.org/"
        );
    }

    #[test]
    fn unredirect_url_resolves_baidu_base64() {
        // "https://rust-lang.org/" 的 base64url（无填充）
        let target = "aHR0cHM6Ly9ydXN0LWxhbmcub3JnLw";
        assert_eq!(
            unredirect_url(&format!("https://www.baidu.com/link?url={target}")),
            "https://rust-lang.org/"
        );
    }

    #[test]
    fn unredirect_url_keeps_normal_url() {
        let u = "https://example.com/path?q=1";
        assert_eq!(unredirect_url(u), u);
        // 非 http 目标不解码
        assert_eq!(
            unredirect_url("https://www.baidu.com/link?url=javascript%3Aalert(1)"),
            "https://www.baidu.com/link?url=javascript%3Aalert(1)"
        );
    }
}
