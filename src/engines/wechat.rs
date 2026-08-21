//! 微信公众号搜索适配器（经搜狗微信搜索，JS 渲染桥）。
//! 专用选择器：`ul.news-list li` -> `h3 a`（标题/链接），摘要 `.txt-info`。
//! 搜狗微信反爬较重（需 SNUID/SUV cookie、常出验证码），失败属预期。

use async_trait::async_trait;
use scraper::{Html, Selector};

use crate::engines::common::{clean_text, clip, encode_query_pct, generic_extract};
use crate::engines::{Engine, EngineContext};
use std::borrow::Cow;

use crate::models::{Category, EngineMeta, SearchResult};

pub const META: EngineMeta = EngineMeta {
    id: Cow::Borrowed("wechat"),
    name: Cow::Borrowed("微信公众号"),
    category: Category::Chinese,
    needs_js: true,
};

pub struct Wechat;

impl Wechat {
    fn parse_html(&self, html: &str, max: usize) -> Vec<SearchResult> {
        let doc = Html::parse_document(html);
        let Ok(result_sel) = Selector::parse("ul.news-list li") else {
            return Vec::new();
        };
        let Ok(link_sel) = Selector::parse("h3 a") else {
            return Vec::new();
        };
        let Ok(snip_sel) = Selector::parse(".txt-info") else {
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
            out.push(SearchResult::new(title, href.to_string(), snippet, "wechat", out.len()));
        }
        out
    }
}

#[async_trait]
impl Engine for Wechat {
    fn meta(&self) -> EngineMeta {
        META
    }

    async fn search(
        &self,
        ctx: &EngineContext,
        query: &str,
        max: usize,
    ) -> Result<Vec<SearchResult>, String> {
        let renderer = ctx
            .js_render
            .as_ref()
            .ok_or("微信公众号需要 JS 渲染桥（配置 js_render.enabled=true 且 sources 含 wechat）")?;
        let url = format!(
            "https://weixin.sogou.com/weixin?type=2&query={}",
            encode_query_pct(query)
        );
        let html = renderer.render(&url).await.map_err(|e| e.to_string())?;

        let mut out = self.parse_html(&html, max);
        if out.is_empty() {
            out = generic_extract(&html, "wechat", max);
        }
        if out.is_empty() {
            Err("微信公众号无结果（搜狗反爬较重，可能需要验证码）".into())
        } else {
            Ok(out)
        }
    }
}
