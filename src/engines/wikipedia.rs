//! 维基百科搜索适配器（MediaWiki Action API，中文站）。
//! 结果条目中的 snippet 为带高亮的 HTML，需去标签。

use async_trait::async_trait;

use crate::engines::common::{clip, encode_query_pct, strip_html};
use crate::engines::{Engine, EngineContext, EngineError, error_detail};
use std::borrow::Cow;

use crate::models::{Category, EngineMeta, SearchResult};

pub const META: EngineMeta = EngineMeta {
    id: Cow::Borrowed("wikipedia"),
    name: Cow::Borrowed("维基百科"),
    category: Category::Academic,
    needs_js: false,
};

pub struct Wikipedia;

#[async_trait]
impl Engine for Wikipedia {
    fn meta(&self) -> EngineMeta {
        META
    }

    async fn search(
        &self,
        ctx: &EngineContext,
        query: &str,
        max: usize,
    ) -> Result<Vec<SearchResult>, EngineError> {
        let srlimit = max.min(50);
        let url = format!(
            "https://zh.wikipedia.org/w/api.php?action=query&list=search&srsearch={}&srlimit={}&format=json&utf8=1",
            encode_query_pct(query),
            srlimit
        );
        let resp = ctx
            .http
            .get("wikipedia", &url)
            .await
            .map_err(|e| EngineError::Http(error_detail(&e)))?;
        let v: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| EngineError::Parse(format!("解析维基百科响应失败: {e}")))?;

        let mut out = Vec::new();
        if let Some(items) = v.pointer("/query/search").and_then(|x| x.as_array()) {
            for (i, it) in items.iter().enumerate() {
                if out.len() >= max {
                    break;
                }
                let Some(title) = it.get("title").and_then(|x| x.as_str()) else {
                    continue;
                };
                let page_url = format!(
                    "https://zh.wikipedia.org/wiki/{}",
                    encode_query_pct(&title.replace(' ', "_"))
                );
                let snippet = it
                    .get("snippet")
                    .and_then(|x| x.as_str())
                    .map(strip_html)
                    .unwrap_or_default();
                let mut r = SearchResult::new(
                    title.to_string(),
                    page_url,
                    clip(&snippet, 300),
                    "wikipedia",
                    i,
                );
                if let Some(ts) = it.get("timestamp").and_then(|x| x.as_str()) {
                    if ts.len() >= 10 {
                        r.published = Some(ts[..10].to_string());
                    }
                }
                out.push(r);
            }
        }

        if out.is_empty() {
            Err(EngineError::Parse("维基百科无结果".into()))
        } else {
            Ok(out)
        }
    }
}
