# 更新日志

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/) 格式，版本号遵循[语义化版本](https://semver.org/lang/zh-CN/)。

## [Unreleased]

### 新增

- **TLS 信任链加固**（`src/tls.rs`，rustls 后端）：
  - 默认信任**内置 Mozilla 根证书**（`webpki-roots` 编译期快照），无需任何外部 CA 文件即可访问主流站点
  - **证书校验失败自动回退重连**：内置根证书过期/缺失导致校验失败时，自动改用系统根证书重试一次（Windows ROOT 证书库 / macOS keychain / Linux / Termux 系统 CA bundle，含 Termux `$PREFIX/etc/tls/cert.pem` 兜底）
  - **私有 CA 支持**：`EDUCE_CA_PEM` 环境变量指向 PEM 文件，根证书与**中间证书**一并注入信任锚集合；中间证书参与链构建，服务器即使不随链下发中间证书也能完成验证
- 新增 `CHANGELOG.md`。

### 变更

- `tls-rustls` feature 由 `reqwest/rustls-tls` 切换为 `reqwest/rustls-tls-no-provider`，出站 HTTPS 的证书信任链改由 `src/tls.rs` 统一控制；新增依赖 `webpki-roots`、`rustls`（ring）、`rustls-native-certs`、`rustls-pemfile`。
- `HttpClient` 增加系统根证书回退客户端池（含代理镜像），主池信任内置 Mozilla 根 + 私有 CA，回退池信任系统根 + 私有 CA。
- `tls-native` 后端（Windows schannel）本就信任系统 ROOT 证书库，无回退池；`EDUCE_CA_PEM` 同样生效（追加到系统根证书集）。

### 文档

- README 新增「TLS 信任链」一节，说明内置根证书、系统根回退与 `EDUCE_CA_PEM` 私有 CA 用法。
