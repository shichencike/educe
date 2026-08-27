//! Startpage 搜索适配器（HTML 解析）。
//! 结果块：`section.w-gl__result` -> `h2.w-gl__result-title a`（标题/链接），摘要 `.w-gl__description`。
//! Startpage 反爬较重，若返回验证码页则按无结果处理（故障隔离）。

use async_trait::async_trait;
use scraper::{Html, Selector};

use crate::engines::common::{clean_text, clip, encode_query_pct};
use crate::engines::{Engine, EngineContext, EngineError, error_detail};
use std::borrow::Cow;

use crate::models::{Category, EngineMeta, SearchResult};

pub const META: EngineMeta = EngineMeta {
    id: Cow::Borrowed("startpage"),
    name: Cow::Borrowed("Startpage"),
    category: Category::General,
    needs_js: false,
};

pub struct Startpage;

impl Startpage {
    /// 解析 startpage 结果页 HTML（独立方法便于单元测试）。
    fn parse_html(&self, html: &str, max: usize) -> Vec<SearchResult> {
        let doc = Html::parse_document(html);
        // 结果容器：新版 w-gl__result，旧版 result；多选择器回退
        let result_sels = [
            Selector::parse("section.w-gl__result").ok(),
            Selector::parse("section.result").ok(),
            Selector::parse("div.w-gl__result").ok(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        let link_sels = [
            Selector::parse("h2.w-gl__result-title a").ok(),
            Selector::parse("h2 a").ok(),
            Selector::parse("a.w-gl__result-title-link").ok(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        let snip_sels = [
            Selector::parse(".w-gl__description").ok(),
            Selector::parse(".description").ok(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

        let mut out = Vec::new();
        for el in doc.select(result_sels.iter().next().unwrap_or_else(|| unreachable!())) {
            if out.len() >= max {
                break;
            }
            let Some(a) = link_sels.iter().find_map(|s| el.select(s).next()) else {
                continue;
            };
            let title = clean_text(&a.text().collect::<String>());
            let href = a.value().attr("href").unwrap_or("");
            let url = if href.starts_with("http") {
                href.to_string()
            } else {
                format!("https://www.startpage.com{}", href)
            };
            if title.is_empty() || url.is_empty() {
                continue;
            }
            let snippet = snip_sels
                .iter()
                .find_map(|s| el.select(s).next())
                .map(|s| clip(&s.text().collect::<String>(), 300))
                .unwrap_or_default();
            out.push(SearchResult::new(
                title,
                url,
                snippet,
                "startpage",
                out.len(),
            ));
        }
        out
    }
}

#[async_trait]
impl Engine for Startpage {
    fn meta(&self) -> EngineMeta {
        META
    }

    async fn search(
        &self,
        ctx: &EngineContext,
        query: &str,
        max: usize,
    ) -> Result<Vec<SearchResult>, EngineError> {
        // 无 JS 参数 + 中文界面语言；max_results 交给后端截断
        let url = format!(
            "https://www.startpage.com/sp/search?query={}&language=chinese_simplified&cat=web",
            encode_query_pct(query)
        );
        let html = ctx
            .http
            .get_with_headers(
                "startpage",
                &url,
                &[("Referer", "https://www.startpage.com/")],
            )
            .await
            .map_err(|e| EngineError::Http(error_detail(&e)))?
            .text()
            .await
            .map_err(|e| EngineError::Http(error_detail(&e)))?;

        let out = self.parse_html(&html, max);
        if out.is_empty() {
            Err(EngineError::Blocked(
                "无结果或触发反爬（Startpage 有验证码防护）".into(),
            ))
        } else {
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_startpage_results() {
        let html = r#"<html><body>
            <section class="w-gl__result">
              <h2 class="w-gl__result-title"><a href="https://example.com/rust">Rust 教程</a></h2>
              <p class="w-gl__description">摘要一</p>
            </section>
            <section class="w-gl__result">
              <h2 class="w-gl__result-title"><a href="/relative">相对链接</a></h2>
            </section>
        </body></html>"#;
        let sp = Startpage;
        let out = sp.parse_html(html, 10);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].title, "Rust 教程");
        assert_eq!(out[0].url, "https://example.com/rust");
        assert_eq!(out[1].url, "https://www.startpage.com/relative");
        assert_eq!(out[0].source, "startpage");
    }

    #[test]
    fn empty_page_yields_no_results() {
        let sp = Startpage;
        assert!(sp.parse_html("<html><body></body></html>", 10).is_empty());
    }
}
