//! 运行时设置（设置页可动态修改的全局项）：代理池 + JS 渲染桥。
//!
//! 与 config.toml 的区别：config.toml 是启动静态配置；runtime.toml 是运行时
//! 可动态修改项，启动时若存在则覆盖 config 对应字段，修改后立即重建
//! HTTP 客户端 / JS 渲染桥生效（无需重启）。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::{AppConfig, JsRenderConfig, ProxyConfig};

/// 运行时配置文件（默认工作目录下）
pub const RUNTIME_FILE: &str = "runtime.toml";

/// 运行时设置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeSettings {
    /// 代理池开关
    pub proxy_enabled: bool,
    /// 代理地址列表（http / https / socks5）
    pub proxy_urls: Vec<String>,
    /// 轮换策略 round_robin | random
    pub proxy_rotate: String,
    /// JS 渲染桥开关
    pub js_enabled: bool,
    /// 渲染命令（接收一个 URL 参数，HTML 输出到 stdout）
    pub js_command: String,
    /// 渲染超时（毫秒）
    pub js_timeout_ms: u64,
    /// 走 JS 桥的源（引擎 id）
    pub js_sources: Vec<String>,
}

impl Default for RuntimeSettings {
    fn default() -> Self {
        RuntimeSettings {
            proxy_enabled: false,
            proxy_urls: Vec::new(),
            proxy_rotate: "round_robin".into(),
            js_enabled: false,
            js_command: "node js-exec/render.js".into(),
            js_timeout_ms: 30_000,
            js_sources: Vec::new(),
        }
    }
}

impl RuntimeSettings {
    /// 从静态配置生成初始运行时设置（启动时无 runtime.toml 时使用）。
    pub fn from_cfg(cfg: &AppConfig) -> Self {
        RuntimeSettings {
            proxy_enabled: cfg.proxy.enabled,
            proxy_urls: cfg.proxy.urls.clone(),
            proxy_rotate: cfg.proxy.rotate.clone(),
            js_enabled: cfg.js_render.enabled,
            js_command: cfg.js_render.command.clone(),
            js_timeout_ms: cfg.js_render.timeout_ms,
            js_sources: cfg.js_render.sources.clone(),
        }
    }

    /// 转成 ProxyConfig（用于重建 HttpClient）。
    pub fn to_proxy_config(&self) -> ProxyConfig {
        ProxyConfig {
            enabled: self.proxy_enabled,
            urls: self.proxy_urls.clone(),
            rotate: self.proxy_rotate.clone(),
        }
    }

    /// 转成 JsRenderConfig（用于重建 JsRenderer）。
    pub fn to_js_config(&self) -> JsRenderConfig {
        JsRenderConfig {
            enabled: self.js_enabled,
            command: self.js_command.clone(),
            timeout_ms: self.js_timeout_ms,
            sources: self.js_sources.clone(),
        }
    }

    /// 从默认路径加载 runtime.toml（缺失/损坏返回 None）。
    pub fn load_file() -> Option<Self> {
        Self::load_file_at(Path::new(RUNTIME_FILE))
    }

    pub fn load_file_at(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        toml::from_str(&text).ok()
    }

    /// 保存到默认路径 runtime.toml。
    pub fn save_file(&self) -> Result<(), String> {
        self.save_file_at(Path::new(RUNTIME_FILE))
    }

    pub fn save_file_at(&self, path: &Path) -> Result<(), String> {
        let text = toml::to_string(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| format!("写入 {} 失败: {e}", path.display()))
    }
}
