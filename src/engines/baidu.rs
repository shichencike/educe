//! 百度搜索适配器（HTML 解析）。
//! 结果块：`div.result` -> `h3 a`（标题/链接），摘要 `.c-abstract`。

use async_trait::async_trait;
use scraper::{Html, Selector};

use crate::engines::common::{absolute_url, clean_text, clip, encode_query_pct};
use crate::engines::{Engine, EngineContext, EngineError};
use std::borrow::Cow;

use crate::models::{Category, EngineMeta, SearchResult};

pub const META: EngineMeta = EngineMeta {
    id: Cow::Borrowed("baidu"),
    name: Cow::Borrowed("百度"),
    category: Category::General,
    needs_js: false,
};

pub struct Baidu;

#[async_trait]
impl Engine for Baidu {
    fn meta(&self) -> EngineMeta {
        META
    }

    async fn search(
        &self,
        ctx: &EngineContext,
        query: &str,
        max: usize,
    ) -> Result<Vec<SearchResult>, EngineError> {
        let rn = max.min(50);
        let url = format!(
            "https://www.baidu.com/s?wd={}&rn={}&ie=utf-8&pn=0",
            encode_query_pct(query),
            rn
        );
        let html = ctx
            .http
            .get_with_headers("baidu", &url, &[("Referer", "https://www.baidu.com/")])
            .await
            .map_err(|e| EngineError::Http(e.to_string()))?
            .text()
            .await
            .map_err(|e| EngineError::Http(e.to_string()))?;

        let doc = Html::parse_document(&html);
        let result_sel =
            Selector::parse("div.result").map_err(|e| EngineError::Http(e.to_string()))?;
        let link_sel = Selector::parse("h3 a").map_err(|e| EngineError::Http(e.to_string()))?;
        let snip_sels = [
            Selector::parse(".c-abstract").map_err(|e| EngineError::Http(e.to_string()))?,
            Selector::parse(".content-right_8Zs40")
                .map_err(|e| EngineError::Http(e.to_string()))?,
        ];

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
            let url = absolute_url("https://www.baidu.com/", href).unwrap_or_default();
            if title.is_empty() || url.is_empty() {
                continue;
            }
            let snippet = snip_sels
                .iter()
                .find_map(|s| el.select(s).next())
                .map(|s| clip(&s.text().collect::<String>(), 300))
                .unwrap_or_default();
            out.push(SearchResult::new(title, url, snippet, "baidu", out.len()));
        }

        if out.is_empty() {
            Err(EngineError::Blocked("无结果或触发反爬".into()))
        } else {
            Ok(out)
        }
    }
}
