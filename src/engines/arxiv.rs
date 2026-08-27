//! arXiv 搜索适配器（官方 Atom API，无需鉴权）。

use async_trait::async_trait;
use scraper::{Html, Selector};

use crate::engines::common::{clean_text, clip, encode_query_pct};
use crate::engines::{error_detail, Engine, EngineContext, EngineError};
use std::borrow::Cow;

use crate::models::{Category, EngineMeta, SearchResult};

pub const META: EngineMeta = EngineMeta {
    id: Cow::Borrowed("arxiv"),
    name: Cow::Borrowed("arXiv"),
    category: Category::Academic,
    needs_js: false,
};

pub struct Arxiv;

#[async_trait]
impl Engine for Arxiv {
    fn meta(&self) -> EngineMeta {
        META
    }

    async fn search(
        &self,
        ctx: &EngineContext,
        query: &str,
        max: usize,
    ) -> Result<Vec<SearchResult>, EngineError> {
        let url = format!(
            "https://export.arxiv.org/api/query?search_query=all:{}&start=0&max_results={}",
            encode_query_pct(query),
            max.min(50)
        );
        let xml = ctx
            .http
            .get_text("arxiv", &url)
            .await
            .map_err(|e| EngineError::Http(error_detail(&e)))?;

        // Atom XML 经 html5ever 解析后按标签名选择（简单可靠）
        let doc = Html::parse_document(&xml);
        let entry_sel =
            Selector::parse("entry").map_err(|e| EngineError::Http(error_detail(&e)))?;
        let title_sel =
            Selector::parse("title").map_err(|e| EngineError::Http(error_detail(&e)))?;
        let id_sel = Selector::parse("id").map_err(|e| EngineError::Http(error_detail(&e)))?;
        let sum_sel =
            Selector::parse("summary").map_err(|e| EngineError::Http(error_detail(&e)))?;
        let pub_sel =
            Selector::parse("published").map_err(|e| EngineError::Http(error_detail(&e)))?;

        let mut out = Vec::new();
        for el in doc.select(&entry_sel) {
            if out.len() >= max {
                break;
            }
            let title = el
                .select(&title_sel)
                .next()
                .map(|t| clean_text(&t.text().collect::<String>()))
                .unwrap_or_default();
            let url = el
                .select(&id_sel)
                .next()
                .map(|t| clean_text(&t.text().collect::<String>()))
                .unwrap_or_default();
            if title.is_empty() || url.is_empty() {
                continue;
            }
            let snippet = el
                .select(&sum_sel)
                .next()
                .map(|s| clip(&s.text().collect::<String>(), 300))
                .unwrap_or_default();
            let mut r = SearchResult::new(title, url, snippet, "arxiv", out.len());
            if let Some(p) = el.select(&pub_sel).next() {
                let p = clean_text(&p.text().collect::<String>());
                if p.len() >= 10 {
                    r.published = Some(p[..10].to_string());
                }
            }
            out.push(r);
        }

        if out.is_empty() {
            Err(EngineError::Parse("arXiv 无结果".into()))
        } else {
            Ok(out)
        }
    }
}
