//! 百度学术搜索适配器（HTML 解析）。
//! 结果块：`div.result.sc_default_result` -> `h3.t a`（标题/链接），摘要 `.c_abstract` / `.sc_abstract`。
//! 条目附带年份/期刊信息（`.sc_info`），尽量提取为 published 字段。

use async_trait::async_trait;
use scraper::{Html, Selector};

use crate::engines::common::{absolute_url, clean_text, clip, encode_query_pct};
use crate::engines::{Engine, EngineContext, EngineError};
use std::borrow::Cow;

use crate::models::{Category, EngineMeta, SearchResult};

pub const META: EngineMeta = EngineMeta {
    id: Cow::Borrowed("xueshu"),
    name: Cow::Borrowed("百度学术"),
    category: Category::Academic,
    needs_js: false,
};

pub struct Xueshu;

impl Xueshu {
    /// 解析百度学术结果页 HTML（独立方法便于单元测试）。
    fn parse_html(&self, html: &str, max: usize) -> Vec<SearchResult> {
        let doc = Html::parse_document(html);
        // 结果容器：主结果块 + 侧栏/相关推荐做区分（主结果优先）
        let result_sels = [
            Selector::parse("div.result.sc_default_result").ok(),
            Selector::parse("div.result").ok(),
            Selector::parse("div.sc_content").ok(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        let link_sel = Selector::parse("h3.t a, h3 a").ok();
        let snip_sels = [
            Selector::parse(".c_abstract").ok(),
            Selector::parse(".sc_abstract").ok(),
            Selector::parse(".abstract").ok(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        let info_sel = Selector::parse(".sc_info").ok();

        let mut out = Vec::new();
        for el in doc.select(result_sels.first().unwrap_or_else(|| unreachable!())) {
            if out.len() >= max {
                break;
            }
            let Some(a) = link_sel.as_ref().and_then(|s| el.select(s).next()) else {
                continue;
            };
            let title = clean_text(&a.text().collect::<String>());
            let href = a.value().attr("href").unwrap_or("");
            let url = absolute_url("https://xueshu.baidu.com/", href).unwrap_or_default();
            if title.is_empty() || url.is_empty() {
                continue;
            }
            let snippet = snip_sels
                .iter()
                .find_map(|s| el.select(s).next())
                .map(|s| clip(&s.text().collect::<String>(), 300))
                .unwrap_or_default();
            let mut r = SearchResult::new(title, url, snippet, "xueshu", out.len());
            // 尝试从 sc_info 提取年份（如 "2023"）
            if let Some(info) = info_sel.as_ref().and_then(|s| el.select(s).next()) {
                let text = clean_text(&info.text().collect::<String>());
                if let Some(year) = extract_year(&text) {
                    r.published = Some(year);
                }
                if r.snippet.is_empty() && !text.is_empty() {
                    r.snippet = clip(&text, 300);
                }
            }
            out.push(r);
        }
        out
    }
}

/// 从文本中提取 4 位年份（1900~2099 区间）。
fn extract_year(text: &str) -> Option<String> {
    let mut it = text.split(|c: char| !c.is_ascii_digit());
    it.find_map(|tok| {
        if tok.len() == 4 {
            let y: i32 = tok.parse().ok()?;
            if (1900..=2099).contains(&y) {
                return Some(tok.to_string());
            }
        }
        None
    })
}

#[async_trait]
impl Engine for Xueshu {
    fn meta(&self) -> EngineMeta {
        META
    }

    async fn search(
        &self,
        ctx: &EngineContext,
        query: &str,
        max: usize,
    ) -> Result<Vec<SearchResult>, EngineError> {
        let url = format!(
            "https://xueshu.baidu.com/s?wd={}&ie=utf-8",
            encode_query_pct(query)
        );
        let html = ctx
            .http
            .get_with_headers("xueshu", &url, &[("Referer", "https://xueshu.baidu.com/")])
            .await
            .map_err(|e| EngineError::Http(e.to_string()))?
            .text()
            .await
            .map_err(|e| EngineError::Http(e.to_string()))?;

        let out = self.parse_html(&html, max);
        if out.is_empty() {
            Err(EngineError::Blocked("无结果或触发反爬".into()))
        } else {
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xueshu_results() {
        let html = r#"<html><body>
            <div class="result sc_default_result">
              <h3 class="t"><a href="https://xueshu.baidu.com/usercenter/paper/show?paperid=1">异步编程研究</a></h3>
              <div class="c_abstract">摘要一</div>
              <div class="sc_info">计算机学报 2023</div>
            </div>
            <div class="result sc_default_result">
              <h3 class="t"><a href="/paper/2">相对链接</a></h3>
            </div>
        </body></html>"#;
        let xu = Xueshu;
        let out = xu.parse_html(html, 10);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].title, "异步编程研究");
        assert_eq!(out[0].url, "https://xueshu.baidu.com/usercenter/paper/show?paperid=1");
        assert_eq!(out[0].published.as_deref(), Some("2023"));
        assert_eq!(out[1].url, "https://xueshu.baidu.com/paper/2");
        assert_eq!(out[0].source, "xueshu");
    }

    #[test]
    fn empty_page_yields_no_results() {
        let xu = Xueshu;
        assert!(xu.parse_html("<html><body></body></html>", 10).is_empty());
    }

    #[test]
    fn year_extraction() {
        assert_eq!(extract_year("计算机学报 2023"), Some("2023".into()));
        assert_eq!(extract_year("无年份"), None);
    }
}
