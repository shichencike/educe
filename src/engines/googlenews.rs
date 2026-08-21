//! Google News 新闻搜索适配器（RSS，无需鉴权，带地区/语言参数）。

use async_trait::async_trait;
use scraper::{Html, Selector};

use crate::engines::common::{clean_text, clip, encode_query_pct, strip_html};
use crate::engines::{Engine, EngineContext};
use std::borrow::Cow;

use crate::models::{Category, EngineMeta, SearchResult};

pub const META: EngineMeta = EngineMeta {
    id: Cow::Borrowed("googlenews"),
    name: Cow::Borrowed("Google 新闻"),
    category: Category::Academic,
    needs_js: false,
};

pub struct GoogleNews;

#[async_trait]
impl Engine for GoogleNews {
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
            "https://news.google.com/rss/search?q={}&hl=zh-CN&gl=CN&ceid=CN:zh-Hans",
            encode_query_pct(query)
        );
        let xml = ctx
            .http
            .get_text("googlenews", &url)
            .await
            .map_err(|e| e.to_string())?;

        let doc = Html::parse_document(&xml);
        let item_sel = Selector::parse("item").map_err(|e| e.to_string())?;
        let title_sel = Selector::parse("title").map_err(|e| e.to_string())?;
        let link_sel = Selector::parse("link").map_err(|e| e.to_string())?;
        let desc_sel = Selector::parse("description").map_err(|e| e.to_string())?;
        let date_sel = Selector::parse("pubDate").map_err(|e| e.to_string())?;

        let mut out = Vec::new();
        for el in doc.select(&item_sel) {
            if out.len() >= max {
                break;
            }
            let title = el
                .select(&title_sel)
                .next()
                .map(|t| clean_text(&t.text().collect::<String>()))
                .unwrap_or_default();
            let url = el
                .select(&link_sel)
                .next()
                .map(|t| clean_text(&t.text().collect::<String>()))
                .unwrap_or_default();
            if title.is_empty() || url.is_empty() {
                continue;
            }
            let snippet = el
                .select(&desc_sel)
                .next()
                .map(|t| {
                    let raw = clean_text(&t.text().collect::<String>());
                    clip(&strip_html(&raw), 300)
                })
                .unwrap_or_default();
            let mut r = SearchResult::new(title, url, snippet, "googlenews", out.len());
            if let Some(d) = el.select(&date_sel).next() {
                let d = clean_text(&d.text().collect::<String>());
                // pubDate 形如 "Fri, 21 Aug 2026 10:00:00 GMT"，截取日期段
                if d.len() >= 16 {
                    r.published = Some(d[5..16].to_string());
                }
            }
            out.push(r);
        }

        if out.is_empty() {
            Err("Google 新闻无结果或网络不可达".into())
        } else {
            Ok(out)
        }
    }
}
