//! 各适配器共享的解析 / URL 助手。

use std::collections::HashSet;

use scraper::{Html, Selector};
use url::Url;

use crate::models::SearchResult;

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
}
