//! 搜索源适配器：Engine trait、共享上下文、注册表。
//!
//! 新增一个搜索源 = 新建一个模块 + 实现 Engine + 在 `build` 中注册。
//! 各适配器通过 `EngineContext` 获取共享 HTTP 客户端与配置。

pub mod arxiv;
pub mod baidu;
pub mod bing;
pub mod common;
pub mod csdn;
pub mod duckduckgo;
pub mod gitee;
pub mod github;
pub mod google;
pub mod googlenews;
pub mod jianshu;
pub mod juejin;
pub mod pubmed;
pub mod so360;
pub mod sogou;
pub mod stackoverflow;
pub mod startpage;
pub mod wechat;
pub mod wikipedia;
pub mod xueshu;
pub mod zhihu;

use std::sync::Arc;

use async_trait::async_trait;

use crate::config::AppConfig;
use crate::http::HttpClient;
use crate::jsrender::JsRenderer;
use crate::models::{EngineMeta, SearchResult};

/// 引擎执行错误（分类用于重试决策与前端展示）。
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// 网络 / HTTP / 渲染类失败，可重试
    #[error("{0}")]
    Http(String),
    /// 单源超时，可重试
    #[error("单源超时")]
    Timeout,
    /// 反爬 / 验证码 / 登录墙 / 限流，重试无意义
    #[error("{0}")]
    Blocked(String),
    /// 解析失败 / 无结果，重试无意义
    #[error("{0}")]
    Parse(String),
}

impl EngineError {
    /// 是否值得自动重试一次（仅网络/超时类）。
    pub fn retryable(&self) -> bool {
        matches!(self, EngineError::Http(_) | EngineError::Timeout)
    }
}

/// 引擎共享上下文（每次聚合搜索时构造一次，各引擎并发使用）。
#[derive(Clone)]
pub struct EngineContext {
    pub http: HttpClient,
    /// JS 渲染桥（未启用则为 None）
    pub js_render: Option<Arc<JsRenderer>>,
}

/// 搜索源适配器接口。
#[async_trait]
pub trait Engine: Send + Sync {
    /// 引擎静态元信息。
    fn meta(&self) -> EngineMeta;
    /// 执行一次搜索，返回原始结果（未去重、未排序）。
    /// 失败时返回分类错误（调度器据此决定是否重试）。
    async fn search(
        &self,
        ctx: &EngineContext,
        query: &str,
        max: usize,
    ) -> Result<Vec<SearchResult>, EngineError>;
}

/// 全部引擎元信息（用于 /api/sources）。
pub fn all_metas() -> Vec<EngineMeta> {
    vec![
        baidu::META,
        bing::META,
        duckduckgo::META,
        sogou::META,
        so360::META,
        google::META,
        startpage::META,
        github::META,
        stackoverflow::META,
        gitee::META,
        juejin::META,
        arxiv::META,
        pubmed::META,
        wikipedia::META,
        googlenews::META,
        xueshu::META,
        zhihu::META,
        csdn::META,
        jianshu::META,
        wechat::META,
    ]
}

/// 按配置构建启用的引擎集合，并注册各自限速。
pub fn build(cfg: &AppConfig, http: HttpClient) -> Vec<Arc<dyn Engine>> {
    let enabled = if cfg.engines.enabled.is_empty() {
        None
    } else {
        Some(&cfg.engines.enabled)
    };
    let mut out: Vec<Arc<dyn Engine>> = Vec::new();
    let mut register = |e: Box<dyn Engine>| {
        let m = e.meta();
        let keep = match enabled {
            None => true,
            Some(list) => list.iter().any(|x| x.as_str() == m.id.as_ref()),
        };
        if keep {
            http.set_rate_limit(m.id.as_ref(), cfg.rate_limit_for(m.id.as_ref()));
            out.push(Arc::from(e));
        }
    };
    register(Box::new(baidu::Baidu));
    register(Box::new(bing::Bing));
    register(Box::new(duckduckgo::DuckDuckGo));
    register(Box::new(sogou::Sogou));
    register(Box::new(so360::So360));
    register(Box::new(google::Google));
    register(Box::new(startpage::Startpage));
    register(Box::new(github::GitHub));
    register(Box::new(stackoverflow::StackOverflow));
    register(Box::new(gitee::Gitee));
    register(Box::new(juejin::Juejin));
    register(Box::new(arxiv::Arxiv));
    register(Box::new(pubmed::PubMed));
    register(Box::new(wikipedia::Wikipedia));
    register(Box::new(googlenews::GoogleNews));
    register(Box::new(xueshu::Xueshu));
    register(Box::new(zhihu::Zhihu));
    register(Box::new(csdn::Csdn));
    register(Box::new(jianshu::Jianshu));
    register(Box::new(wechat::Wechat));
    out
}
