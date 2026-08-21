//! 配置加载：内置默认值 -> config.toml 文件 -> 环境变量覆盖。
//!
//! 环境变量规则：`EDUCE_` 前缀 + 层级路径（`__` 分隔），如
//! `EDUCE_SERVER__PORT=9000`、`EDUCE_PROXY__ENABLED=true`。
//! 值会自动推断为整数 / 浮点 / 布尔 / 字符串。

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

/// 默认配置文件的完整内容（gen-config 命令输出用）。
pub const DEFAULT_CONFIG_TOML: &str = include_str!("../config.example.toml");

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            host: "127.0.0.1".into(),
            port: 8080,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SearchConfig {
    /// 每源最多返回结果数
    pub max_per_source: usize,
    /// 单源超时（毫秒）
    pub timeout_ms: u64,
    /// 聚合去重后返回上限
    pub max_results: usize,
    /// 是否跨源去重合并
    pub dedup: bool,
    /// 同时请求的源数量上限（限流，降低目标站与网络压力）
    pub max_concurrent: usize,
}

impl Default for SearchConfig {
    fn default() -> Self {
        SearchConfig {
            max_per_source: 30,
            timeout_ms: 10_000,
            max_results: 100,
            dedup: true,
            max_concurrent: 8,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ProxyConfig {
    pub enabled: bool,
    /// 代理地址列表，支持 http / https / socks5
    pub urls: Vec<String>,
    /// round_robin | random
    pub rotate: String,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        ProxyConfig {
            enabled: false,
            urls: Vec::new(),
            rotate: "round_robin".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct JsRenderConfig {
    pub enabled: bool,
    /// 外部渲染命令，接收一个 URL 参数，渲染后的 HTML 输出到 stdout
    pub command: String,
    pub timeout_ms: u64,
    /// 使用 JS 渲染桥的源
    pub sources: Vec<String>,
}

impl Default for JsRenderConfig {
    fn default() -> Self {
        JsRenderConfig {
            enabled: false,
            command: "node js-exec/render.js".into(),
            timeout_ms: 30_000,
            sources: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct EnginesConfig {
    /// 启用白名单；空 = 全部启用
    pub enabled: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        LoggingConfig {
            level: "info".into(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub search: SearchConfig,
    pub proxy: ProxyConfig,
    /// 每源限速（请求/分钟），key 可为引擎 id 或 "default"
    pub rate_limit: HashMap<String, u32>,
    /// 引擎权重（影响跨源评分），key 为引擎 id
    pub weights: HashMap<String, f64>,
    pub js_render: JsRenderConfig,
    pub engines: EnginesConfig,
    pub logging: LoggingConfig,
}

impl AppConfig {
    /// 加载配置：默认值 + 可选 TOML 文件 + 环境变量覆盖。
    /// 配置文件缺失时仅告警并继续（用默认值），解析失败则报错。
    pub fn load(path: Option<&str>) -> Result<Self> {
        let mut table = toml::Table::new();
        if let Some(p) = path {
            if Path::new(p).exists() {
                let text =
                    std::fs::read_to_string(p).with_context(|| format!("读取配置文件失败: {p}"))?;
                table = toml::from_str(&text).with_context(|| format!("解析配置文件失败: {p}"))?;
            } else {
                eprintln!(
                    "[warn] 配置文件不存在: {p}，使用默认配置（可运行 `educe gen-config` 生成）"
                );
            }
        }
        apply_env_overrides(&mut table);
        let cfg: AppConfig = table
            .try_into()
            .context("配置内容非法（字段类型不匹配？）")?;
        Ok(cfg)
    }

    /// 查询某引擎的限速（次/分钟），未配置时回退 default -> 30。
    pub fn rate_limit_for(&self, engine: &str) -> u32 {
        self.rate_limit
            .get(engine)
            .or_else(|| self.rate_limit.get("default"))
            .copied()
            .unwrap_or(30)
    }

    /// 查询某引擎的排序权重，默认 1.0。
    pub fn weight_for(&self, engine: &str) -> f64 {
        self.weights.get(engine).copied().unwrap_or(1.0)
    }
}

const ENV_PREFIX: &str = "EDUCE_";

/// 递归遍历配置表，将 `EDUCE_A__B__C=value` 写入对应路径。
fn apply_env_overrides(table: &mut toml::Table) {
    for (key, value) in std::env::vars() {
        let Some(rest) = key.strip_prefix(ENV_PREFIX) else {
            continue;
        };
        if rest.is_empty() {
            continue;
        }
        let parts: Vec<&str> = rest.split("__").collect();
        set_path(table, &parts, coerce_env_value(&value));
    }
}

/// 沿路径逐级进入（缺失的表自动创建），最后一级写入值。
fn set_path(table: &mut toml::Table, parts: &[&str], value: toml::Value) {
    if parts.len() == 1 {
        table.insert(parts[0].to_string(), value);
        return;
    }
    let entry = table
        .entry(parts[0].to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let toml::Value::Table(sub) = entry {
        set_path(sub, &parts[1..], value);
    }
}

/// 把环境变量字符串推断为合适的 TOML 值。
fn coerce_env_value(s: &str) -> toml::Value {
    if let Ok(i) = s.parse::<i64>() {
        return toml::Value::Integer(i);
    }
    if let Ok(f) = s.parse::<f64>() {
        return toml::Value::Float(f);
    }
    match s {
        "true" => toml::Value::Boolean(true),
        "false" => toml::Value::Boolean(false),
        _ => toml::Value::String(s.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coerce_env_value_infers_types() {
        assert_eq!(coerce_env_value("9000"), toml::Value::Integer(9000));
        assert_eq!(coerce_env_value("1.5"), toml::Value::Float(1.5));
        assert_eq!(coerce_env_value("true"), toml::Value::Boolean(true));
        assert_eq!(coerce_env_value("abc"), toml::Value::String("abc".into()));
    }

    #[test]
    fn set_path_writes_nested_table() {
        let mut table = toml::Table::new();
        set_path(&mut table, &["server", "port"], toml::Value::Integer(9000));
        assert_eq!(table["server"]["port"].as_integer(), Some(9000));
    }

    #[test]
    fn env_override_applies_to_load() {
        std::env::set_var("EDUCE_SERVER__PORT", "9000");
        let cfg = AppConfig::load(None).expect("默认配置可加载");
        std::env::remove_var("EDUCE_SERVER__PORT");
        assert_eq!(cfg.server.port, 9000);
    }

    #[test]
    fn rate_limit_and_weight_fallbacks() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.rate_limit_for("any_unknown"), 30); // 回退 default
        assert_eq!(cfg.weight_for("any_unknown"), 1.0);
    }
}
