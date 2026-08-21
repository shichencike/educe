//! 用户偏好（SearXNG 风格）：界面偏好 + 每引擎开关/权重。
//! 以 cookie `educe_prefs` 持久化：JSON 经百分号编码后存入，无需登录。
//! 不依赖 cookie crate —— 手动构造 Set-Cookie 头 / 解析 Cookie 请求头。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::config::AppConfig;
use crate::engines::all_metas;

/// cookie 名称
pub const PREFS_COOKIE: &str = "educe_prefs";
/// cookie 有效期（秒）
pub const PREFS_MAX_AGE: i64 = 365 * 24 * 3600;

/// 单引擎偏好
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnginePref {
    pub enabled: bool,
    /// None = 用配置文件里的权重
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<f64>,
}

/// 用户偏好集合
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UserPrefs {
    /// 界面语言 zh | en
    pub lang: String,
    /// 主题 dark | light
    pub theme: String,
    /// 每页结果数（/api/search 的 max 缺省值）
    pub results_per_page: usize,
    /// 结果是否新标签页打开
    pub open_in_new_tab: bool,
    /// 单源超时（毫秒），0 = 用配置文件
    pub timeout_ms: u64,
    /// 引擎 id -> 偏好
    pub engines: HashMap<String, EnginePref>,
}

impl Default for UserPrefs {
    fn default() -> Self {
        UserPrefs {
            lang: "zh".into(),
            theme: "dark".into(),
            results_per_page: 50,
            open_in_new_tab: true,
            timeout_ms: 0,
            engines: HashMap::new(),
        }
    }
}

impl UserPrefs {
    /// 以配置为基础生成默认偏好（引擎启用状态与权重取自配置）。
    pub fn defaults_from_config(cfg: &AppConfig) -> Self {
        let cfg_enabled = &cfg.engines.enabled;
        let mut engines = HashMap::new();
        for m in all_metas() {
            let enabled =
                cfg_enabled.is_empty() || cfg_enabled.iter().any(|e| e.as_str() == m.id.as_ref());
            engines.insert(
                m.id.to_string(),
                EnginePref {
                    enabled,
                    weight: cfg.weights.get(m.id.as_ref()).copied(),
                },
            );
        }
        UserPrefs {
            results_per_page: cfg.search.max_results.clamp(10, 100),
            engines,
            ..UserPrefs::default()
        }
    }

    /// 用另一份偏好覆盖当前值（POST 部分更新；空字段不覆盖）。
    pub fn merge_from(&mut self, other: &UserPrefs) {
        if !other.lang.is_empty() {
            self.lang = other.lang.clone();
        }
        if !other.theme.is_empty() {
            self.theme = other.theme.clone();
        }
        if other.results_per_page != 0 {
            self.results_per_page = other.results_per_page;
        }
        self.open_in_new_tab = other.open_in_new_tab;
        if other.timeout_ms != 0 {
            self.timeout_ms = other.timeout_ms;
        }
        for (id, pref) in &other.engines {
            self.engines.insert(id.clone(), pref.clone());
        }
    }

    /// 当前启用的引擎 id（按注册顺序）。
    pub fn enabled_engine_ids(&self) -> Vec<String> {
        all_metas()
            .iter()
            .filter(|m| {
                self.engines
                    .get(m.id.as_ref())
                    .map(|p| p.enabled)
                    .unwrap_or(true)
            })
            .map(|m| m.id.to_string())
            .collect()
    }

    /// 偏好中显式设置的权重覆盖。
    pub fn weight_overrides(&self) -> HashMap<String, f64> {
        self.engines
            .iter()
            .filter_map(|(id, p)| p.weight.map(|w| (id.clone(), w)))
            .collect()
    }

    /// 生效的单源超时（毫秒）。
    pub fn effective_timeout(&self, cfg: &AppConfig) -> u64 {
        if self.timeout_ms > 0 {
            self.timeout_ms
        } else {
            cfg.search.timeout_ms
        }
    }

    /// 序列化为 cookie 值（percent 编码，避免 JSON 特殊字符破坏 cookie 头）。
    pub fn to_cookie(&self) -> String {
        let json = serde_json::to_string(self).unwrap_or_else(|_| "{}".into());
        percent_encode(&json)
    }

    /// 从 cookie 值解析；解析失败返回 None（调用方回退默认偏好）。
    pub fn from_cookie(value: &str) -> Option<Self> {
        let json = percent_decode(value)?;
        serde_json::from_str(&json).ok()
    }
}

fn percent_encode(s: &str) -> String {
    use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
    utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
}

fn percent_decode(s: &str) -> Option<String> {
    use percent_encoding::percent_decode_str;
    percent_decode_str(s)
        .decode_utf8()
        .ok()
        .map(|c| c.into_owned())
}

/// 从 Cookie 请求头中取出指定名称的原始值。
pub fn extract_cookie<'a>(header: Option<&'a str>, name: &str) -> Option<&'a str> {
    let header = header?;
    for part in header.split(';') {
        let part = part.trim();
        if let Some((k, v)) = part.split_once('=') {
            if k.trim() == name {
                return Some(v.trim());
            }
        }
    }
    None
}

/// 构造 Set-Cookie 响应头（删除时传空值 + Max-Age=0）。
pub fn set_cookie_header(value: &str, max_age: i64) -> String {
    format!(
        "{PREFS_COOKIE}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}"
    )
}
