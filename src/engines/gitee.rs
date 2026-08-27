//! Gitee 搜索适配器。
//! 优先走官方 API v5（search/repositories）；失败则退回 HTML 解析
//! （search.gitee.com 结构变动频繁，属尽力而为）。

use async_trait::async_trait;
use scraper::{Html, Selector};

use crate::engines::common::{clean_text, clip, encode_query_pct};
use crate::engines::{Engine, EngineContext, EngineError, error_detail};
use std::borrow::Cow;

use crate::models::{Category, EngineMeta, SearchResult};

pub const META: EngineMeta = EngineMeta {
    id: Cow::Borrowed("gitee"),
    name: Cow::Borrowed("Gitee"),
    category: Category::Code,
    needs_js: false,
};

pub struct Gitee;

impl Gitee {
    /// 官方 API：search/repositories，返回 items[]。
    async fn via_api(
        &self,
        ctx: &EngineContext,
        query: &str,
        max: usize,
    ) -> Result<Vec<SearchResult>, EngineError> {
        let url = format!(
            "https://gitee.com/api/v5/search/repositories?q={}&per_page={}",
            encode_query_pct(query),
            max.min(50)
        );
        let resp = ctx
            .http
            .get("gitee", &url)
            .await
            .map_err(|e| EngineError::Http(error_detail(&e)))?;
        if !resp.status().is_success() {
            return Err(EngineError::Http(format!(
                "API HTTP {}",
                resp.status().as_u16()
            )));
        }
        let v: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| EngineError::Parse(format!("解析 Gitee API 失败: {e}")))?;
        let mut out = Vec::new();
        if let Some(items) = v.as_array() {
            for (i, it) in items.iter().enumerate() {
                if out.len() >= max {
                    break;
                }
                let full_name = it.get("full_name").and_then(|x| x.as_str()).unwrap_or("");
                let html_url = it.get("html_url").and_then(|x| x.as_str()).unwrap_or("");
                if full_name.is_empty() || html_url.is_empty() {
                    continue;
                }
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
                    "gitee",
                    i,
                );
                if let Some(p) = it.get("pushed_at").and_then(|x| x.as_str()) {
                    if p.len() >= 10 {
                        r.published = Some(p[..10].to_string());
                    }
                }
                out.push(r);
            }
        }
        Ok(out)
    }

    /// HTML 解析（尽力而为）：`.repository-list li` -> `a.title`。
    async fn via_html(
        &self,
        ctx: &EngineContext,
        query: &str,
        max: usize,
    ) -> Result<Vec<SearchResult>, EngineError> {
        let url = format!(
            "https://search.gitee.com/?q={}&type=repository",
            encode_query_pct(query)
        );
        let html = ctx
            .http
            .get_text("gitee", &url)
            .await
            .map_err(|e| EngineError::Http(error_detail(&e)))?;
        let doc = Html::parse_document(&html);
        let result_sel = Selector::parse("li.project-item, li[class*='repository']")
            .map_err(|e| EngineError::Http(error_detail(&e)))?;
        let link_sel = Selector::parse("a.title, a[class*='title']")
            .map_err(|e| EngineError::Http(error_detail(&e)))?;
        let snip_sel = Selector::parse(".desc, .project-desc")
            .map_err(|e| EngineError::Http(error_detail(&e)))?;

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
                format!("https://gitee.com{}", href)
            };
            if title.is_empty() || url.is_empty() {
                continue;
            }
            let snippet = el
                .select(&snip_sel)
                .next()
                .map(|s| clip(&s.text().collect::<String>(), 300))
                .unwrap_or_default();
            out.push(SearchResult::new(title, url, snippet, "gitee", out.len()));
        }
        Ok(out)
    }
}

#[async_trait]
impl Engine for Gitee {
    fn meta(&self) -> EngineMeta {
        META
    }

    async fn search(
        &self,
        ctx: &EngineContext,
        query: &str,
        max: usize,
    ) -> Result<Vec<SearchResult>, EngineError> {
        // 先 API
        match self.via_api(ctx, query, max).await {
            Ok(items) if !items.is_empty() => return Ok(items),
            _ => {}
        }
        // 退回 HTML
        let items = self.via_html(ctx, query, max).await?;
        if items.is_empty() {
            Err(EngineError::Parse("Gitee 无结果或页面结构变化".into()))
        } else {
            Ok(items)
        }
    }
}
