//! 掘金搜索适配器（官方 JSON API）。
//! 接口：https://api.juejin.cn/search_api/v1/search，id_type=0 文章类型。
//! 注意：掘金会校验 Referer，需带上。

use async_trait::async_trait;

use crate::engines::common::{clip, encode_query_pct};
use crate::engines::{Engine, EngineContext};
use std::borrow::Cow;

use crate::models::{Category, EngineMeta, SearchResult};

pub const META: EngineMeta = EngineMeta {
    id: Cow::Borrowed("juejin"),
    name: Cow::Borrowed("掘金"),
    category: Category::Code,
    needs_js: false,
};

pub struct Juejin;

#[async_trait]
impl Engine for Juejin {
    fn meta(&self) -> EngineMeta {
        META
    }

    async fn search(
        &self,
        ctx: &EngineContext,
        query: &str,
        max: usize,
    ) -> Result<Vec<SearchResult>, String> {
        let page_size = max.min(20);
        let url = format!(
            "https://api.juejin.cn/search_api/v1/search?query={}&id_type=0&sort_type=0&page=0&page_size={}",
            encode_query_pct(query),
            page_size
        );
        let resp = ctx
            .http
            .get_with_headers(
                "juejin",
                &url,
                &[(
                    "Referer",
                    "https://juejin.cn/search",
                )],
            )
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status().as_u16()));
        }
        let v: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("解析掘金响应失败: {e}"))?;

        let mut out = Vec::new();
        if let Some(data) = v.get("data").and_then(|x| x.as_array()) {
            for (i, item) in data.iter().enumerate() {
                if out.len() >= max {
                    break;
                }
                let rm = item.get("result_model").unwrap_or(item);
                // 标题：文章/沸点/用户等类型字段不同，逐级回退
                let title = rm
                    .get("title")
                    .and_then(|x| x.as_str())
                    .or_else(|| rm.pointer("/article_info/title").and_then(|x| x.as_str()))
                    .unwrap_or("");
                let url = rm
                    .get("url")
                    .and_then(|x| x.as_str())
                    .or_else(|| rm.pointer("/article_info/article_url").and_then(|x| x.as_str()))
                    .unwrap_or("");
                if title.is_empty() || url.is_empty() {
                    continue;
                }
                let snippet = rm
                    .get("summary")
                    .and_then(|x| x.as_str())
                    .or_else(|| rm.pointer("/article_info/brief_content").and_then(|x| x.as_str()))
                    .unwrap_or("");
                // 摘要中可能带 HTML 标签，去掉
                let snippet = crate::engines::common::strip_html(snippet);
                let mut r = SearchResult::new(
                    clip(&title, 200),
                    url.to_string(),
                    clip(&snippet, 300),
                    "juejin",
                    i,
                );
                if let Some(ct) = rm
                    .pointer("/article_info/ctime")
                    .and_then(|x| x.as_i64())
                {
                    if let Some(dt) = unix_to_date(ct) {
                        r.published = Some(dt);
                    }
                }
                out.push(r);
            }
        }

        if out.is_empty() {
            Err("掘金无结果或接口变更".into())
        } else {
            Ok(out)
        }
    }
}

/// unix 秒 -> "YYYY-MM-DD"（无外部时间库，手写换算）。
fn unix_to_date(secs: i64) -> Option<String> {
    let days = secs.div_euclid(86400);
    // 1970-01-01 起的天数转日期（公历换算）
    let (y, m, d) = civil_from_days(days);
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

/// days（自 1970-01-01）转公历年月日（Howard Hinnant 算法）。
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}
