//! Google 搜索适配器。
//! 策略：先尝试轻量 HTML 请求（需 consent cookie）；若结果为空或反爬，
//! 且配置了 JS 渲染桥，则退回桥接渲染。桥接实现在 jsrender 模块（任务 8）。
//! 结果块（HTML 版）：`div.g` -> `a[href^=http]` + `h3`，摘要 `.VwiC3b`。

use async_trait::async_trait;
use scraper::{Html, Selector};

use crate::engines::common::{clean_text, clip, encode_query_pct};
use crate::engines::{error_detail, Engine, EngineContext, EngineError};
use std::borrow::Cow;

use crate::models::{Category, EngineMeta, SearchResult};

pub const META: EngineMeta = EngineMeta {
    id: Cow::Borrowed("google"),
    name: Cow::Borrowed("Google"),
    category: Category::General,
    needs_js: true,
};

pub struct Google;

impl Google {
    /// 轻量 HTML 解析（不执行 JS）。返回 (title, url, snippet) 列表。
    fn parse_html(&self, html: &str) -> Vec<(String, String, String)> {
        let doc = Html::parse_document(html);
        let Ok(result_sel) = Selector::parse("div.g") else {
            return Vec::new();
        };
        let Ok(link_sel) = Selector::parse("a[href^='http']") else {
            return Vec::new();
        };
        let Ok(title_sel) = Selector::parse("h3") else {
            return Vec::new();
        };
        let Ok(snip_sel) = Selector::parse(".VwiC3b") else {
            return Vec::new();
        };

        let mut out = Vec::new();
        for el in doc.select(&result_sel) {
            let Some(a) = el.select(&link_sel).next() else {
                continue;
            };
            let href = a.value().attr("href").unwrap_or("");
            let Some(h3) = el.select(&title_sel).next() else {
                continue;
            };
            let title = clean_text(&h3.text().collect::<String>());
            if title.is_empty() || href.is_empty() {
                continue;
            }
            let snippet = el
                .select(&snip_sel)
                .next()
                .map(|s| clip(&s.text().collect::<String>(), 300))
                .unwrap_or_default();
            out.push((title, href.to_string(), snippet));
        }
        out
    }
}

#[async_trait]
impl Engine for Google {
    fn meta(&self) -> EngineMeta {
        META
    }

    async fn search(
        &self,
        ctx: &EngineContext,
        query: &str,
        max: usize,
    ) -> Result<Vec<SearchResult>, EngineError> {
        let num = max.min(100);
        let url = format!(
            "https://www.google.com/search?q={}&num={}&hl=zh-CN&gl=us",
            encode_query_pct(query),
            num
        );

        // 尝试 1：轻量 HTML（带 consent cookie，_/chrome-update-frameworks=1 防重定向）
        let html = ctx
            .http
            .get_with_headers(
                "google",
                &url,
                &[(
                    "Cookie",
                    "CONSENT=YES+; SOCS=CAISHAgBEhJnd3NfMjAyNDA4MDEtMF9SQzIaAmNuIAEaBgiA_LyaBg",
                )],
            )
            .await
            .map_err(|e| EngineError::Http(error_detail(&e)))?
            .text()
            .await
            .map_err(|e| EngineError::Http(error_detail(&e)))?;

        let parsed = self.parse_html(&html);
        if parsed.is_empty() {
            // 尝试 2：JS 渲染桥（需启用 js_render 并配置 sources 含 google）
            if let Some(renderer) = &ctx.js_render {
                let rendered = renderer
                    .render(&url)
                    .await
                    .map_err(|e| EngineError::Http(error_detail(&e)))?;
                let parsed2 = self.parse_html(&rendered);
                if !parsed2.is_empty() {
                    return Ok(parsed2
                        .into_iter()
                        .take(max)
                        .enumerate()
                        .map(|(i, (title, url, snippet))| {
                            SearchResult::new(title, url, snippet, "google", i)
                        })
                        .collect());
                }
            }
            return Err(EngineError::Blocked(
                "无结果（Google 反爬较强，建议启用 JS 渲染桥）".into(),
            ));
        }

        let results: Vec<SearchResult> = parsed
            .into_iter()
            .take(max)
            .enumerate()
            .map(|(i, (title, url, snippet))| SearchResult::new(title, url, snippet, "google", i))
            .collect();
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_google_results() {
        let html = r#"<html><body>
            <div class="g"><a href="https://x.com/1"><h3>标题一</h3></a><div class="VwiC3b">摘要一</div></div>
            <div class="g"><a href="https://x.com/2"><h3>标题二</h3></a></div>
        </body></html>"#;
        let g = Google;
        let out = g.parse_html(html);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0, "标题一");
        assert_eq!(out[0].1, "https://x.com/1");
        assert_eq!(out[0].2, "摘要一");
        assert_eq!(out[1].0, "标题二");
    }
}
