//! PubMed 搜索适配器（NCBI E-utilities：ESearch 取 PMID，ESummary 取详情）。
//! 无需鉴权，但请求需带 User-Agent 与联系方式（已在请求头中带 UA）。

use async_trait::async_trait;

use crate::engines::common::{clip, encode_query_pct};
use crate::engines::{Engine, EngineContext, EngineError, error_detail};
use std::borrow::Cow;

use crate::models::{Category, EngineMeta, SearchResult};

pub const META: EngineMeta = EngineMeta {
    id: Cow::Borrowed("pubmed"),
    name: Cow::Borrowed("PubMed"),
    category: Category::Academic,
    needs_js: false,
};

pub struct PubMed;

#[async_trait]
impl Engine for PubMed {
    fn meta(&self) -> EngineMeta {
        META
    }

    async fn search(
        &self,
        ctx: &EngineContext,
        query: &str,
        max: usize,
    ) -> Result<Vec<SearchResult>, EngineError> {
        let retmax = max.min(50);
        // 第一步：ESearch 拿到 PMID 列表
        let esearch = format!(
            "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esearch.fcgi?db=pubmed&term={}&retmax={}&retmode=json",
            encode_query_pct(query),
            retmax
        );
        let resp = ctx
            .http
            .get("pubmed", &esearch)
            .await
            .map_err(|e| EngineError::Http(error_detail(&e)))?;
        let v: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| EngineError::Parse(format!("解析 ESearch 失败: {e}")))?;
        let ids: Vec<&str> = v
            .pointer("/esearchresult/idlist")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(|i| i.as_str()).collect())
            .unwrap_or_default();
        if ids.is_empty() {
            return Err(EngineError::Parse("PubMed 无结果".into()));
        }

        // 第二步：ESummary 批量取详情
        let esummary = format!(
            "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esummary.fcgi?db=pubmed&id={}&retmode=json",
            ids.join(",")
        );
        let resp = ctx
            .http
            .get("pubmed", &esummary)
            .await
            .map_err(|e| EngineError::Http(error_detail(&e)))?;
        let v: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| EngineError::Parse(format!("解析 ESummary 失败: {e}")))?;

        let mut out = Vec::new();
        let mut rank = 0usize;
        for id in ids {
            if out.len() >= max {
                break;
            }
            let Some(rec) = v.get("result").and_then(|r| r.get(id)) else {
                continue;
            };
            let Some(title) = rec.get("title").and_then(|x| x.as_str()) else {
                continue;
            };
            let journal = rec
                .get("fulljournalname")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            let pubdate = rec.get("pubdate").and_then(|x| x.as_str()).unwrap_or("");
            let pages = rec
                .get("articleids")
                .and_then(|x| x.as_array())
                .and_then(|a| {
                    a.iter()
                        .find(|it| it.get("idtype").and_then(|t| t.as_str()) == Some("pubmed"))
                        .and_then(|it| it.get("value").and_then(|x| x.as_str()))
                })
                .unwrap_or("")
                .to_string();
            let url = if pages.is_empty() {
                format!("https://pubmed.ncbi.nlm.nih.gov/{id}/")
            } else {
                pages
            };
            let snippet = if journal.is_empty() {
                pubdate.to_string()
            } else {
                format!("{journal} · {pubdate}")
            };
            let mut r =
                SearchResult::new(clip(title, 200), url, clip(&snippet, 300), "pubmed", rank);
            if pubdate.len() >= 4 {
                r.published = Some(pubdate[..4].to_string()); // 年份
            }
            rank += 1;
            out.push(r);
        }

        if out.is_empty() {
            Err(EngineError::Parse("PubMed 详情解析失败".into()))
        } else {
            Ok(out)
        }
    }
}
