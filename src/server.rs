//! HTTP 服务：REST API + 内嵌单页前端。
//!
//! 路由：
//! - `GET  /`                 单页前端（include_str! 内嵌，零静态文件）
//! - `GET  /settings.html`    设置页（SearXNG 风格）
//! - `GET  /api/search`       聚合搜索（参数 q, sources, max, offset）
//! - `GET  /api/sources`      全部引擎元信息（含启用状态与权重）
//! - `GET/POST/DELETE /api/prefs`  用户偏好（cookie 持久化）
//! - `GET  /healthz`          健康检查

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};

use crate::config::AppConfig;
use crate::custom::CustomEngineConfig;
use crate::models::{SearchQuery, SearchResponse};
use crate::prefs::{
    extract_cookie, set_cookie_header, UserPrefs, PREFS_COOKIE, PREFS_MAX_AGE,
};
use crate::runtime::RuntimeSettings;
use crate::search::Aggregator;

/// 内嵌单页前端
const INDEX_HTML: &str = include_str!("web/index.html");
/// 内嵌设置页
const SETTINGS_HTML: &str = include_str!("web/settings.html");

#[derive(Clone)]
pub struct AppState {
    agg: Arc<Aggregator>,
    cfg: Arc<AppConfig>,
}

/// 启动 HTTP 服务（阻塞直到退出）。
pub async fn serve(cfg: Arc<AppConfig>) -> anyhow::Result<()> {
    let agg = Arc::new(Aggregator::new(cfg.clone())?);
    let state = AppState { agg, cfg };

    let addr = format!("{}:{}", state.cfg.server.host, state.cfg.server.port);

    let app = Router::new()
        .route("/", get(index))
        .route("/settings.html", get(settings))
        .route("/api/search", get(search_handler))
        .route("/api/sources", get(sources_handler))
        .route("/api/prefs", get(prefs_get).post(prefs_post).delete(prefs_delete))
        .route(
            "/api/engines/custom",
            get(custom_list).post(custom_add),
        )
        .route("/api/engines/custom/{id}", delete(custom_delete))
        .route("/api/runtime", get(runtime_get).post(runtime_post))
        .route("/healthz", get(healthz))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| anyhow::anyhow!("监听 {addr} 失败: {e}"))?;
    tracing::info!("Educe 已启动: http://{addr}");
    axum::serve(listener, app)
        .await
        .map_err(|e| anyhow::anyhow!("服务异常退出: {e}"))?;
    Ok(())
}

async fn index() -> impl IntoResponse {
    Html(INDEX_HTML)
}

async fn settings() -> impl IntoResponse {
    Html(SETTINGS_HTML)
}

async fn healthz() -> &'static str {
    "ok"
}

/// 读取当前生效偏好：cookie 优先，无 cookie 用配置默认值。
fn current_prefs(cfg: &AppConfig, headers: &HeaderMap) -> UserPrefs {
    let cookie_value = headers
        .get(COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|h| extract_cookie(Some(h), PREFS_COOKIE));
    match cookie_value.and_then(UserPrefs::from_cookie) {
        Some(p) => p,
        None => UserPrefs::defaults_from_config(cfg),
    }
}

async fn prefs_get(State(st): State<AppState>, headers: HeaderMap) -> Json<UserPrefs> {
    Json(current_prefs(&st.cfg, &headers))
}

async fn prefs_post(
    State(st): State<AppState>,
    headers: HeaderMap,
    body: Json<UserPrefs>,
) -> Response {
    let mut prefs = current_prefs(&st.cfg, &headers);
    prefs.merge_from(&body.0);
    let cookie = prefs.to_cookie();
    let mut hm = HeaderMap::new();
    hm.insert(
        SET_COOKIE,
        set_cookie_header(&cookie, PREFS_MAX_AGE).parse().unwrap(),
    );
    (hm, Json(prefs)).into_response()
}

async fn prefs_delete() -> Response {
    let mut hm = HeaderMap::new();
    hm.insert(SET_COOKIE, set_cookie_header("", 0).parse().unwrap());
    (hm, Json(serde_json::json!({"ok": true}))).into_response()
}

/// 自定义引擎列表（设置页编辑用）。
async fn custom_list(State(st): State<AppState>) -> Json<Vec<CustomEngineConfig>> {
    Json(st.agg.custom_engines())
}

/// 新增/更新自定义引擎；返回更新后的完整列表。
async fn custom_add(
    State(st): State<AppState>,
    Json(cfg): Json<CustomEngineConfig>,
) -> Response {
    match st.agg.add_custom_engine(&cfg) {
        Ok(list) => Json(list).into_response(),
        Err(msg) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": msg})),
        )
            .into_response(),
    }
}

/// 删除自定义引擎；返回更新后的完整列表。
async fn custom_delete(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    match st.agg.remove_custom_engine(&id) {
        Ok(list) => Json(list).into_response(),
        Err(msg) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": msg})),
        )
            .into_response(),
    }
}

/// 当前运行时设置（代理池 / JS 桥）。
async fn runtime_get(State(st): State<AppState>) -> Json<RuntimeSettings> {
    Json(st.agg.runtime_settings())
}

/// 更新运行时设置：重建 HTTP 客户端 / JS 渲染桥并持久化。
async fn runtime_post(
    State(st): State<AppState>,
    Json(rs): Json<RuntimeSettings>,
) -> Response {
    match st.agg.apply_runtime(&rs) {
        Ok(r) => Json(r).into_response(),
        Err(msg) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": msg})),
        )
            .into_response(),
    }
}

async fn sources_handler(State(st): State<AppState>, headers: HeaderMap) -> Json<Vec<crate::models::EngineInfo>> {
    let prefs = current_prefs(&st.cfg, &headers);
    Json(st.agg.source_infos_with_prefs(&prefs))
}

async fn search_handler(
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<SearchQuery>,
) -> Response {
    let query = q.q.trim().to_string();
    if query.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "参数 q 不能为空"})),
        )
            .into_response();
    }

    // 用户偏好（cookie）
    let prefs = current_prefs(&st.cfg, &headers);

    // 源白名单：查询参数优先，否则用偏好中启用的引擎（并补上全部自定义引擎）
    let sources: Option<Vec<String>> = match q.sources.as_ref() {
        Some(s) if !s.trim().is_empty() => Some(
            s.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect(),
        ),
        _ => {
            let mut ids = prefs.enabled_engine_ids();
            ids.extend(st.agg.custom_engine_ids());
            Some(ids)
        }
    };
    // 结果数：参数缺省用偏好
    let max = q.max.unwrap_or(prefs.results_per_page).clamp(1, 200);
    let offset = q.offset.unwrap_or(0);

    let resp: SearchResponse = st
        .agg
        .search(&query, sources.as_deref(), offset, max, Some(&prefs))
        .await;
    Json(resp).into_response()
}
