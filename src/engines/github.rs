//! GitHub 搜索适配器（官方 REST API，无需鉴权，搜索限速 10 次/分钟）。

use async_trait::async_trait;

use crate::engines::common::{clip, encode_query_pct};
use crate::engines::{Engine, EngineContext, EngineError};
use std::borrow::Cow;

use crate::models::{Category, EngineMeta, SearchResult};

pub const META: EngineMeta = EngineMeta {
    id: Cow::Borrowed("github"),
    name: Cow::Borrowed("GitHub"),
    category: Category::Code,
    needs_js: false,
};

pub struct GitHub;

#[async_trait]
impl Engine for GitHub {
    fn meta(&self) -> EngineMeta {
        META
    }

    async fn search(
        &self,
        ctx: &EngineContext,
        query: &str,
        max: usize,
    ) -> Result<Vec<SearchResult>, EngineError> {
        let per_page = max.min(50);
        let url = format!(
            "https://api.github.com/search/repositories?q={}&per_page={}&sort=stars",
            encode_query_pct(query),
            per_page
        );
        let resp = ctx
            .http
            .get_with_headers("github", &url, &[("Accept", "application/vnd.github+json")])
            .await
            .map_err(|e| EngineError::Http(e.to_string()))?;

        if resp.status().as_u16() == 403 {
            return Err(EngineError::Blocked(
                "GitHub API 限流（搜索接口约 10 次/分钟）".into(),
            ));
        }
        let v: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| EngineError::Parse(format!("解析 GitHub 响应失败: {e}")))?;

        let mut out = Vec::new();
        if let Some(items) = v.get("items").and_then(|x| x.as_array()) {
            for (i, it) in items.iter().enumerate() {
                if out.len() >= max {
                    break;
                }
                let Some(full_name) = it.get("full_name").and_then(|x| x.as_str()) else {
                    continue;
                };
                let Some(html_url) = it.get("html_url").and_then(|x| x.as_str()) else {
                    continue;
                };
                let desc = it.get("description").and_then(|x| x.as_str()).unwrap_or("");
                let stars = it
                    .get("stargazers_count")
                    .and_then(|x| x.as_i64())
                    .unwrap_or(0);
                let lang = it
                    .get("language")
                    .and_then(|x| x.as_str())
                    .unwrap_or("未知语言");
                let snippet = if desc.is_empty() {
                    format!("⭐ {stars} · {lang}")
                } else {
                    format!("⭐ {stars} · {lang} — {desc}")
                };
                let mut r = SearchResult::new(
                    full_name.to_string(),
                    html_url.to_string(),
                    clip(&snippet, 300),
                    "github",
                    i,
                );
                if let Some(p) = it.get("pushed_at").and_then(|x| x.as_str()) {
                    r.published = Some(p[..10].to_string());
                }
                out.push(r);
            }
        }

        if out.is_empty() {
            Err(EngineError::Parse("GitHub 无结果".into()))
        } else {
            Ok(out)
        }
    }
}
