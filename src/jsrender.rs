//! JS 渲染桥：对需要执行 JS 的搜索源（知乎/CSDN/简书/微信/Google 等），
//! 把渲染工作交给外部命令（Node + puppeteer-core / playwright），
//! 外部进程把渲染后的完整 HTML 输出到 stdout，由 Rust 侧解析。
//!
//! 配套脚本见 `js-exec/render.js`（npm i puppeteer-core 后即可使用，
//! 需要本机已有 Chrome/Chromium，可用 CHROME_PATH 环境变量指定）。

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};

use crate::config::JsRenderConfig;

pub struct JsRenderer {
    command: String,
    timeout: Duration,
}

impl JsRenderer {
    /// 依据配置构建渲染桥；未启用时返回 None。
    pub fn from_config(cfg: &JsRenderConfig) -> Option<Arc<Self>> {
        if !cfg.enabled || cfg.command.trim().is_empty() {
            return None;
        }
        Some(Arc::new(JsRenderer {
            command: cfg.command.trim().to_string(),
            timeout: Duration::from_millis(cfg.timeout_ms.max(1000)),
        }))
    }

    /// 渲染 url 对应的页面，返回完整 HTML（截断到 4MB 防失控）。
    pub async fn render(&self, url: &str) -> Result<String> {
        let mut parts = self.command.split_whitespace();
        let program = parts
            .next()
            .ok_or_else(|| anyhow!("js_render.command 为空"))?;
        let args: Vec<&str> = parts.collect();

        let mut cmd = tokio::process::Command::new(program);
        cmd.args(&args).arg(url).stdout(Stdio::piped()).stderr(Stdio::piped());

        let output = tokio::time::timeout(self.timeout, cmd.output())
            .await
            .map_err(|_| anyhow!("JS 渲染超时（>{}s）: {url}", self.timeout.as_secs()))?
            .with_context(|| format!("无法启动渲染命令 {program}"))?;

        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("渲染命令失败({program}): {}", err.trim()));
        }
        let html = String::from_utf8_lossy(&output.stdout);
        if html.trim().is_empty() {
            return Err(anyhow!("渲染命令未输出内容: {url}"));
        }
        Ok(html.chars().take(4_000_000).collect())
    }
}
