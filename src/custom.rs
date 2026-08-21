//! 自定义搜索源（设置页可动态添加，无需改代码）。
//!
//! 用户提供：URL 模板（`{query}` 占位符会被 URL 编码替换）+ 结果容器/标题/链接/摘要
//! 的 CSS 选择器；专用选择器解析失败时自动退回通用兜底提取。
//! 配置持久化在 `custom_engines.json`（工作目录）。

use std::borrow::Cow;
use std::path::Path;

use async_trait::async_trait;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};

use crate::engines::common::{absolute_url, clean_text, clip, encode_query_pct, generic_extract};
use crate::engines::{Engine, EngineContext, EngineError};
use crate::models::{Category, EngineMeta, SearchResult};

/// 自定义引擎配置文件（默认工作目录下）
pub const CUSTOM_FILE: &str = "custom_engines.json";

/// 自定义引擎配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomEngineConfig {
    /// 引擎 id：2~32 位字母/数字/下划线，不得与内置引擎冲突
    pub id: String,
    /// 显示名
    pub name: String,
    /// 分类 general | code | chinese | academic
    pub category: String,
    /// 是否走 JS 渲染桥
    pub needs_js: bool,
    /// 搜索 URL 模板，`{query}` 会被 URL 编码后替换
    pub url_template: String,
    /// 结果容器 CSS 选择器
    pub result_selector: String,
    /// 标题元素 CSS 选择器（相对结果容器）
    pub title_selector: String,
    /// 链接元素 CSS 选择器（取 href 属性）
    pub link_selector: String,
    /// 摘要元素 CSS 选择器（可留空）
    pub snippet_selector: String,
}

impl Default for CustomEngineConfig {
    fn default() -> Self {
        CustomEngineConfig {
            id: String::new(),
            name: String::new(),
            category: "general".into(),
            needs_js: false,
            url_template: String::new(),
            result_selector: String::new(),
            title_selector: String::new(),
            link_selector: String::new(),
            snippet_selector: String::new(),
        }
    }
}

impl CustomEngineConfig {
    /// 校验配置；返回人类可读的错误信息。
    pub fn validate(&self) -> Result<(), String> {
        if self.id.len() < 2
            || self.id.len() > 32
            || !self
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err("id 需为 2~32 位字母/数字/下划线".into());
        }
        if self.name.trim().is_empty() {
            return Err("名称不能为空".into());
        }
        if !self.url_template.contains("{query}") {
            return Err("URL 模板必须包含 {query} 占位符".into());
        }
        if self.result_selector.trim().is_empty() || self.title_selector.trim().is_empty() {
            return Err("结果容器与标题选择器不能为空".into());
        }
        Selector::parse(&self.result_selector)
            .map_err(|_| format!("结果选择器不是合法 CSS 选择器: {}", self.result_selector))?;
        Ok(())
    }

    pub fn category(&self) -> Category {
        match self.category.as_str() {
            "code" => Category::Code,
            "chinese" => Category::Chinese,
            "academic" => Category::Academic,
            _ => Category::General,
        }
    }
}

/// 自定义引擎的 Engine 实现。
pub struct CustomEngine {
    pub config: CustomEngineConfig,
}

impl CustomEngine {
    pub fn new(config: CustomEngineConfig) -> Result<Self, String> {
        config.validate()?;
        Ok(CustomEngine { config })
    }

    /// 用配置的选择器解析页面。
    fn parse_html(&self, html: &str, base_url: &str, max: usize) -> Vec<SearchResult> {
        let Ok(result_sel) = Selector::parse(&self.config.result_selector) else {
            return Vec::new();
        };
        let Ok(title_sel) = Selector::parse(&self.config.title_selector) else {
            return Vec::new();
        };
        let Ok(link_sel) = Selector::parse(&self.config.link_selector) else {
            return Vec::new();
        };
        let snip_sel = if self.config.snippet_selector.trim().is_empty() {
            None
        } else {
            Selector::parse(&self.config.snippet_selector).ok()
        };

        let doc = Html::parse_document(html);
        let mut out = Vec::new();
        for el in doc.select(&result_sel) {
            if out.len() >= max {
                break;
            }
            let Some(t) = el.select(&title_sel).next() else {
                continue;
            };
            let title = clean_text(&t.text().collect::<String>());
            let Some(l) = el.select(&link_sel).next() else {
                continue;
            };
            let href = l.value().attr("href").unwrap_or("");
            let url = absolute_url(base_url, href).unwrap_or_default();
            if title.is_empty() || url.is_empty() {
                continue;
            }
            let snippet = snip_sel
                .as_ref()
                .and_then(|s| el.select(s).next())
                .map(|s| clip(&s.text().collect::<String>(), 300))
                .unwrap_or_default();
            out.push(SearchResult::new(
                title,
                url,
                snippet,
                &self.config.id,
                out.len(),
            ));
        }
        out
    }
}

#[async_trait]
impl Engine for CustomEngine {
    fn meta(&self) -> EngineMeta {
        EngineMeta {
            id: Cow::Owned(self.config.id.clone()),
            name: Cow::Owned(self.config.name.clone()),
            category: self.config.category(),
            needs_js: self.config.needs_js,
        }
    }

    async fn search(
        &self,
        ctx: &EngineContext,
        query: &str,
        max: usize,
    ) -> Result<Vec<SearchResult>, EngineError> {
        let url = self
            .config
            .url_template
            .replace("{query}", &encode_query_pct(query));

        let html = if self.config.needs_js {
            let renderer = ctx.js_render.as_ref().ok_or(EngineError::Blocked(
                "该自定义引擎需要启用 JS 渲染桥（见设置 → JS 渲染桥）".into(),
            ))?;
            renderer
                .render(&url)
                .await
                .map_err(|e| EngineError::Http(e.to_string()))?
        } else {
            ctx.http
                .get_text(&self.config.id, &url)
                .await
                .map_err(|e| EngineError::Http(e.to_string()))?
        };

        let mut out = self.parse_html(&html, &url, max);
        if out.is_empty() {
            // 兜底：通用提取
            out = generic_extract(&html, &self.config.id, max);
        }
        if out.is_empty() {
            Err(EngineError::Parse(
                "无结果（检查选择器是否匹配页面结构）".into(),
            ))
        } else {
            Ok(out)
        }
    }
}

// ---- 持久化 ----

/// 从文件加载自定义引擎列表（文件缺失或损坏时返回空列表）。
pub fn load_custom(file: Option<&Path>) -> Vec<CustomEngineConfig> {
    let path = file.unwrap_or(Path::new(CUSTOM_FILE));
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

/// 保存自定义引擎列表到文件。
pub fn save_custom(configs: &[CustomEngineConfig], file: Option<&Path>) -> Result<(), String> {
    let path: &Path = match file {
        Some(p) => p,
        None => Path::new(CUSTOM_FILE),
    };
    let json = serde_json::to_string_pretty(configs).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| format!("写入 {} 失败: {e}", path.display()))
}
