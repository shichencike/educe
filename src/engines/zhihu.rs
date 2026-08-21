//! 知乎搜索适配器（JS 渲染桥）。
//! 知乎搜索页为 SPA，需浏览器执行 JS；登录墙/验证码存在时可能无结果。
//! 专用选择器：`div.SearchResult-Card` -> `h2.ContentItem-title a`，摘要 `.RichText`。

use async_trait::async_trait;
use scraper::{Html, Selector};

use crate::engines::common::{clean_text, clip, encode_query_pct, generic_extract};
use crate::engines::{Engine, EngineContext, EngineError};
use std::borrow::Cow;

use crate::models::{Category, EngineMeta, SearchResult};

pub const META: EngineMeta = EngineMeta {
    id: Cow::Borrowed("zhihu"),
    name: Cow::Borrowed("知乎"),
    category: Category::Chinese,
    needs_js: true,
};

pub struct Zhihu;

impl Zhihu {
    fn parse_html(&self, html: &str, max: usize) -> Vec<SearchResult> {
        let doc = Html::parse_document(html);
        let Ok(result_sel) = Selector::parse("div.SearchResult-Card") else {
            return Vec::new();
        };
        let Ok(link_sel) = Selector::parse("h2.ContentItem-title a") else {
            return Vec::new();
        };
        let Ok(snip_sel) = Selector::parse(".RichText") else {
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
            if title.is_empty() || href.is_empty() || !href.starts_with("http") {
                continue;
            }
            let snippet = el
                .select(&snip_sel)
                .next()
                .map(|s| clip(&s.text().collect::<String>(), 300))
                .unwrap_or_default();
            out.push(SearchResult::new(
                title,
                href.to_string(),
                snippet,
                "zhihu",
                out.len(),
            ));
        }
        out
    }
}

#[async_trait]
impl Engine for Zhihu {
    fn meta(&self) -> EngineMeta {
        META
    }

    async fn search(
        &self,
        ctx: &EngineContext,
        query: &str,
        max: usize,
    ) -> Result<Vec<SearchResult>, EngineError> {
        let renderer = ctx.js_render.as_ref().ok_or_else(|| {
            EngineError::Blocked(
                "知乎需要 JS 渲染桥（配置 js_render.enabled=true 且 sources 含 zhihu）".into(),
            )
        })?;
        let url = format!(
            "https://www.zhihu.com/search?type=content&q={}",
            encode_query_pct(query)
        );
        let html = renderer
            .render(&url)
            .await
            .map_err(|e| EngineError::Http(e.to_string()))?;

        let mut out = self.parse_html(&html, max);
        if out.is_empty() {
            // 兜底：通用提取
            out = generic_extract(&html, "zhihu", max);
        }
        if out.is_empty() {
            Err(EngineError::Blocked(
                "知乎无结果（可能需要登录/验证码）".into(),
            ))
        } else {
            Ok(out)
        }
    }
}
