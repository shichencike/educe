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
            title: clean_title(&title),
            url,
            snippet,
            source: source.to_string(),
            rank,
            score: 1.0 / (rank as f64 + 1.0),
            published: None,
        }
    }
}

/// 已知的"站点后缀"标题模式（如 "xxx - 知乎" / "xxx_百度知道"）。
const TITLE_SUFFIXES: &[&str] = &[
    " - 知乎",
    " - 知乎专栏",
    " - 百度知道",
    " - 百度百科",
    " - CSDN博客",
    " - CSDN",
    " - 简书",
    " - 掘金",
    " - 博客园",
    " - 菜鸟教程",
    " - 廖雪峰的官方网站",
    " - SegmentFault 思否",
    " - 腾讯云开发者社区",
    " - 阿里云开发者社区",
    " - 哔哩哔哩",
    " - 少数派",
    " - 虎嗅网",
    " - 36氪",
    " - 豆瓣",
    " - V2EX",
    " - Gitee",
    " - GitHub",
    " - InfoQ",
    "_知乎",
    "_百度知道",
    "_简书",
    "_CSDN博客",
];

/// 清洗标题：去掉常见的"站点后缀"（如 "xxx - 知乎"），以及短的
/// ` | 站点` / ` - 站点` 尾缀（站点名 ≤ 8 字符、不含空格/标点时）。
pub fn clean_title(title: &str) -> String {
    let t = title.trim();
    if t.is_empty() {
        return t.to_string();
    }
    for suffix in TITLE_SUFFIXES {
        if t.ends_with(suffix) {
            return t[..t.len() - suffix.len()].trim_end().to_string();
        }
    }
    // 通用短尾缀：` | xxx` / ` - xxx` / `– xxx`，站点名短且无空格
    for sep in [" | ", " - ", " – ", " - ", "｜"] {
        if let Some(pos) = t.rfind(sep) {
            let site = &t[pos + sep.len()..];
            let site_len = site.chars().count();
            if (1..=8).contains(&site_len)
                && !site
                    .chars()
                    .any(|c| c.is_whitespace() || c == ',' || c == '.' || c == '、' || c == '，')
            {
                return t[..pos].trim_end().to_string();
            }
        }
    }
    t.to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_title_strips_known_site_suffix() {
        assert_eq!(clean_title("Rust 入门 - 知乎"), "Rust 入门");
        assert_eq!(clean_title("HashMap 详解_CSDN博客"), "HashMap 详解");
        assert_eq!(clean_title("无后缀标题"), "无后缀标题");
    }

    #[test]
    fn clean_title_strips_short_generic_suffix() {
        assert_eq!(clean_title("Rust 异步 | MDN"), "Rust 异步");
        // 含空格/标点的尾缀保留（可能是正文一部分）
        assert_eq!(clean_title("标题 - 很长的站点名称后缀超出八字符"), "标题 - 很长的站点名称后缀超出八字符");
    }

    #[test]
    fn clean_title_keeps_empty() {
        assert_eq!(clean_title(""), "");
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
    /// 是否经历过一次自动重试（网络类失败）
    #[serde(skip_serializing_if = "is_false")]
    pub retried: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
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
    /// 是否命中缓存（未命中/未启用时不序列化）
    #[serde(skip_serializing_if = "is_false")]
    pub cached: bool,
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
