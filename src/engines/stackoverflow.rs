//! Stack Overflow 搜索适配器（StackExchange API，无需鉴权，配额有限但稳定）。

use async_trait::async_trait;

use crate::engines::common::{clip, encode_query_pct};
use crate::engines::{Engine, EngineContext};
use std::borrow::Cow;

use crate::models::{Category, EngineMeta, SearchResult};

pub const META: EngineMeta = EngineMeta {
    id: Cow::Borrowed("stackoverflow"),
    name: Cow::Borrowed("Stack Overflow"),
    category: Category::Code,
    needs_js: false,
};

pub struct StackOverflow;

#[async_trait]
impl Engine for StackOverflow {
    fn meta(&self) -> EngineMeta {
        META
    }

    async fn search(
        &self,
        ctx: &EngineContext,
        query: &str,
        max: usize,
    ) -> Result<Vec<SearchResult>, String> {
        let pagesize = max.min(50);
        let url = format!(
            "https://api.stackexchange.com/2.3/search/advanced?order=desc&sort=relevance&q={}&site=stackoverflow&pagesize={}",
            encode_query_pct(query),
            pagesize
        );
        let resp = ctx
            .http
            .get("stackoverflow", &url)
            .await
            .map_err(|e| e.to_string())?;
        let v: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("解析 StackExchange 响应失败: {e}"))?;

        let mut out = Vec::new();
        if let Some(items) = v.get("items").and_then(|x| x.as_array()) {
            for (i, it) in items.iter().enumerate() {
                if out.len() >= max {
                    break;
                }
                let Some(title) = it.get("title").and_then(|x| x.as_str()) else {
                    continue;
                };
                let Some(link) = it.get("link").and_then(|x| x.as_str()) else {
                    continue;
                };
                let views = it.get("view_count").and_then(|x| x.as_i64()).unwrap_or(0);
                let answered = it.get("is_answered").and_then(|x| x.as_bool()).unwrap_or(false);
                let mut tags: Vec<&str> = it
                    .get("tags")
                    .and_then(|x| x.as_array())
                    .map(|a| a.iter().filter_map(|t| t.as_str()).collect())
                    .unwrap_or_default();
                if tags.len() > 4 {
                    tags.truncate(4);
                }
                let snippet = format!(
                    "👁 {views} · {}{}",
                    if answered { "✔ 已解答" } else { "未解答" },
                    if tags.is_empty() {
                        String::new()
                    } else {
                        format!(" · 标签: {}", tags.join(" / "))
                    }
                );
                out.push(SearchResult::new(
                    title.to_string(),
                    link.to_string(),
                    clip(&snippet, 300),
                    "stackoverflow",
                    i,
                ));
            }
        }

        if out.is_empty() {
            Err("Stack Overflow 无结果或配额用尽".into())
        } else {
            Ok(out)
        }
    }
}
