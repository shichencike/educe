//! Bing 搜索适配器（HTML 解析）。
//! 结果块：`li.b_algo` -> `h2 a`（标题/链接），摘要 `p`。

use async_trait::async_trait;
use scraper::{Html, Selector};

use crate::engines::common::{absolute_url, clean_text, clip, encode_query_pct};
use crate::engines::{Engine, EngineContext, EngineError};
use std::borrow::Cow;

use crate::models::{Category, EngineMeta, SearchResult};

pub const META: EngineMeta = EngineMeta {
    id: Cow::Borrowed("bing"),
    name: Cow::Borrowed("必应"),
    category: Category::General,
    needs_js: false,
};

pub struct Bing;

#[async_trait]
impl Engine for Bing {
    fn meta(&self) -> EngineMeta {
        META
    }

    async fn search(
        &self,
        ctx: &EngineContext,
        query: &str,
        max: usize,
    ) -> Result<Vec<SearchResult>, EngineError> {
        let count = max.min(50);
        let url = format!(
            "https://www.bing.com/search?q={}&count={}&setlang=zh-hans&mkt=zh-CN&form=QBLH",
            encode_query_pct(query),
            count
        );
        let html = ctx
            .http
            .get_text("bing", &url)
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

impl Bing {
    /// 解析 bing 结果页 HTML（独立方法便于单元测试）。
    fn parse_html(&self, html: &str, max: usize) -> Vec<SearchResult> {
        let doc = Html::parse_document(html);
        let Ok(result_sel) = Selector::parse("li.b_algo") else {
            return Vec::new();
        };
        let Ok(link_sel) = Selector::parse("h2 a") else {
            return Vec::new();
        };
        let Ok(snip_sel) = Selector::parse("p") else {
            return Vec::new();
        };

        let mut out = Vec::new();
        for el in doc.select(&result_sel) {
            if out.len() >= max {
                break;
            }
            let Some(a) = el.select(&link_sel).next() else {
                continue;
            };
            let title = clean_text(&a.text().collect::<String>());
            let href = a.value().attr("href").unwrap_or("");
            let url = absolute_url("https://www.bing.com/", href).unwrap_or_default();
            if title.is_empty() || url.is_empty() {
                continue;
            }
            let snippet = el
                .select(&snip_sel)
                .next()
                .map(|s| clip(&s.text().collect::<String>(), 300))
                .unwrap_or_default();
            out.push(SearchResult::new(title, url, snippet, "bing", out.len()));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bing_results() {
        let html = r#"<html><body>
            <li class="b_algo"><h2><a href="https://example.com/rust">Rust 教程</a></h2><p>摘要一</p></li>
            <li class="b_algo"><h2><a href="/relative">相对链接</a></h2><p>摘要二</p></li>
        </body></html>"#;
        let bing = Bing;
        let out = bing.parse_html(html, 10);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].title, "Rust 教程");
        assert_eq!(out[0].url, "https://example.com/rust");
        assert_eq!(out[1].url, "https://www.bing.com/relative");
        assert_eq!(out[0].source, "bing");
    }

    #[test]
    fn empty_page_yields_no_results() {
        let bing = Bing;
        assert!(bing.parse_html("<html><body></body></html>", 10).is_empty());
    }
}
