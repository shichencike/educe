//! DuckDuckGo 搜索适配器（HTML 解析，走 html.duckduckgo.com 轻量端点）。
//! 结果块：`div.result` -> `a.result__a`（标题/链接），摘要 `a.result__snippet`。

use async_trait::async_trait;
use scraper::{Html, Selector};

use crate::engines::common::{clean_text, clip, encode_query_pct};
use crate::engines::{Engine, EngineContext, EngineError};
use std::borrow::Cow;

use crate::models::{Category, EngineMeta, SearchResult};

pub const META: EngineMeta = EngineMeta {
    id: Cow::Borrowed("duckduckgo"),
    name: Cow::Borrowed("DuckDuckGo"),
    category: Category::General,
    needs_js: false,
};

pub struct DuckDuckGo;

#[async_trait]
impl Engine for DuckDuckGo {
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
            "https://html.duckduckgo.com/html/?q={}&kl=cn-zh",
            encode_query_pct(query)
        );
        let html = ctx
            .http
            .get_text("duckduckgo", &url)
            .await
            .map_err(|e| EngineError::Http(e.to_string()))?;

        let doc = Html::parse_document(&html);
        let result_sel =
            Selector::parse("div.result").map_err(|e| EngineError::Http(e.to_string()))?;
        let link_sel =
            Selector::parse("a.result__a").map_err(|e| EngineError::Http(e.to_string()))?;
        let snip_sel =
            Selector::parse("a.result__snippet").map_err(|e| EngineError::Http(e.to_string()))?;

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
            let url = if href.starts_with("http") {
                href.to_string()
            } else {
                format!("https://duckduckgo.com{}", href)
            };
            if title.is_empty() || url.is_empty() {
                continue;
            }
            let snippet = el
                .select(&snip_sel)
                .next()
                .map(|s| clip(&s.text().collect::<String>(), 300))
                .unwrap_or_default();
            out.push(SearchResult::new(
                title,
                url,
                snippet,
                "duckduckgo",
                out.len(),
            ));
        }

        if out.is_empty() {
            Err(EngineError::Blocked("无结果或触发反爬".into()))
        } else {
            Ok(out)
        }
    }
}
