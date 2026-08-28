# 更新日志

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/) 格式，版本号遵循[语义化版本](https://semver.org/lang/zh-CN/)。

## [2.3.0] - 2026-08-28

### 新增

- **设置页可视化操作**（`src/web/settings.html`，保持 Chrome 70 兼容）：
  - 复选框全部改为 **Switch 开关**样式（新标签页 / 代理池 / JS 桥 / 自定义引擎需 JS / 每引擎启用）
  - **主题色板**：7 色可视化点选（蓝/紫/绿/橙/粉/青/红），`data-accent` 驱动 CSS 变量，设置页实时预览、保存后搜索页同步生效（`UserPrefs` 新增 `accent` 字段，cookie 持久化）
  - **代理连通性测试**按钮：用当前填写（未保存）的配置实测代理是否可用，显示耗时与结果
  - **自定义引擎解析预览**：填写选择器后输入测试关键词，实时预览解析出的结果卡片
  - **配置备份导入/导出**：一键导出当前完整配置为 JSON 下载；选择文件导入，运行时部分立即生效
- **后端可视化辅助接口**（`src/server.rs`）：
  - `POST /api/runtime/test`：代理连通性测试（构建临时客户端实测，不保存、不影响现有连接）
  - `POST /api/engines/custom/preview`：自定义引擎临时执行搜索并返回解析结果
  - `GET /api/config/export` / `POST /api/config/import`：配置备份与恢复（运行时立即生效；静态部分写入 `config.toml`，原文件备份为 `config.toml.bak`；自定义引擎即时加载并持久化）
  - `AppConfig` 系列结构体补 `Serialize`（导出需要）

### 修复

- **聚合缓存窗口切片越界 panic**（`src/search.rs`）：`offset` 超出结果总数（如直接请求 `offset=999999`）时，缓存窗口切片 `ranked[win_base..win_end]` 越界直接崩溃；现收敛到 `total` 并跳过空窗口，超界分页请求不再打挂服务。

### 变更

- **后端性能优化**（`src/search.rs` / `src/http.rs` / `src/server.rs`）：
  - 评分关键词匹配改为一次分词进 `HashSet`、O(1) 查集（原实现 O(词数 × 文本长度)），查询词预处理外提，循环内不再重复计算
  - 搜索建议 DuckDuckGo / 百度**并发**查询 + 各自 3s 硬超时：DDG 失败即时取百度结果，不再串行等一轮
  - 限速器热路径零分配：引擎已预注册时直接命中桶，不再每次 `acquire` 构造 String 键
  - 聚合结果容量预分配（按引擎数 × 每源上限），避免多次扩容
  - 新增 **gzip 响应压缩**（`tower-http` 依赖）：首屏 HTML 与搜索结果 JSON 传输体积显著减小；自动跳过 `text/event-stream`，不影响 SSE 流式渐进渲染

### 前端

- **搜索页 UI/UX 优化**（`src/web/index.html`，保持 Chrome 70 兼容）：
  - 搜索后 hero 紧凑态（SearXNG 风格：logo 收缩让位给结果）
  - 搜索按钮忙碌态（搜索期间禁用并显示"搜索中…"）
  - 回到顶部浮动按钮；`Ctrl/Cmd+K` / `/` 快捷聚焦搜索框
  - 流式搜索实时进度：已返回 X/Y 源 · 累计 N 条 · 已用 X.Xs
  - 主题色 `theme-color` meta 随深浅色同步（移动端地址栏配色）
  - 键盘焦点环 + `prefers-reduced-motion` 减少动效
  - 骨架屏条数随每页结果数伸缩
- **搜索页渲染性能优化**：
  - 关键词高亮合并为**单个交替正则**，一次 `replace` 完成全部高亮（O(词数 × 长度) → O(长度)），长词优先避免误匹配
  - 引擎状态行改为**增量更新**：每源完成只更新对应一行，不再 JSON 序列化 + 全量重建列表
  - 流式结果累积由 `concat` 改 `push` 原地扩展（O(n²) → O(n)）
  - IndexedDB 连接**复用**：首次 open 后缓存连接，翻页/写缓存不再反复建连
- **设置页**（`src/web/settings.html`）：恢复默认增加确认弹窗、主题色 meta 同步。

### 文档

- README 特性列表更新：并发搜索建议、gzip 响应压缩、前端新交互与渲染性能优化、设置可视化操作说明。

## [2.2.3] - 2026-08-27

### 修复

- **系统 DNS 解析挂起时自动回退公共 DNS 直连查询**（`src/dns.rs`，新增 `hickory-resolver` 依赖）：
  - 系统 getaddrinfo 失败/超时（UI 显示 `DNS 解析超时`）后，自动改用阿里 `223.5.5.5` / DNSPod `119.29.29.29` / 114DNS / 谷歌 `8.8.8.8` 直连查询，绕开损坏的系统解析器（Termux resolv.conf 指向不可达 DNS 时仍可解析）
  - 解析预算 60% 给系统解析、其余留给公共 DNS 回退，整体仍受 `EDUCE_DNS_TIMEOUT_MS`（默认 3000ms）硬超时约束
  - 公共 DNS 回退路径同样保持 IPv4 优先与 `EDUCE_NO_IPV6=1` 过滤

### 文档

- README Termux 故障排查章节补充 `DNS 解析超时` 症状解读与公共 DNS 回退说明。

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
