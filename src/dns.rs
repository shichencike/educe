//! 自定义 DNS 解析器：修复安卓 8.1 / 部分移动网络下"全部源统一超时"。
//!
//! 背景（见 src/http.rs 连接调优注释）：安卓 8.1 上系统 getaddrinfo 解析出的地址
//! 常把 IPv6 排前，而移动网络 IPv6 链路不通时 TCP 连接会一直挂起；更常见的是系统
//! 解析器本身长时间无响应（DNS 服务器不可达时没有超时上限），一次请求就把单源
//! 预算耗尽。两者都表现为"全部源 ~8s 统一失败"，且 UI 只显示
//! "请求失败(engine): url" 外层上下文，根因被吞掉。
//!
//! 本解析器在 reqwest 的 DNS 层做四件事：
//! - 解析带硬超时（`EDUCE_DNS_TIMEOUT_MS`，默认 3000ms）：解析挂起立即失败，
//!   而不是干耗预算，错误信息明确暴露"DNS 解析超时"
//! - 系统 getaddrinfo 失败/超时后，**回退公共 DNS 直连查询**（hickory-resolver，
//!   阿里 223.5.5.5 / DNSPod 119.29.29.29 / 114DNS / 谷歌 8.8.8.8），绕开损坏的
//!   系统解析器（Termux 的 resolv.conf 指向的 DNS 不可达时仍能解析）
//! - IPv4 地址排到前面：hyper-util 的 happy-eyeballs 把"第一个地址的地址族"
//!   作为优先组，IPv4 优先即可在 IPv4 可用时立刻建连，不被坏掉的 IPv6 拖累
//! - `EDUCE_NO_IPV6=1` 时直接丢弃 IPv6 地址（IPv6 链路完全不可用时的终极手段）
//!
//! 通过 `reqwest::ClientBuilder::dns_resolver` 注入（reqwest 0.12 公开 API，
//! 对主客户端与系统根证书回退客户端同时生效）。

use std::env;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::OnceLock;
use std::time::Duration;

use hickory_resolver::config::{LookupIpStrategy, ResolverConfig, ResolverOpts, ServerGroup};
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_resolver::TokioResolver;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};

/// DNS 解析超时环境变量（毫秒）。
pub const ENV_DNS_TIMEOUT_MS: &str = "EDUCE_DNS_TIMEOUT_MS";
/// 跳过 IPv6 环境变量（"1"/"true" 启用）。
pub const ENV_NO_IPV6: &str = "EDUCE_NO_IPV6";

const DEFAULT_DNS_TIMEOUT_MS: u64 = 3000;
const MIN_DNS_TIMEOUT_MS: u64 = 500;
const MAX_DNS_TIMEOUT_MS: u64 = 8000;

/// 公共 DNS 列表（系统解析失败时的直连回退，UDP/TCP 53）。
const PUBLIC_DNS: &[IpAddr] = &[
    IpAddr::V4(Ipv4Addr::new(223, 5, 5, 5)),       // 阿里 AliDNS
    IpAddr::V4(Ipv4Addr::new(119, 29, 29, 29)),    // 腾讯 DNSPod
    IpAddr::V4(Ipv4Addr::new(114, 114, 114, 114)), // 114DNS
    IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),         // 谷歌（兜底）
];

/// 系统解析预算占比：60% 给 getaddrinfo，其余留给公共 DNS 回退。
const SYS_BUDGET_RATIO: f64 = 0.6;

/// IPv4 优先 + 解析超时 + 公共 DNS 回退的解析器。
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
            "DNS 解析器就绪：IPv4 优先 + 解析硬超时 + 公共 DNS 回退"
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
            // 1) 系统 getaddrinfo（常见网络下最快、最贴合本网 DNS）
            let sys_budget = timeout.mul_f64(SYS_BUDGET_RATIO);
            match tokio::time::timeout(sys_budget, tokio::net::lookup_host((host.as_str(), 0)))
                .await
            {
                Ok(Ok(addrs)) => {
                    let ordered = order_addresses(addrs, drop_ipv6, &host)?;
                    if !ordered.is_empty() {
                        return Ok(Box::new(ordered.into_iter()) as Addrs);
                    }
                    tracing::warn!(host, "系统解析无可用地址，回退公共 DNS");
                }
                Ok(Err(e)) => tracing::warn!(host, error = %e, "系统解析失败，回退公共 DNS"),
                Err(_) => tracing::warn!(host, "系统解析超时，回退公共 DNS"),
            }

            // 2) 公共 DNS 直连回退（绕开损坏的系统解析器）
            let fb_budget = timeout - sys_budget;
            match tokio::time::timeout(fb_budget, public_dns_lookup(&host, drop_ipv6)).await {
                Ok(Ok(addrs)) => Ok(addrs),
                Ok(Err(e)) => {
                    Err(format!("DNS 解析失败({host}): 系统解析与公共 DNS 均失败: {e}").into())
                }
                Err(_) => Err(format!("DNS 解析超时({host}) 超过 {timeout:?}").into()),
            }
        })
    }
}

/// 地址排序：IPv4 在前、IPv6 在后；`drop_ipv6` 时丢弃 IPv6。
/// 空结果返回 None（由调用方决定是否回退）。
fn order_addresses(
    addrs: impl IntoIterator<Item = SocketAddr>,
    drop_ipv6: bool,
    host: &str,
) -> Result<Vec<SocketAddr>, Box<dyn std::error::Error + Send + Sync>> {
    let mut v4 = Vec::new();
    let mut v6 = Vec::new();
    for addr in addrs {
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
    v4.extend(v6);
    Ok(v4)
}

/// 公共 DNS 直连查询（hickory-resolver，懒初始化单例）。
async fn public_dns_lookup(host: &str, drop_ipv6: bool) -> Result<Addrs, String> {
    static RESOLVER: OnceLock<TokioResolver> = OnceLock::new();
    let resolver = RESOLVER.get_or_init(|| {
        // 0.26 的 ResolverConfig 由 ServerGroup 构造（UDP + TCP）
        let config = ResolverConfig::udp_and_tcp(&ServerGroup {
            ips: PUBLIC_DNS,
            server_name: "",
            path: "",
        });
        let mut opts = ResolverOpts::default();
        opts.timeout = Duration::from_secs(2);
        opts.attempts = 1;
        opts.ip_strategy = if drop_ipv6 {
            LookupIpStrategy::Ipv4Only
        } else {
            LookupIpStrategy::Ipv4AndIpv6
        };
        TokioResolver::builder_with_config(config, TokioRuntimeProvider::default())
            .with_options(opts)
            .build()
            .expect("构建公共 DNS 解析器失败")
    });

    let lookup = resolver
        .lookup_ip(host)
        .await
        .map_err(|e| format!("公共 DNS 查询失败: {e}"))?;
    let mut v4 = Vec::new();
    let mut v6 = Vec::new();
    for ip in lookup.iter() {
        match ip {
            IpAddr::V4(_) => v4.push(SocketAddr::new(ip, 0)),
            IpAddr::V6(_) if !drop_ipv6 => v6.push(SocketAddr::new(ip, 0)),
            _ => {}
        }
    }
    if v4.is_empty() && v6.is_empty() {
        return Err("公共 DNS 无可用地址".into());
    }
    v4.extend(v6);
    Ok(Box::new(v4.into_iter()) as Addrs)
}
