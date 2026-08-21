//! HTTP 基础层：reqwest 客户端池（直连 + 代理旋转）、UA 轮换、
//! 随机请求头、每源令牌桶限速。
//!
//! 注意：reqwest 的 TLS 后端由 Cargo feature 决定（tls-rustls / tls-native），
//! 本模块不关心具体后端。

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use rand::Rng;
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, USER_AGENT};

use crate::config::ProxyConfig;

/// 浏览器 UA 池（请求时随机挑一个，降低被识别为爬虫的概率）。
const UA_POOL: &[&str] = &[
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36 Edg/125.0.0.0",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15",
    "Mozilla/5.0 (X11; Linux x86_64; rv:127.0) Gecko/20100101 Firefox/127.0",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:126.0) Gecko/20100101 Firefox/126.0",
    "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1",
    "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Mobile Safari/537.36",
];

/// 请求头中 Accept-Language 的候选值。
const LANG_POOL: &[&str] = &[
    "zh-CN,zh;q=0.9,en;q=0.8",
    "en-US,en;q=0.9,zh-CN;q=0.8",
    "zh-CN,zh;q=0.8,en-US;q=0.7,en;q=0.6",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rotate {
    RoundRobin,
    Random,
}

impl Rotate {
    fn from_str(s: &str) -> Rotate {
        match s {
            "random" => Rotate::Random,
            _ => Rotate::RoundRobin,
        }
    }
}

/// 每源令牌桶限速器（请求/分钟）。线程安全，跨请求共享。
#[derive(Debug, Default)]
pub struct RateLimiter {
    buckets: Mutex<HashMap<String, Bucket>>,
}

#[derive(Debug)]
struct Bucket {
    per_min: u32,
    tokens: f64,
    last: Instant,
}

impl RateLimiter {
    pub fn new() -> Self {
        RateLimiter {
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// 设置某引擎的限速（0 = 不限）。
    pub fn set_limit(&self, engine: &str, per_min: u32) {
        let mut buckets = self.buckets.lock().unwrap();
        match buckets.get_mut(engine) {
            Some(b) => b.per_min = per_min,
            None => {
                buckets.insert(
                    engine.to_string(),
                    Bucket {
                        per_min,
                        tokens: per_min as f64,
                        last: Instant::now(),
                    },
                );
            }
        }
    }

    /// 等待直到可以发起一次请求。
    pub async fn acquire(&self, engine: &str) {
        loop {
            let wait = {
                let mut buckets = self.buckets.lock().unwrap();
                let now = Instant::now();
                let b = buckets.entry(engine.to_string()).or_insert_with(|| Bucket {
                    per_min: u32::MAX,
                    tokens: f64::MAX,
                    last: now,
                });
                if b.per_min == 0 {
                    None // 不限速
                } else {
                    // 按流逝时间补充令牌
                    let elapsed = now.duration_since(b.last).as_secs_f64() / 60.0;
                    b.tokens = (b.tokens + elapsed * b.per_min as f64).min(b.per_min as f64);
                    b.last = now;
                    if b.tokens >= 1.0 {
                        b.tokens -= 1.0;
                        None
                    } else {
                        let need_secs = ((1.0 - b.tokens) / b.per_min as f64 * 60.0).ceil();
                        Some(Duration::from_secs_f64(need_secs.clamp(0.05, 60.0)))
                    }
                }
            };
            match wait {
                Some(d) => tokio::time::sleep(d).await,
                None => return,
            }
        }
    }
}

/// HTTP 客户端池：直连 + 每代理一个客户端，请求时按策略旋转；
/// 每次请求随机挑选 UA 与 Accept-Language。
#[derive(Clone)]
pub struct HttpClient {
    clients: Arc<Vec<reqwest::Client>>,
    rotate: Rotate,
    idx: Arc<AtomicUsize>,
    rate: Arc<RateLimiter>,
}

impl HttpClient {
    /// 依据代理配置构建客户端池。
    pub fn new(cfg: &ProxyConfig) -> Result<Self> {
        let builder = || {
            reqwest::Client::builder()
                .user_agent("Mozilla/5.0")
                .timeout(Duration::from_secs(20))
                .connect_timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::limited(8))
                // 维持会话 cookie（百度/搜狗/360 等需要）
                .cookie_store(true)
                .pool_max_idle_per_host(2)
                .build()
        };

        let mut clients = Vec::new();
        if cfg.enabled && !cfg.urls.is_empty() {
            for url in &cfg.urls {
                let proxy = reqwest::Proxy::all(url)
                    .with_context(|| format!("非法代理地址: {url}"))?;
                let c = reqwest::Client::builder()
                    .proxy(proxy)
                    .timeout(Duration::from_secs(20))
                    .connect_timeout(Duration::from_secs(10))
                    .redirect(reqwest::redirect::Policy::limited(8))
                    .cookie_store(true)
                    .build()
                    .context("构建代理客户端失败")?;
                clients.push(c);
            }
        }
        if clients.is_empty() {
            clients.push(builder().context("构建 HTTP 客户端失败")?);
        }

        let rotate = Rotate::from_str(&cfg.rotate);
        Ok(HttpClient {
            clients: Arc::new(clients),
            rotate,
            idx: Arc::new(AtomicUsize::new(0)),
            rate: Arc::new(RateLimiter::new()),
        })
    }

    /// 为引擎注册限速（请求/分钟）。
    pub fn set_rate_limit(&self, engine: &str, per_min: u32) {
        self.rate.set_limit(engine, per_min);
    }

    fn pick_client(&self) -> &reqwest::Client {
        if self.clients.len() == 1 {
            return &self.clients[0];
        }
        match self.rotate {
            Rotate::Random => {
                let i = rand::thread_rng().gen_range(0..self.clients.len());
                &self.clients[i]
            }
            Rotate::RoundRobin => {
                let i = self.idx.fetch_add(1, Ordering::Relaxed) % self.clients.len();
                &self.clients[i]
            }
        }
    }

    fn pick_ua(&self) -> &'static str {
        UA_POOL[rand::thread_rng().gen_range(0..UA_POOL.len())]
    }

    fn base_request(&self, url: &str) -> Result<reqwest::RequestBuilder> {
        let client = self.pick_client();
        let ua = self.pick_ua();
        let lang = LANG_POOL[rand::thread_rng().gen_range(0..LANG_POOL.len())];
        Ok(client
            .get(url)
            .header(USER_AGENT, ua)
            .header(ACCEPT, "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8")
            .header(ACCEPT_LANGUAGE, lang))
    }

    /// 发 GET 请求（先过限速，随机 UA/代理）。返回 Response，由调用方决定解析方式。
    pub async fn get(&self, engine: &str, url: &str) -> Result<reqwest::Response> {
        self.rate.acquire(engine).await;
        let req = self
            .base_request(url)
            .with_context(|| format!("构造请求失败: {url}"))?;
        let resp = req
            .send()
            .await
            .with_context(|| format!("请求失败({engine}): {url}"))?;
        Ok(resp)
    }

    /// 带附加请求头的 GET（如 Referer、Cookie）。
    pub async fn get_with_headers(
        &self,
        engine: &str,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<reqwest::Response> {
        self.rate.acquire(engine).await;
        let mut req = self
            .base_request(url)
            .with_context(|| format!("构造请求失败: {url}"))?;
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let resp = req
            .send()
            .await
            .with_context(|| format!("请求失败({engine}): {url}"))?;
        Ok(resp)
    }

    /// GET 并取文本（自动按响应 charset 解码）。
    pub async fn get_text(&self, engine: &str, url: &str) -> Result<String> {
        let resp = self.get(engine, url).await?;
        if !resp.status().is_success() {
            return Err(anyhow!(
                "HTTP {}: {}",
                resp.status().as_u16(),
                url
            ));
        }
        Ok(resp.text().await.context("读取响应体失败")?)
    }
}
