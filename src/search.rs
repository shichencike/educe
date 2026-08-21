//! 聚合搜索编排：并发调度各引擎、单源超时与故障隔离、
//! 跨源去重合并、加权评分排序、分页。
//!
//! 调度语义：
//! - 每个引擎独立并发执行，互不阻塞；
//! - 单源超时/失败只影响该源（错误记录进 `engines` 报告，其余源正常返回）；
//! - 同一 URL 跨源出现时合并为一条，来源并列展示，评分叠加；
//! - 引擎集合（内置 + 自定义）、代理池、JS 渲染桥支持运行时动态变更（设置页）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use futures::stream::{FuturesUnordered, StreamExt};
use tokio::sync::Semaphore;

use crate::config::AppConfig;
use crate::custom::{load_custom, save_custom, CustomEngine, CustomEngineConfig};
use crate::engines::common::normalize_url;
use crate::engines::{all_metas, build, Engine, EngineContext, EngineError};
use crate::http::HttpClient;
use crate::jsrender::JsRenderer;
use crate::models::{EngineInfo, EngineReport, SearchResponse, SearchResult};
use crate::prefs::UserPrefs;
use crate::runtime::RuntimeSettings;

/// 聚合器：持有引擎集合与共享资源，负责一次完整聚合搜索。
pub struct Aggregator {
    /// 引擎集合（内置 + 自定义），支持运行时增删
    engines: Mutex<Vec<Arc<dyn Engine>>>,
    /// 自定义引擎配置（与 engines 同步），供 API 展示与持久化
    custom_configs: Mutex<Vec<CustomEngineConfig>>,
    cfg: Arc<AppConfig>,
    /// HTTP 客户端（代理池变更时重建）
    http: RwLock<HttpClient>,
    /// JS 渲染桥（配置变更时重建）
    js_render: RwLock<Option<Arc<JsRenderer>>>,
    /// 运行时设置（代理池 / JS 桥）
    runtime: RwLock<RuntimeSettings>,
}

/// 网络类失败自动重试前的退避时长。
const RETRY_BACKOFF: Duration = Duration::from_millis(300);

/// 单引擎执行结果（报告 + 结果列表）。
struct EngineOutcome {
    report: EngineReport,
    results: Vec<SearchResult>,
}

/// 执行单个引擎搜索（带超时），超时归一为 EngineError::Timeout。
async fn run_engine(
    e: &Arc<dyn Engine>,
    ctx: &EngineContext,
    query: &str,
    max: usize,
    timeout: Duration,
) -> Result<Vec<SearchResult>, EngineError> {
    tokio::time::timeout(timeout, e.search(ctx, query, max))
        .await
        .map_err(|_| EngineError::Timeout)?
}

impl Aggregator {
    pub fn new(cfg: Arc<AppConfig>) -> anyhow::Result<Self> {
        // 运行时设置（runtime.toml）优先于静态配置（代理池 / JS 桥）
        let runtime =
            RuntimeSettings::load_file().unwrap_or_else(|| RuntimeSettings::from_cfg(&cfg));
        let http = HttpClient::new(&runtime.to_proxy_config())?;
        let js_render = JsRenderer::from_config(&runtime.to_js_config());

        let mut engines = build(&cfg, http.clone());
        // 加载持久化的自定义引擎
        let mut custom_configs = Vec::new();
        for config in load_custom(None) {
            match CustomEngine::new(config.clone()) {
                Ok(e) => {
                    http.set_rate_limit(&config.id, cfg.rate_limit_for(&config.id));
                    engines.push(Arc::new(e));
                    custom_configs.push(config);
                }
                Err(err) => tracing::warn!("自定义引擎 `{}` 配置无效，已跳过: {err}", config.id),
            }
        }

        Ok(Aggregator {
            engines: Mutex::new(engines),
            custom_configs: Mutex::new(custom_configs),
            cfg,
            http: RwLock::new(http),
            js_render: RwLock::new(js_render),
            runtime: RwLock::new(runtime),
        })
    }

    /// 全部引擎信息（内置 + 自定义），供 /api/sources。
    pub fn source_infos(&self) -> Vec<EngineInfo> {
        self.engine_infos(None)
    }

    /// 引擎信息，但启用状态与权重按用户偏好覆盖（设置页用）。
    pub fn source_infos_with_prefs(&self, prefs: &UserPrefs) -> Vec<EngineInfo> {
        self.engine_infos(Some(prefs))
    }

    fn engine_infos(&self, prefs: Option<&UserPrefs>) -> Vec<EngineInfo> {
        let engines = self.engines.lock().unwrap().clone();
        engines
            .iter()
            .map(|e| {
                let m = e.meta();
                let p = prefs.and_then(|x| x.engines.get(m.id.as_ref()));
                EngineInfo {
                    id: m.id.to_string(),
                    name: m.name.to_string(),
                    category: m.category.as_str().to_string(),
                    needs_js: m.needs_js,
                    enabled: p.map(|x| x.enabled).unwrap_or(true),
                    weight: p
                        .and_then(|x| x.weight)
                        .unwrap_or_else(|| self.cfg.weight_for(m.id.as_ref())),
                }
            })
            .collect()
    }

    /// 自定义引擎配置列表（API 展示）。
    pub fn custom_engines(&self) -> Vec<CustomEngineConfig> {
        self.custom_configs.lock().unwrap().clone()
    }

    /// 自定义引擎 id 列表（源白名单补全用）。
    pub fn custom_engine_ids(&self) -> Vec<String> {
        self.custom_configs
            .lock()
            .unwrap()
            .iter()
            .map(|c| c.id.clone())
            .collect()
    }

    /// 新增/更新自定义引擎并持久化；返回更新后的完整列表。
    pub fn add_custom_engine(
        &self,
        config: &CustomEngineConfig,
    ) -> Result<Vec<CustomEngineConfig>, String> {
        config.validate()?;
        if all_metas().iter().any(|m| m.id.as_ref() == config.id) {
            return Err(format!("id `{}` 与内置引擎冲突", config.id));
        }
        let engine = CustomEngine::new(config.clone())?;

        // 锁序：先 custom_configs 后 engines，避免死锁
        let mut configs = self.custom_configs.lock().unwrap();
        let mut engines = self.engines.lock().unwrap();
        if let Some(idx) = engines
            .iter()
            .position(|e| e.meta().id.as_ref() == config.id)
        {
            engines[idx] = Arc::new(engine); // 覆盖同 id
        } else {
            self.http
                .read()
                .unwrap()
                .set_rate_limit(&config.id, self.cfg.rate_limit_for(&config.id));
            engines.push(Arc::new(engine));
        }
        if let Some(idx) = configs.iter().position(|c| c.id == config.id) {
            configs[idx] = config.clone();
        } else {
            configs.push(config.clone());
        }
        save_custom(configs.as_slice(), None)?;
        Ok(configs.clone())
    }

    /// 删除自定义引擎并持久化；返回更新后的完整列表。
    pub fn remove_custom_engine(&self, id: &str) -> Result<Vec<CustomEngineConfig>, String> {
        let mut configs = self.custom_configs.lock().unwrap();
        if !configs.iter().any(|c| c.id == id) {
            return Err(format!("自定义引擎 `{id}` 不存在"));
        }
        let mut engines = self.engines.lock().unwrap();
        engines.retain(|e| e.meta().id.as_ref() != id);
        configs.retain(|c| c.id != id);
        save_custom(configs.as_slice(), None)?;
        Ok(configs.clone())
    }

    /// 当前运行时设置（代理池 / JS 桥）。
    pub fn runtime_settings(&self) -> RuntimeSettings {
        self.runtime.read().unwrap().clone()
    }

    /// 应用运行时设置：重建 HTTP 客户端与 JS 渲染桥，持久化 runtime.toml。
    pub fn apply_runtime(&self, rs: &RuntimeSettings) -> Result<RuntimeSettings, String> {
        // 先构建新组件，失败则保持现状不变
        let http = HttpClient::new(&rs.to_proxy_config()).map_err(|e| e.to_string())?;
        let js = JsRenderer::from_config(&rs.to_js_config());
        {
            // 重建后重新注册各引擎限速
            let engines = self.engines.lock().unwrap();
            for e in engines.iter() {
                let id = e.meta().id.as_ref().to_string();
                http.set_rate_limit(&id, self.cfg.rate_limit_for(&id));
            }
        }
        *self.http.write().unwrap() = http;
        *self.js_render.write().unwrap() = js;
        *self.runtime.write().unwrap() = rs.clone();
        rs.save_file()?;
        Ok(rs.clone())
    }

    /// 执行一次聚合搜索。
    /// `sources`：引擎白名单（None = 全部启用引擎）。
    /// `offset`/`max`：分页。`prefs`：用户偏好（覆盖超时与权重）。
    pub async fn search(
        &self,
        query: &str,
        sources: Option<&[String]>,
        offset: usize,
        max: usize,
        prefs: Option<&UserPrefs>,
    ) -> SearchResponse {
        let started = Instant::now();
        let ctx = EngineContext {
            http: self.http.read().unwrap().clone(),
            js_render: self.js_render.read().unwrap().clone(),
        };

        // 选择引擎（白名单过滤）
        let snapshot = self.engines.lock().unwrap().clone();
        let selected: Vec<Arc<dyn Engine>> = snapshot
            .iter()
            .filter(|e| {
                sources.is_none_or(|list| list.iter().any(|s| s.as_str() == e.meta().id.as_ref()))
            })
            .cloned()
            .collect();

        let max_per_source = self.cfg.search.max_per_source;
        // 单源超时：用户偏好覆盖配置文件
        let timeout_ms = prefs
            .map(|p| p.effective_timeout(&self.cfg))
            .unwrap_or(self.cfg.search.timeout_ms);
        let timeout = Duration::from_millis(timeout_ms);
        // 权重覆盖：用户偏好
        let weight_overrides = prefs.map(|p| p.weight_overrides());

        // 并发执行全部引擎（Semaphore 限流，超出的源排队等待）
        let max_concurrent = self.cfg.search.max_concurrent.clamp(1, 32);
        let semaphore = Arc::new(Semaphore::new(max_concurrent));
        tracing::debug!(
            query = %query,
            engines = selected.len(),
            max_concurrent,
            "aggregate search start"
        );

        let mut tasks = FuturesUnordered::new();
        for e in &selected {
            let e = e.clone();
            let ctx = ctx.clone();
            let q = query.to_string();
            let sem = semaphore.clone();
            tasks.push(async move {
                let _permit = sem.acquire_owned().await.expect("semaphore closed");
                let id = e.meta().id.to_string();
                let t0 = Instant::now();
                // 首次执行；网络/超时类失败自动重试一次（带退避）
                let mut outcome = run_engine(&e, &ctx, &q, max_per_source, timeout).await;
                let mut retried = false;
                if let Err(err) = &outcome {
                    if err.retryable() {
                        tracing::debug!(engine = %id, error = %err, "engine failed, retrying once");
                        tokio::time::sleep(RETRY_BACKOFF).await;
                        outcome = run_engine(&e, &ctx, &q, max_per_source, timeout).await;
                        retried = true;
                    }
                }
                let time_ms = t0.elapsed().as_millis() as u64;
                match outcome {
                    Ok(results) => {
                        tracing::debug!(
                            engine = %id,
                            count = results.len(),
                            time_ms,
                            retried,
                            "engine ok"
                        );
                        EngineOutcome {
                            report: EngineReport {
                                id,
                                count: results.len(),
                                time_ms,
                                error: None,
                                retried,
                            },
                            results,
                        }
                    }
                    Err(err) => {
                        tracing::debug!(
                            engine = %id,
                            error = %err,
                            time_ms,
                            retried,
                            "engine failed"
                        );
                        EngineOutcome {
                            report: EngineReport {
                                id,
                                count: 0,
                                time_ms,
                                error: Some(err.to_string()),
                                retried,
                            },
                            results: Vec::new(),
                        }
                    }
                }
            });
        }

        let mut reports = Vec::with_capacity(selected.len());
        let mut all: Vec<SearchResult> = Vec::new();
        while let Some(outcome) = tasks.next().await {
            reports.push(outcome.report);
            all.extend(outcome.results);
        }
        // 按引擎 id 排序报告，输出稳定
        reports.sort_by(|a, b| a.id.cmp(&b.id));

        // 去重合并 + 评分排序
        let ranked = if self.cfg.search.dedup {
            dedup_and_rank(all, &self.cfg, query, weight_overrides.as_ref())
        } else {
            rank_all(all, &self.cfg, query, weight_overrides.as_ref())
        };
        let total = ranked.len();
        let page: Vec<SearchResult> = ranked.into_iter().skip(offset).take(max).collect();

        let engines_ok = reports.iter().filter(|r| r.error.is_none()).count();
        tracing::info!(
            query = %query,
            total,
            time_ms = started.elapsed().as_millis() as u64,
            engines_ok,
            engines_failed = reports.len() - engines_ok,
            "aggregate search completed"
        );

        SearchResponse {
            query: query.to_string(),
            total,
            time_ms: started.elapsed().as_millis() as u64,
            results: page,
            engines: reports,
        }
    }
}

/// 跨源去重合并 + 评分。
/// 合并规则：URL 规范化（去跟踪参数/片段/尾斜杠）后相同的归为一条；
/// 合并时来源并列、标题取长、摘要取非空、评分叠加。
fn dedup_and_rank(
    all: Vec<SearchResult>,
    cfg: &AppConfig,
    query: &str,
    weight_overrides: Option<&HashMap<String, f64>>,
) -> Vec<SearchResult> {
    let mut merged: Vec<SearchResult> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();

    for r in all {
        let key = normalize_url(&r.url);
        if let Some(&i) = index.get(&key) {
            let m = &mut merged[i];
            if !m.source.split(',').any(|s| s == r.source) {
                m.source.push(',');
                m.source.push_str(&r.source);
            }
            if r.title.chars().count() > m.title.chars().count() {
                m.title = r.title;
            }
            if m.snippet.is_empty() && !r.snippet.is_empty() {
                m.snippet = r.snippet;
            }
            if r.rank < m.rank {
                m.rank = r.rank;
            }
            if m.published.is_none() {
                m.published = r.published;
            }
            m.score += r.score;
        } else {
            let idx = merged.len();
            merged.push(r);
            index.insert(key, idx);
        }
    }

    apply_scores(&mut merged, cfg, query, weight_overrides);
    merged.sort_unstable_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    merged
}

/// 不去重，仅评分排序。
fn rank_all(
    all: Vec<SearchResult>,
    cfg: &AppConfig,
    query: &str,
    weight_overrides: Option<&HashMap<String, f64>>,
) -> Vec<SearchResult> {
    let mut v = all;
    apply_scores(&mut v, cfg, query, weight_overrides);
    v.sort_unstable_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    v
}

/// 评分：`权重 * 位置因子`（多源合并时叠加）+ 标题命中关键词加成。
/// 权重优先取用户偏好覆盖，其次配置文件。
fn apply_scores(
    results: &mut [SearchResult],
    cfg: &AppConfig,
    query: &str,
    weight_overrides: Option<&HashMap<String, f64>>,
) {
    let words: Vec<String> = query
        .split_whitespace()
        .map(|w| w.to_lowercase())
        .filter(|w| w.chars().count() >= 2)
        .collect();

    for r in results.iter_mut() {
        // 位置因子：排名越靠前得分越高
        let pos = 1.0 / (1.0 + r.rank as f64 * 0.15);
        let src = r.source.split(',').next().unwrap_or("");
        let weight = weight_overrides
            .and_then(|o| o.get(src))
            .copied()
            .unwrap_or_else(|| cfg.weight_for(src));
        let mut score = weight * pos * r.score.max(0.05);
        // 标题命中关键词加成
        if !words.is_empty() {
            let title_lower = r.title.to_lowercase();
            let hits = words
                .iter()
                .filter(|w| title_lower.contains(w.as_str()))
                .count();
            score += hits as f64 * 0.15;
        }
        r.score = score;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Category;

    fn result(title: &str, url: &str, source: &str, rank: usize) -> SearchResult {
        SearchResult::new(title.into(), url.into(), String::new(), source, rank)
    }

    #[test]
    fn dedup_merges_same_url_across_sources() {
        let all = vec![
            result("Rust", "https://rust-lang.org/", "bing", 0),
            result(
                "Rust lang",
                "https://rust-lang.org/?utm_source=1",
                "baidu",
                1,
            ),
        ];
        let merged = dedup_and_rank(all, &AppConfig::default(), "rust", None);
        assert_eq!(merged.len(), 1);
        assert!(merged[0].source.contains("bing"));
        assert!(merged[0].source.contains("baidu"));
        assert_eq!(merged[0].title, "Rust lang"); // 标题取长
    }

    #[test]
    fn keyword_hit_boosts_ranking() {
        let all = vec![
            result("rust async book", "https://a.com/rust-async", "bing", 0),
            result("Something else", "https://b.com/other", "bing", 0),
        ];
        let ranked = rank_all(all, &AppConfig::default(), "rust", None);
        assert_eq!(ranked[0].url, "https://a.com/rust-async");
        assert!(ranked[0].score > ranked[1].score);
    }

    #[test]
    fn weight_override_changes_order() {
        let all = vec![
            result("same", "https://a.com/x", "bing", 0),
            result("same", "https://b.com/y", "github", 0),
        ];
        let mut overrides = HashMap::new();
        overrides.insert("github".to_string(), 5.0);
        let ranked = rank_all(all, &AppConfig::default(), "", Some(&overrides));
        assert_eq!(ranked[0].url, "https://b.com/y"); // github 权重 5.0 胜出
    }
}
