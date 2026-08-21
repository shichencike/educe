//! Bing 搜索适配器（HTML 解析）。
//! 结果块：`li.b_algo` -> `h2 a`（标题/链接），摘要 `p`。

use async_trait::async_trait;
use scraper::{Html, Selector};

use crate::engines::common::{absolute_url, clean_text, clip, encode_query_pct};
use crate::engines::{Engine, EngineContext};
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
    ) -> Result<Vec<SearchResult>, String> {
        let count = max.min(50);
        let url = format!(
            "https://www.bing.com/search?q={}&count={}&setlang=zh-hans&mkt=zh-CN&form=QBLH",
            encode_query_pct(query),
            count
        );
        let html = ctx.http.get_text("bing", &url).await.map_err(|e| e.to_string())?;

        let doc = Html::parse_document(&html);
        let result_sel = Selector::parse("li.b_algo").map_err(|e| e.to_string())?;
        let link_sel = Selector::parse("h2 a").map_err(|e| e.to_string())?;
        let snip_sel = Selector::parse("p").map_err(|e| e.to_string())?;

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

        if out.is_empty() {
            Err("无结果或触发反爬".into())
        } else {
            Ok(out)
        }
    }
}
