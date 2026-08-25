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
#[cfg(feature = "tls-rustls")]
use crate::tls::rustls_backend::{self, TrustSource};
#[cfg(feature = "tls-native")]
use crate::tls::native_backend;

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
    /// 系统根证书回退池（仅 tls-rustls 后端）：证书校验失败时自动重连用。
    fallback: Option<Arc<Vec<reqwest::Client>>>,
    rotate: Rotate,
    idx: Arc<AtomicUsize>,
    rate: Arc<RateLimiter>,
}

impl HttpClient {
    /// 依据代理配置构建客户端池。
    ///
    /// 信任链（见 src/tls.rs）：
    /// - 主池：内置 Mozilla 根证书（webpki-roots）+ 私有 CA（EDUCE_CA_PEM）
    /// - 回退池（仅 tls-rustls 后端）：系统根证书 + 私有 CA，证书校验失败时自动重连
    /// - tls-native 后端（schannel 等）本就信任系统根证书，无回退池
    pub fn new(cfg: &ProxyConfig) -> Result<Self> {
        let clients = build_pool(cfg, tls_client_builder)?;
        let fallback = build_fallback_pool(cfg)?;

        let rotate = Rotate::from_str(&cfg.rotate);
        Ok(HttpClient {
            clients: Arc::new(clients),
            fallback,
            rotate,
            idx: Arc::new(AtomicUsize::new(0)),
            rate: Arc::new(RateLimiter::new()),
        })
    }

    /// 为引擎注册限速（请求/分钟）。
    pub fn set_rate_limit(&self, engine: &str, per_min: u32) {
        self.rate.set_limit(engine, per_min);
    }

    fn pick_from<'a>(&self, pool: &'a [reqwest::Client]) -> &'a reqwest::Client {
        if pool.len() == 1 {
            return &pool[0];
        }
        match self.rotate {
            Rotate::Random => {
                let i = rand::thread_rng().gen_range(0..pool.len());
                &pool[i]
            }
            Rotate::RoundRobin => {
                let i = self.idx.fetch_add(1, Ordering::Relaxed) % pool.len();
                &pool[i]
            }
        }
    }

    fn pick_client(&self) -> &reqwest::Client {
        self.pick_from(&self.clients)
    }

    fn pick_ua(&self) -> &'static str {
        UA_POOL[rand::thread_rng().gen_range(0..UA_POOL.len())]
    }

    fn base_request_with(&self, client: &reqwest::Client, url: &str) -> Result<reqwest::RequestBuilder> {
        let ua = self.pick_ua();
        let lang = LANG_POOL[rand::thread_rng().gen_range(0..LANG_POOL.len())];
        Ok(client
            .get(url)
            .header(USER_AGENT, ua)
            .header(ACCEPT, "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8")
            .header(ACCEPT_LANGUAGE, lang))
    }

    fn base_request(&self, url: &str) -> Result<reqwest::RequestBuilder> {
        let client = self.pick_client();
        self.base_request_with(client, url)
    }

    /// 发 GET 请求（先过限速，随机 UA/代理）。返回 Response，由调用方决定解析方式。
    pub async fn get(&self, engine: &str, url: &str) -> Result<reqwest::Response> {
        self.rate.acquire(engine).await;
        self.send(engine, url, &[]).await
    }

    /// 带附加请求头的 GET（如 Referer、Cookie）。
    pub async fn get_with_headers(
        &self,
        engine: &str,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<reqwest::Response> {
        self.rate.acquire(engine).await;
        self.send(engine, url, headers).await
    }

    /// 发送 GET；证书校验失败时自动用系统根证书回退池重连一次。
    async fn send(
        &self,
        engine: &str,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<reqwest::Response> {
        match self.send_once(engine, url, headers, None).await {
            Ok(resp) => Ok(resp),
            Err(err) => {
                if let Some(pool) = &self.fallback {
                    if is_cert_error(&err) {
                        tracing::warn!(engine, url, "证书校验失败，回退系统根证书重连");
                        let client = self.pick_from(pool);
                        if let Ok(resp) = self.send_once(engine, url, headers, Some(client)).await
                        {
                            return Ok(resp);
                        }
                    }
                }
                Err(err)
            }
        }
    }

    async fn send_once(
        &self,
        engine: &str,
        url: &str,
        headers: &[(&str, &str)],
        client: Option<&reqwest::Client>,
    ) -> Result<reqwest::Response> {
        let mut req = match client {
            Some(c) => self.base_request_with(c, url),
            None => self.base_request(url),
        }
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
            return Err(anyhow!("HTTP {}: {}", resp.status().as_u16(), url));
        }
        resp.text().await.context("读取响应体失败")
    }
}

// ---- TLS 信任链辅助（见 src/tls.rs）----

/// TLS 后端的主客户端 builder（rustls：内置 Mozilla 根 + 私有 CA；native：系统根 + 私有 CA）。
fn tls_client_builder() -> Result<reqwest::ClientBuilder> {
    #[cfg(feature = "tls-rustls")]
    {
        let store = rustls_backend::build_root_store(TrustSource::Builtin)?;
        let config = rustls::ClientConfig::builder()
            .with_root_certificates(store)
            .with_no_client_auth();
        return Ok(reqwest::Client::builder().use_preconfigured_tls(config));
    }
    #[cfg(feature = "tls-native")]
    {
        let mut builder = reqwest::Client::builder();
        for cert in native_backend::ca_certificates()? {
            builder = builder.add_root_certificate(cert);
        }
        Ok(builder)
    }
}

/// 回退池客户端 builder：系统根证书 + 私有 CA（仅 tls-rustls 后端需要）。
#[cfg(feature = "tls-rustls")]
fn fallback_tls_client_builder() -> Result<reqwest::ClientBuilder> {
    let store = rustls_backend::build_root_store(TrustSource::System)?;
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(store)
        .with_no_client_auth();
    Ok(reqwest::Client::builder().use_preconfigured_tls(config))
}

/// 按代理配置构建整个客户端池（直连 + 每代理一个客户端），应用公共连接调优。
/// `tls` 为客户端 builder 工厂：reqwest 的 ClientBuilder 不可 Clone，每个客户端单独构造。
fn build_pool(
    cfg: &ProxyConfig,
    tls: impl Fn() -> Result<reqwest::ClientBuilder>,
) -> Result<Vec<reqwest::Client>> {
    // 公共连接调优：空闲连接复用、keepalive、TCP 保活、超时分级（连接 10s / 整体 20s）
    let tuned = |tls: reqwest::ClientBuilder| {
        tls.user_agent("Mozilla/5.0")
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::limited(8))
            // 维持会话 cookie（百度/搜狗/360 等需要）
            .cookie_store(true)
            // 每主机最多保留 8 条空闲连接，多源并发时复用
            .pool_max_idle_per_host(8)
            .pool_idle_timeout(Duration::from_secs(60))
            .http2_keep_alive_interval(Duration::from_secs(30))
            .http2_keep_alive_timeout(Duration::from_secs(10))
            .tcp_keepalive(Duration::from_secs(30))
    };

    let mut clients = Vec::new();
    if cfg.enabled && !cfg.urls.is_empty() {
        for url in &cfg.urls {
            let proxy = reqwest::Proxy::all(url).with_context(|| format!("非法代理地址: {url}"))?;
            let c = tuned(tls()?)
                .proxy(proxy)
                .build()
                .context("构建代理客户端失败")?;
            clients.push(c);
        }
    }
    if clients.is_empty() {
        clients.push(tuned(tls()?).build().context("构建 HTTP 客户端失败")?);
    }
    Ok(clients)
}

/// 系统根证书回退池；tls-native 后端天然信任系统根证书，无需回退池。
#[cfg(feature = "tls-rustls")]
fn build_fallback_pool(cfg: &ProxyConfig) -> Result<Option<Arc<Vec<reqwest::Client>>>> {
    let pool = build_pool(cfg, fallback_tls_client_builder)?;
    Ok(Some(Arc::new(pool)))
}

#[cfg(feature = "tls-native")]
fn build_fallback_pool(_cfg: &ProxyConfig) -> Result<Option<Arc<Vec<reqwest::Client>>>> {
    Ok(None)
}

/// 判定是否为 TLS 证书校验失败（tls-native 后端无内置根库可回退，恒为 false）。
fn is_cert_error(err: &anyhow::Error) -> bool {
    #[cfg(feature = "tls-rustls")]
    {
        return rustls_backend::is_certificate_error(err);
    }
    #[cfg(feature = "tls-native")]
    {
        let _ = err;
        false
    }
}
