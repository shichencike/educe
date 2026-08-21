//! 核心数据模型：搜索结果、引擎元信息、API 请求/响应结构。

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

/// 单个搜索结果（来自某个引擎的原始条目）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// 标题
    pub title: String,
    /// 链接
    pub url: String,
    /// 摘要
    pub snippet: String,
    /// 来源引擎 id（如 "bing"）
    pub source: String,
    /// 在该引擎内的原始排名（0 起）
    pub rank: usize,
    /// 基础得分（引擎内相对分数，0.0~1.0）
    pub score: f64,
    /// 发布时间（可选，字符串形式，如 "2026-08-01"）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
}

impl SearchResult {
    pub fn new(title: String, url: String, snippet: String, source: &str, rank: usize) -> Self {
        SearchResult {
            title,
            url,
            snippet,
            source: source.to_string(),
            rank,
            score: 1.0 / (rank as f64 + 1.0),
            published: None,
        }
    }
}

/// 引擎分类（用于 API 展示与前端分组）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// 通用搜索引擎
    General,
    /// 代码 / 技术社区
    Code,
    /// 中文内容社区
    Chinese,
    /// 学术 / 百科 / 新闻
    Academic,
}

impl Category {
    pub fn as_str(&self) -> &'static str {
        match self {
            Category::General => "general",
            Category::Code => "code",
            Category::Chinese => "chinese",
            Category::Academic => "academic",
        }
    }
}

/// 引擎元信息。id/name 用 Cow：内置引擎为静态借用，自定义引擎为动态拥有。
#[derive(Debug, Clone)]
pub struct EngineMeta {
    /// 引擎 id（URL 参数、配置中引用）
    pub id: Cow<'static, str>,
    /// 中文显示名
    pub name: Cow<'static, str>,
    /// 分类
    pub category: Category,
    /// 是否需要 JS 渲染桥
    pub needs_js: bool,
}

/// 单源执行报告（聚合响应中返回给前端展示每源状态）。
#[derive(Debug, Clone, Serialize)]
pub struct EngineReport {
    pub id: String,
    pub count: usize,
    pub time_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// GET /api/search 查询参数。
#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    /// 搜索关键词（必填）
    pub q: String,
    /// 逗号分隔的引擎 id 白名单；不填则用配置
    pub sources: Option<String>,
    /// 返回结果上限
    pub max: Option<usize>,
    /// 跳过前 N 条（分页）
    pub offset: Option<usize>,
}

/// GET /api/search 响应。
#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub total: usize,
    pub time_ms: u64,
    pub results: Vec<SearchResult>,
    /// 每源执行情况
    pub engines: Vec<EngineReport>,
}

/// GET /api/sources 中单个引擎的描述。
#[derive(Debug, Serialize)]
pub struct EngineInfo {
    pub id: String,
    pub name: String,
    pub category: String,
    pub needs_js: bool,
    pub enabled: bool,
    pub weight: f64,
}
