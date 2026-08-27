# 更新日志

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/) 格式，版本号遵循[语义化版本](https://semver.org/lang/zh-CN/)。

## [Unreleased]

## [2.2.2] - 2026-08-27

### 修复

- **安卓 8.1 Termux 全源统一超时**（约 8.3s 均匀失败，`src/dns.rs` / `src/http.rs` / 各引擎）：
  - 新增自定义 DNS 解析器注入 reqwest：解析带硬超时（`EDUCE_DNS_TIMEOUT_MS`，默认 3000ms，挂起立即明示 `DNS 解析超时`）、IPv4 地址优先、`EDUCE_NO_IPV6=1` 可直接丢弃 IPv6
  - 主客户端与系统根证书回退客户端均接入该解析器
  - 引擎错误串联完整原因链（`error_detail`，anyhow chain / std source 逐层下钻），DNS / 连接 / TLS 哪层失败可直接在 UI 与 CLI 看到

### 文档

- README 新增 Termux 故障排查章节（curl / getent / ip route 三步定位 + 环境变量说明）。

## [2.2.1] - 2026-08-26

### 变更

- **Termux 使用体验优化**（`scripts/termux-setup.sh` / `scripts/build-termux.sh` / `Cargo.toml`）：
  - 新增轻量 `release-termux` 构建 profile（关闭 LTO、`codegen-units=16`），显著降低手机端构建内存占用与耗时，避免低内存设备 OOM
  - 脚本首次使用自动 `pkg update` 同步软件源，全新 Termux 也能直接安装依赖
  - 补装 `clang`（编译 ring 的 C 部分，`tls-rustls` 后端必需）
  - 按 `/proc/meminfo` 自动限制并行编译任务数（<4GB→2、4~8GB→4、≥8GB→8）
  - 自动安装 `termux-api` 并启用 `termux-wake-lock`，防止后台休眠断网

### 文档

- README 更新 Termux 构建表与使用说明。

## [2.2.0] - 2026-08-26

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
