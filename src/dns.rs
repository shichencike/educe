//! 自定义 DNS 解析器：修复安卓 8.1 / 部分移动网络下"全部源统一超时"。
//!
//! 背景（见 src/http.rs 连接调优注释）：安卓 8.1 上系统 getaddrinfo 解析出的地址
//! 常把 IPv6 排前，而移动网络 IPv6 链路不通时 TCP 连接会一直挂起；且系统解析器
//! 本身可能长时间无响应（DNS 服务器不可达时没有超时上限），一次请求就把单源预算
//! 耗尽。两者都表现为"全部源 ~8s 统一失败"。
//!
//! 本解析器在 reqwest 的 DNS 层做三件事：
//! - 解析带硬超时（`EDUCE_DNS_TIMEOUT_MS`，默认 3000ms）：解析挂起立即失败，
//!   而不是干耗预算，错误信息明确暴露"DNS 解析超时"
//! - IPv4 地址排到前面：hyper-util 的 happy-eyeballs 把"第一个地址的地址族"
//!   作为优先组，IPv4 优先即可在 IPv4 可用时立刻建连，不被坏掉的 IPv6 拖累
//! - `EDUCE_NO_IPV6=1` 时直接丢弃 IPv6 地址（IPv6 链路完全不可用时的终极手段）
//!
//! 通过 `reqwest::ClientBuilder::dns_resolver` 注入（reqwest 0.12 公开 API，
//! 对主客户端与系统根证书回退客户端同时生效）。

use std::env;
use std::net::SocketAddr;
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

/// DNS 解析超时环境变量（毫秒）。
pub const ENV_DNS_TIMEOUT_MS: &str = "EDUCE_DNS_TIMEOUT_MS";
/// 跳过 IPv6 环境变量（"1"/"true" 启用）。
pub const ENV_NO_IPV6: &str = "EDUCE_NO_IPV6";

const DEFAULT_DNS_TIMEOUT_MS: u64 = 3000;
const MIN_DNS_TIMEOUT_MS: u64 = 500;
const MAX_DNS_TIMEOUT_MS: u64 = 8000;

/// IPv4 优先 + 解析超时的 DNS 解析器。
#[derive(Clone, Debug)]
pub struct PreferV4Resolver {
    resolve_timeout: Duration,
    drop_ipv6: bool,
}

impl PreferV4Resolver {
    /// 依据环境变量构造（未设置时用默认值）。
    pub fn from_env() -> Self {
        let timeout_ms = env::var_os(ENV_DNS_TIMEOUT_MS)
            .and_then(|v| v.to_string_lossy().parse::<u64>().ok())
            .unwrap_or(DEFAULT_DNS_TIMEOUT_MS)
            .clamp(MIN_DNS_TIMEOUT_MS, MAX_DNS_TIMEOUT_MS);
        let drop_ipv6 = env::var_os(ENV_NO_IPV6)
            .map(|v| matches!(v.to_string_lossy().as_ref(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);
        tracing::info!(
            resolve_timeout_ms = timeout_ms,
            drop_ipv6,
            "DNS 解析器就绪：IPv4 优先 + 解析硬超时"
        );
        Self {
            resolve_timeout: Duration::from_millis(timeout_ms),
            drop_ipv6,
        }
    }
}

impl Resolve for PreferV4Resolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        let timeout = self.resolve_timeout;
        let drop_ipv6 = self.drop_ipv6;
        Box::pin(async move {
            let lookups = tokio::time::timeout(timeout, tokio::net::lookup_host((host.as_str(), 0)))
                .await
                .map_err(|_| format!("DNS 解析超时({host}) 超过 {timeout:?}"))?
                .map_err(|e| format!("DNS 解析失败({host}): {e}"))?
                .collect::<Vec<_>>();

            let mut v4 = Vec::new();
            let mut v6 = Vec::new();
            for addr in lookups {
                match addr {
                    SocketAddr::V4(_) => v4.push(addr),
                    SocketAddr::V6(_) if !drop_ipv6 => v6.push(addr),
                    _ => {}
                }
            }
            if v4.is_empty() && v6.is_empty() {
                let hint = if drop_ipv6 {
                    "（已按 EDUCE_NO_IPV6 跳过 IPv6）"
                } else {
                    ""
                };
                return Err(format!("DNS 解析无可用地址({host}){hint}").into());
            }
            // IPv4 优先：hyper-util 的 happy-eyeballs 以首个地址的地址族作为优先组
            v4.extend(v6);
            let addrs: Addrs = Box::new(v4.into_iter());
            Ok(addrs)
        })
    }
}
