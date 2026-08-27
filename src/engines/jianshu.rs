//! 简书搜索适配器（JS 渲染桥）。
//! 专用选择器：`div.note-list li` -> `a.title`（标题/链接），摘要 `.abstract`。

use async_trait::async_trait;
use scraper::{Html, Selector};

use crate::engines::common::{clean_text, clip, encode_query_pct, generic_extract};
use crate::engines::{error_detail, Engine, EngineContext, EngineError};
use std::borrow::Cow;

use crate::models::{Category, EngineMeta, SearchResult};

pub const META: EngineMeta = EngineMeta {
    id: Cow::Borrowed("jianshu"),
    name: Cow::Borrowed("简书"),
    category: Category::Chinese,
    needs_js: true,
};

pub struct Jianshu;

impl Jianshu {
    fn parse_html(&self, html: &str, max: usize) -> Vec<SearchResult> {
        let doc = Html::parse_document(html);
        let Ok(result_sel) = Selector::parse("div.note-list li") else {
            return Vec::new();
        };
        let Ok(link_sel) = Selector::parse("a.title") else {
            return Vec::new();
        };
        let Ok(snip_sel) = Selector::parse(".abstract") else {
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
            if title.is_empty() || href.is_empty() {
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
                "jianshu",
                out.len(),
            ));
        }
        out
    }
}

#[async_trait]
impl Engine for Jianshu {
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
                "简书需要 JS 渲染桥（配置 js_render.enabled=true 且 sources 含 jianshu）".into(),
            )
        })?;
        let url = format!(
            "https://www.jianshu.com/search?q={}&page=1&type=note",
            encode_query_pct(query)
        );
        let html = renderer
            .render(&url)
            .await
            .map_err(|e| EngineError::Http(error_detail(&e)))?;

        let mut out = self.parse_html(&html, max);
        if out.is_empty() {
            out = generic_extract(&html, "jianshu", max);
        }
        if out.is_empty() {
            Err(EngineError::Blocked("简书无结果（可能需要验证码）".into()))
        } else {
            Ok(out)
        }
    }
}
