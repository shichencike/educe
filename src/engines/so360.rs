//! 360 搜索适配器（HTML 解析）。
//! 结果块：`li.res-list` -> `h3 a`（标题/链接），摘要 `.res-desc`。

use async_trait::async_trait;
use scraper::{Html, Selector};

use crate::engines::common::{absolute_url, clean_text, clip, encode_query_pct};
use crate::engines::{Engine, EngineContext, EngineError, error_detail};
use std::borrow::Cow;

use crate::models::{Category, EngineMeta, SearchResult};

pub const META: EngineMeta = EngineMeta {
    id: Cow::Borrowed("so360"),
    name: Cow::Borrowed("360 搜索"),
    category: Category::General,
    needs_js: false,
};

pub struct So360;

#[async_trait]
impl Engine for So360 {
    fn meta(&self) -> EngineMeta {
        META
    }

    async fn search(
        &self,
        ctx: &EngineContext,
        query: &str,
        max: usize,
    ) -> Result<Vec<SearchResult>, EngineError> {
        let url = format!("https://www.so.com/s?q={}&pn=1", encode_query_pct(query));
        let html = ctx
            .http
            .get_text("so360", &url)
            .await
            .map_err(|e| EngineError::Http(error_detail(&e)))?;

        let doc = Html::parse_document(&html);
        let result_sel =
            Selector::parse("li.res-list").map_err(|e| EngineError::Http(error_detail(&e)))?;
        let link_sel = Selector::parse("h3 a").map_err(|e| EngineError::Http(error_detail(&e)))?;
        let snip_sel =
            Selector::parse(".res-desc").map_err(|e| EngineError::Http(error_detail(&e)))?;

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
            let url = absolute_url("https://www.so.com/", href).unwrap_or_default();
            if title.is_empty() || url.is_empty() {
                continue;
            }
            let snippet = el
                .select(&snip_sel)
                .next()
                .map(|s| clip(&s.text().collect::<String>(), 300))
                .unwrap_or_default();
            out.push(SearchResult::new(title, url, snippet, "so360", out.len()));
        }

        if out.is_empty() {
            Err(EngineError::Blocked("无结果或触发反爬".into()))
        } else {
            Ok(out)
        }
    }
}
