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
use tokio::sync::{mpsc, Semaphore};

use crate::config::AppConfig;
use crate::custom::{load_custom, save_custom, CustomEngine, CustomEngineConfig};
use crate::engines::common::{normalize_url, unredirect_url};
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
    /// 聚合结果 TTL 缓存
    cache: crate::cache::SearchCache,
}

/// 网络类失败自动重试前的退避时长。
const RETRY_BACKOFF: Duration = Duration::from_millis(300);

/// 搜索建议去重（保序）并截断到 10 条。
fn dedup_suggestions(items: Vec<String>) -> Vec<String> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    items
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && seen.insert(s.clone()))
        .take(10)
        .collect()
}

/// 单引擎执行结果（报告 + 结果列表）。
struct EngineOutcome {
    report: EngineReport,
    results: Vec<SearchResult>,
}

/// SSE 流式事件：每源完成后推送 `Engine`，全部完成后推送 `Done`（含最终排序结果）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    Engine {
        id: String,
        count: usize,
        time_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        #[serde(skip_serializing_if = "is_false")]
        retried: bool,
        results: Vec<SearchResult>,
    },
    Done {
        total: usize,
        time_ms: u64,
        results: Vec<SearchResult>,
    },
}

fn is_false(b: &bool) -> bool {
    !*b
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

        // 缓存：按配置启用与否构造（cfg 之后会被移入结构体，先算好）
        let cache = crate::cache::SearchCache::new(
            if cfg.cache.enabled {
                Duration::from_secs(cfg.cache.ttl_seconds)
            } else {
                Duration::ZERO
            },
            cfg.cache.max_entries,
        );

        Ok(Aggregator {
            engines: Mutex::new(engines),
            custom_configs: Mutex::new(custom_configs),
            cfg,
            http: RwLock::new(http),
            js_render: RwLock::new(js_render),
            runtime: RwLock::new(runtime),
            cache,
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

    /// 搜索建议（自动补全）：优先 DuckDuckGo 建议接口，失败/为空时回退百度建议。
    /// 返回去重后的建议词（最多 10 条）。
    pub async fn suggest(&self, query: &str) -> Vec<String> {
        let q = query.trim();
        if q.is_empty() {
            return Vec::new();
        }
        let http = self.http.read().unwrap().clone();

        // 1) DuckDuckGo autocomplete（JSON）
        let ddg_url = format!(
            "https://duckduckgo.com/ac/?q={}&type=list",
            crate::engines::common::encode_query_pct(q)
        );
        if let Ok(resp) = http.get("duckduckgo", &ddg_url).await {
            if resp.status().is_success() {
                if let Ok(v) = resp.json::<serde_json::Value>().await {
                    let mut items: Vec<String> = Vec::new();
                    // 数组格式: ["q", ["s1","s2"]]
                    if let Some(arr) = v.as_array() {
                        if let Some(list) = arr.get(1).and_then(|x| x.as_array()) {
                            items.extend(list.iter().filter_map(|x| x.as_str().map(String::from)));
                        }
                    }
                    // 对象格式: {"suggestions": [...]}
                    if items.is_empty() {
                        if let Some(list) = v.get("suggestions").and_then(|x| x.as_array()) {
                            items.extend(list.iter().filter_map(|x| x.as_str().map(String::from)));
                        }
                    }
                    if !items.is_empty() {
                        return dedup_suggestions(items);
                    }
                }
            }
        }

        // 2) 百度建议（JSONP，取 {q,s} 对象）
        let bd_url = format!(
            "https://suggestion.baidu.com/su?wd={}&cb=educe",
            crate::engines::common::encode_query_pct(q)
        );
        if let Ok(text) = http.get_text("baidu", &bd_url).await {
            if let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text[start..=end]) {
                    if let Some(list) = v.get("s").and_then(|x| x.as_array()) {
                        let items: Vec<String> = list
                            .iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect();
                        if !items.is_empty() {
                            return dedup_suggestions(items);
                        }
                    }
                }
            }
        }
        Vec::new()
    }

    /// 执行一次聚合搜索（非流式，一次性返回全部结果）。
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
        self.search_impl(query, sources, offset, max, prefs, None)
            .await
    }

    /// 流式聚合搜索：每源完成后经 `tx` 推送 `StreamEvent::Engine`，
    /// 全部完成（含最终排序分页）后推送 `StreamEvent::Done`。返回与 `search` 相同的完整响应。
    pub async fn search_stream(
        &self,
        query: &str,
        sources: Option<&[String]>,
        offset: usize,
        max: usize,
        prefs: Option<&UserPrefs>,
        tx: mpsc::Sender<StreamEvent>,
    ) -> SearchResponse {
        self.search_impl(query, sources, offset, max, prefs, Some(tx))
            .await
    }

    async fn search_impl(
        &self,
        query: &str,
        sources: Option<&[String]>,
        offset: usize,
        max: usize,
        prefs: Option<&UserPrefs>,
        tx: Option<mpsc::Sender<StreamEvent>>,
    ) -> SearchResponse {
        let started = Instant::now();

        // 缓存键：查询 + 源白名单（排序后）。有权重覆盖时不缓存（排序依赖偏好）。
        let cache_key = self.cache_key(query, sources);
        if let Some(key) = &cache_key {
            if let Some((base, total, cached)) = self.cache.get(key) {
                // 请求的 [offset, offset+max) 落在缓存窗口内才命中，否则重新聚合
                if offset >= base && offset.saturating_add(max) <= base + cached.len() {
                    let start = offset - base;
                    let page: Vec<SearchResult> = cached[start..start + max].to_vec();
                    let time_ms = started.elapsed().as_millis() as u64;
                    tracing::info!(
                        query = %query,
                        total,
                        time_ms,
                        engines = 0,
                        "aggregate search cache hit"
                    );
                    // 流式模式：缓存命中直接推送 Done
                    if let Some(tx) = &tx {
                        let _ = tx
                            .send(StreamEvent::Done {
                                total,
                                time_ms,
                                results: page.clone(),
                            })
                            .await;
                    }
                    return SearchResponse {
                        query: query.to_string(),
                        total,
                        time_ms,
                        results: page,
                        engines: Vec::new(),
                        cached: true,
                    };
                }
            }
        }

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
            // 流式：每源完成立即推送（前端渐进渲染）
            if let Some(tx) = &tx {
                let ev = StreamEvent::Engine {
                    id: outcome.report.id.clone(),
                    count: outcome.report.count,
                    time_ms: outcome.report.time_ms,
                    error: outcome.report.error.clone(),
                    retried: outcome.report.retried,
                    results: outcome.results.clone(),
                };
                // 发送失败（客户端断开）时忽略，继续聚合
                let _ = tx.send(ev).await;
            }
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
        // 写入缓存：只保存当前页 ±1 页的窗口（省内存），分页命中按窗口切片；
        // 用户权重覆盖时跳过（排序依赖偏好）
        if let Some(key) = cache_key {
            if !weight_overrides.as_ref().is_some_and(|m| !m.is_empty()) {
                let win_base = offset.saturating_sub(max); // 上一页起
                let win_end = (offset + 2 * max).min(total); // 下一页止
                self.cache
                    .set(key, win_base, total, ranked[win_base..win_end].to_vec());
            }
        }
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

        // 流式：全部完成，推送最终排序分页结果
        if let Some(tx) = &tx {
            let ev = StreamEvent::Done {
                total,
                time_ms: started.elapsed().as_millis() as u64,
                results: page.clone(),
            };
            let _ = tx.send(ev).await;
        }

        SearchResponse {
            query: query.to_string(),
            total,
            time_ms: started.elapsed().as_millis() as u64,
            results: page,
            engines: reports,
            cached: false,
        }
    }

    /// 构造缓存键：查询小写 + 源白名单（排序去重后）。空源 = "all"。
    fn cache_key(&self, query: &str, sources: Option<&[String]>) -> Option<String> {
        if !self.cache.enabled() {
            return None;
        }
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return None;
        }
        let src: Vec<String> = match sources {
            Some(list) if !list.is_empty() => {
                let mut v: Vec<String> = list.iter().map(|s| s.to_lowercase()).collect();
                v.sort();
                v.dedup();
                v
            }
            _ => vec!["all".into()],
        };
        Some(format!("{}|{}", q, src.join(",")))
    }
}

/// 跨源去重合并 + 评分。
/// 合并规则：URL 规范化（跳转链还原 + 去跟踪参数/片段）后相同的归为一条；
/// 合并时来源并列、标题取长、摘要取最长、评分叠加。
fn dedup_and_rank(
    all: Vec<SearchResult>,
    cfg: &AppConfig,
    query: &str,
    weight_overrides: Option<&HashMap<String, f64>>,
) -> Vec<SearchResult> {
    let mut merged: Vec<SearchResult> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();

    for r in all {
        // 跳转链还原后再规范化，让不同引擎对同一目标的跳转 URL 能合并；
        // 合并后的 URL 直接使用还原后的真实目标（前端点击直达）。
        let real = unredirect_url(&r.url);
        let key = normalize_url(&real);
        if let Some(&i) = index.get(&key) {
            let m = &mut merged[i];
            if !m.source.split(',').any(|s| s == r.source) {
                m.source.push(',');
                m.source.push_str(&r.source);
            }
            if r.title.chars().count() > m.title.chars().count() {
                m.title = r.title;
            }
            // 摘要取非空且更长的一条（信息量更大）
            if r.snippet.chars().count() > m.snippet.chars().count() {
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
            let mut item = r;
            item.url = real; // 展示还原后的真实 URL
            let idx = merged.len();
            merged.push(item);
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
    let mut v: Vec<SearchResult> = all
        .into_iter()
        .map(|mut r| {
            r.url = unredirect_url(&r.url); // 展示还原后的真实 URL
            r
        })
        .collect();
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
        // 标题/摘要命中关键词加成：词边界匹配（英文），中文子串匹配
        if !words.is_empty() {
            let title_lower = r.title.to_lowercase();
            let snip_lower = r.snippet.to_lowercase();
            for w in &words {
                let hit_title = if w.chars().all(|c| c.is_ascii_alphanumeric()) {
                    title_lower
                        .split(|c: char| !c.is_ascii_alphanumeric())
                        .any(|t| t == w.as_str())
                } else {
                    title_lower.contains(w.as_str())
                };
                let hit_snip = if w.chars().all(|c| c.is_ascii_alphanumeric()) {
                    snip_lower
                        .split(|c: char| !c.is_ascii_alphanumeric())
                        .any(|t| t == w.as_str())
                } else {
                    snip_lower.contains(w.as_str())
                };
                if hit_title {
                    score += 0.18;
                } else if hit_snip {
                    score += 0.08;
                }
            }
        }
        r.score = score;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
