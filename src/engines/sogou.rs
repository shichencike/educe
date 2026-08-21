//! 搜狗搜索适配器（HTML 解析）。
//! 结果块：`div.vrwrap` -> `h3 a`（标题/链接），摘要 `.text-layout` / `.str_info`。
//! 注意：搜狗有 JS 挑战页，首次请求可能被拦，属预期（故障隔离）。

use async_trait::async_trait;
use scraper::{Html, Selector};

use crate::engines::common::{absolute_url, clean_text, clip, encode_query_pct};
use crate::engines::{Engine, EngineContext};
use std::borrow::Cow;

use crate::models::{Category, EngineMeta, SearchResult};

pub const META: EngineMeta = EngineMeta {
    id: Cow::Borrowed("sogou"),
    name: Cow::Borrowed("搜狗"),
    category: Category::General,
    needs_js: false,
};

pub struct Sogou;

#[async_trait]
impl Engine for Sogou {
    fn meta(&self) -> EngineMeta {
        META
    }

    async fn search(
        &self,
        ctx: &EngineContext,
        query: &str,
        max: usize,
    ) -> Result<Vec<SearchResult>, String> {
        let url = format!(
            "https://www.sogou.com/web?query={}&ie=utf8",
            encode_query_pct(query)
        );
        let html = ctx.http.get_text("sogou", &url).await.map_err(|e| e.to_string())?;

        let doc = Html::parse_document(&html);
        let result_sel = Selector::parse("div.vrwrap").map_err(|e| e.to_string())?;
        let link_sel = Selector::parse("h3 a").map_err(|e| e.to_string())?;
        let snip_sels = [
            Selector::parse(".text-layout").map_err(|e| e.to_string())?,
            Selector::parse(".str_info").map_err(|e| e.to_string())?,
            Selector::parse(".star-wiki").map_err(|e| e.to_string())?,
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
            let url = absolute_url("https://www.sogou.com/", href).unwrap_or_default();
            if title.is_empty() || url.is_empty() {
                continue;
            }
            let snippet = snip_sels
                .iter()
                .find_map(|s| el.select(s).next())
                .map(|s| clip(&s.text().collect::<String>(), 300))
                .unwrap_or_default();
            out.push(SearchResult::new(title, url, snippet, "sogou", out.len()));
        }

        if out.is_empty() {
            Err("无结果或触发反爬".into())
        } else {
            Ok(out)
        }
    }
}
