//! TLS 信任链配置：内置 Mozilla 根证书 + 系统根证书回退 + 私有 CA 注入。
//!
//! - 默认信任内置 Mozilla 根证书（`webpki-roots`，编译期快照，无需外部文件）
//! - 证书校验失败时自动回退系统根证书（Windows ROOT 证书库 / macOS keychain /
//!   Linux / Termux CA bundle），防止内置根证书过期导致连接失败
//! - 私有 CA：`EDUCE_CA_PEM` 环境变量指向 PEM 文件，其中根证书与**中间证书**
//!   都会被注入信任锚集合——服务器即使不随链下发中间证书也能完成链构建验证

#[cfg(feature = "tls-rustls")]
use std::path::Path;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};

/// 私有 CA 信任文件的环境变量名（PEM，可同时含根证书与中间证书）。
pub const ENV_CA_PEM: &str = "EDUCE_CA_PEM";

/// 私有 CA 文件路径（`EDUCE_CA_PEM`），未设置返回 None。
pub fn ca_pem_path() -> Option<PathBuf> {
    std::env::var_os(ENV_CA_PEM)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// 读取 PEM 文件中的所有证书（DER 字节列表）。
#[cfg(feature = "tls-rustls")]
pub fn read_pem_certs(path: &Path) -> Result<Vec<Vec<u8>>> {
    let data =
        std::fs::read(path).with_context(|| format!("读取 PEM 文件失败: {}", path.display()))?;
    let mut reader = std::io::BufReader::new(data.as_slice());
    let certs = rustls_pemfile::certs(&mut reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("解析 PEM 文件失败: {}", path.display()))?;
    if certs.is_empty() {
        bail!("PEM 文件中没有任何证书: {}", path.display());
    }
    Ok(certs.into_iter().map(|c| c.to_vec()).collect())
}

/// rustls 后端（tls-rustls feature，Linux / Termux / 默认构建）。
#[cfg(feature = "tls-rustls")]
pub mod rustls_backend {
    use super::*;
    use rustls::pki_types::CertificateDer;
    use rustls::RootCertStore;

    /// 信任锚来源。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum TrustSource {
        /// 内置 Mozilla 根证书（webpki-roots）
        Builtin,
        /// 系统根证书（Windows ROOT 库 / macOS keychain / Linux / Termux CA bundle）
        System,
    }

    /// 构建根证书库：指定来源 + 私有 CA（`EDUCE_CA_PEM`）注入。
    pub fn build_root_store(source: TrustSource) -> Result<RootCertStore> {
        let mut store = RootCertStore::empty();
        let (added, skipped) = match source {
            TrustSource::Builtin => {
                store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
                (webpki_roots::TLS_SERVER_ROOTS.len(), 0)
            }
            TrustSource::System => load_system_roots(&mut store)?,
        };
        tracing::debug!(?source, added, skipped, "TLS 信任锚就绪");
        inject_ca_pem(&mut store)?;
        Ok(store)
    }

    /// 加载系统根证书：rustls-native-certs（Windows ROOT 库 / macOS keychain /
    /// Linux 标准路径），并补 Termux `$PREFIX/etc/tls/cert.pem` 兜底。
    fn load_system_roots(store: &mut RootCertStore) -> Result<(usize, usize)> {
        let native = rustls_native_certs::load_native_certs();
        let mut added = native.certs.len();
        let mut skipped = native.errors.len();
        for err in &native.errors {
            tracing::debug!("系统根证书加载失败(跳过): {err}");
        }
        store.add_parsable_certificates(native.certs);

        // Termux 兜底：rustls-native-certs 可能命中不了 $PREFIX/etc/tls/cert.pem
        if let Some(bundle) = termux_bundle_path() {
            if let Ok(data) = std::fs::read(&bundle) {
                let mut reader = std::io::BufReader::new(data.as_slice());
                let parsed = rustls_pemfile::certs(&mut reader).collect::<Vec<_>>();
                let (a, s) = store.add_parsable_certificates(parsed.into_iter().flatten());
                added += a;
                skipped += s;
            }
        }
        Ok((added, skipped))
    }

    /// Termux 的 CA bundle：`$PREFIX/etc/tls/cert.pem`。
    fn termux_bundle_path() -> Option<PathBuf> {
        let prefix = std::env::var_os("PREFIX").filter(|v| !v.is_empty())?;
        Some(PathBuf::from(prefix).join("etc/tls/cert.pem"))
    }

    /// 将私有 CA（`EDUCE_CA_PEM`）的根证书与中间证书注入信任锚集合。
    /// 中间证书与根证书一样作为信任锚参与链构建，服务器不随链下发也能完成验证。
    fn inject_ca_pem(store: &mut RootCertStore) -> Result<()> {
        let Some(path) = ca_pem_path() else {
            return Ok(());
        };
        let der_certs = read_pem_certs(&path)?;
        let (added, ignored) =
            store.add_parsable_certificates(der_certs.into_iter().map(CertificateDer::from));
        tracing::info!(path = %path.display(), added, ignored, "私有 CA 证书已注入信任锚");
        if added == 0 {
            bail!("私有 CA 文件没有可用作信任锚的证书: {}", path.display());
        }
        Ok(())
    }

    /// 判定错误是否为证书校验失败（错误链上存在 rustls 证书错误）。
    /// 用于触发"回退系统根证书重连"。
    pub fn is_certificate_error(err: &anyhow::Error) -> bool {
        err.chain().any(|cause| {
            matches!(
                cause.downcast_ref::<rustls::Error>(),
                Some(rustls::Error::InvalidCertificate(_))
            )
        })
    }
}

/// native-tls 后端（tls-native feature，Windows schannel / macOS / Linux）。
#[cfg(feature = "tls-native")]
pub mod native_backend {
    use super::*;

    /// 私有 CA（`EDUCE_CA_PEM`）→ reqwest 根证书列表，供 `add_root_certificate` 使用。
    /// schannel / SecurityFramework / OpenSSL 后端均支持追加根证书。
    pub fn ca_certificates() -> Result<Vec<reqwest::tls::Certificate>> {
        let Some(path) = ca_pem_path() else {
            return Ok(Vec::new());
        };
        let data = std::fs::read(&path)
            .with_context(|| format!("读取私有 CA 文件失败: {}", path.display()))?;
        let certs = reqwest::tls::Certificate::from_pem_bundle(&data)
            .with_context(|| format!("解析私有 CA 文件失败: {}", path.display()))?;
        if certs.is_empty() {
            bail!("私有 CA 文件没有证书: {}", path.display());
        }
        tracing::info!(path = %path.display(), count = certs.len(), "私有 CA 证书已加载");
        Ok(certs)
    }
}
