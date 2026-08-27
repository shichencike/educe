# Educe 元搜索引擎

多源聚合的元搜索引擎：一次搜索并发请求多个搜索引擎/社区/学术源，**跨源去重合并、加权评分排序**，输出统一结果的 Web 页面 + REST API。

用 Rust 编写，**静态编译为单二进制**，可部署于 **Windows / Linux / Termux(Android)**，无任何运行时依赖；默认 TLS 用 rustls（无 OpenSSL），Windows 可选 schannel（零 C 依赖编译）。

## 特性

- 🌐 聚合 **20 个搜索源**（通用 / 代码技术 / 中文内容 / 学术媒体），模块化适配器，新增源只需一个文件
- 🔀 **并发调度**：各源独立执行，单源超时/失败只影响该源，其余正常返回
- 🧹 **去重合并**：URL 规范化（**跳转链还原** + 去跟踪参数/片段）后跨源合并，来源并列展示，前端点击直达真实地址
- ⭐ **加权评分**：引擎权重 × 排名位置 × 标题/摘要命中关键词（英文词边界匹配），多源佐证自动加分
- ✂️ **标题清洗**：自动去除 "xxx - 知乎 / _CSDN博客" 等站点后缀，摘要跨源择优取最长
- ⚡ **SSE 流式响应**：`/api/search/stream` 逐引擎推送，前端渐进渲染，首屏大幅提前（不支持 SSE 时自动降级为普通接口）
- 🔮 **搜索建议**：`/api/suggest` 代理 DuckDuckGo/百度建议接口，前端输入框实时下拉补全
- 🗄 **TTL 结果缓存**：相同查询 5 分钟内直接命中，省配额、降延迟（有用户权重覆盖时自动跳过）
- 🕶 **基础反爬**：UA 轮换、随机请求头、会话 cookie、每源限速、代理池（HTTP/SOCKS5 轮换）
- 🖥 **可选 JS 渲染桥**：知乎/CSDN/简书/微信公众号/Google 等需要执行 JS 的源，交给外部 Node+puppeteer 渲染后回传 HTML 解析，并带通用兜底提取器
- 📱 **SearXNG 风格前端**：搜索建议下拉、即时搜索防抖、键盘导航（↑↓ 选建议 / ←→ 翻页 / Enter 搜索）、分页、结果卡片增强（域名/关键词高亮/来源徽章）、响应式移动端，兼容安卓 8.1（Chrome 70 级 WebView）
- 📦 **单二进制分发**：内嵌单页前端（零静态文件）、Web + API + CLI 三合一

## 搜索源一览

| 分类 | id | 名称 | 需 JS 桥 |
| --- | --- | --- | --- |
| 通用 | baidu / bing / duckduckgo / sogou / so360 / startpage | 百度 / 必应 / DuckDuckGo / 搜狗 / 360 搜索 / Startpage | 否 |
| 通用 | google | Google | 是（可回退轻量 HTML） |
| 代码 | github / stackoverflow / gitee / juejin | GitHub / Stack Overflow / Gitee / 掘金 | 否 |
| 中文 | zhihu / csdn / jianshu / wechat | 知乎 / CSDN / 简书 / 微信公众号 | 是 |
| 学术 | arxiv / pubmed / wikipedia / googlenews / xueshu | arXiv / PubMed / 维基百科 / Google 新闻 / 百度学术 | 否 |

## 快速开始

### 1. 生成配置

```bash
educe gen-config > config.toml   # 按需编辑
```

### 2. 启动服务

```bash
educe serve --config config.toml
# 打开 http://127.0.0.1:8080
```

### 3. 命令行直接搜

```bash
educe search "rust 异步" --sources bing,github,arxiv --max 30
educe search "rust 异步" --json          # 输出 JSON
educe sources                            # 查看引擎清单
```

## 构建（静态编译）

> Windows 本机开发验证（无需任何 C 编译器）：
> `cargo check --no-default-features --features tls-native`

| 平台 | 命令 | 说明 |
| --- | --- | --- |
| Windows | `powershell -File scripts/build-windows.ps1` | schannel TLS，零 C 依赖，产出单个 exe |
| Linux (x86_64/ARM64) | `bash scripts/build-musl.sh` | musl 全静态，需 cargo-zigbuild + zig |
| Termux (Android) | `pkg install rust binutils clang && bash scripts/build-termux.sh` | 本机构建（bionic，轻量 release-termux profile） |
| 全平台 | 打 `v*` tag 推 GitHub，走 `.github/workflows/release.yml` | 自动产出三平台二进制 |

默认 feature（`tls-rustls`）用于 Linux/Termux 静态构建；Windows 用 `--no-default-features --features tls-native`（系统 schannel，不引入任何 C 依赖）。

## Termux（Android）使用体验

支持 **安卓 8.1 及以上**（Chrome 70 级 WebView，前端已兼容）。推荐在 Termux 内一键安装并启动：

```bash
# 1. 安装 bash（首次）
pkg install bash
# 2. 一键：装依赖 + bionic 本机构建 + 生成配置 + 启动
bash scripts/termux-setup.sh
```

脚本会自动完成：

- 自动安装 `rust` / `binutils` / `clang`，**bionic 本机构建**（无需交叉编译，天然适配安卓 8.1）
- 首次使用自动 `pkg update` 同步软件源（全新 Termux 也能直接装包）
- 用轻量 `release-termux` profile 构建，并按内存自动限制并行度，避免低内存手机 OOM
- 首次自动生成 `config.toml` 并把 `host` 设为 `0.0.0.0` —— 手机与电脑同一 Wi-Fi 即可访问
- 自动安装 `termux-api` 并启用 `termux-wake-lock` 防止后台休眠断网（仍需在系统安装 Termux:API 应用）
- 打印本机 / 局域网访问地址

常用参数：

```bash
bash scripts/termux-setup.sh --no-build          # 已构建过，仅启动
bash scripts/termux-setup.sh --port 9000         # 指定端口
bash scripts/termux-setup.sh --install-service   # 注册 termux-services 开机自启
```

开机自启（`--install-service`）注册后：`sv up educe` 启用、`sv down educe` 禁用、`sv status educe` 查看状态。自启服务默认按 `config.toml` 的 `host` 绑定，如需开机后局域网访问请保持 `host = "0.0.0.0"`。

> 若已用 CI 发布的 `linux-aarch64` 静态产物（`educe-linux-aarch64`），也可直接 `chmod +x` 后运行，无需本机构建；但本机 bionic 构建对安卓 8.1 兼容性最稳。

### Termux 故障排查：全部搜索源统一超时

症状：所有源约 **8 秒**后失败（`4s 连接超时 × 2 次尝试 + 0.3s 退避`），电脑上正常。这通常不是引擎问题，而是手机网络层：系统 DNS 解析挂起，或 DNS 把 IPv6 排前而移动网络的 IPv6 链路不通。Educe 已内置对策（启动日志可见 `DNS 解析器就绪：IPv4 优先 + 解析硬超时`）：

- **IPv4 优先**：解析结果把 IPv4 排前，IPv4 可用时立即建连，不被坏掉的 IPv6 拖累
- **解析硬超时**：系统 getaddrinfo 挂起超过 `EDUCE_DNS_TIMEOUT_MS`（默认 3000ms）立即失败并明示 `DNS 解析超时`
- **`EDUCE_NO_IPV6=1`**：IPv6 链路完全不可用时直接丢弃 IPv6 地址（终极手段，NAT64 / 纯 IPv6 网络请勿开启）

先分清是哪一层的问题，在 Termux 里依次执行：

```bash
# 1) 网络可达性（curl 有 happy-eyeballs，能自动 IPv4/IPv6 回退）
curl -v --connect-timeout 5 -m 10 https://www.baidu.com
#    成功 → 网络通，问题在 Educe 的解析/建连顺序，重启后即生效
#    超时 → 手机出站 TCP 被拦（防火墙 App / 受限网络 / 无外网），先修设备

# 2) DNS 解析（看是否返回 IPv4、顺序如何）
getent ahosts www.baidu.com
#    长时间无输出 → 系统 DNS 不可用：可把解析器指到公共 DNS 再重启 Educe
#    nameserver 223.5.5.5  # 追加到 $PREFIX/etc/resolv.conf（Termux 重启可能被覆盖）
#    只出 IPv6 且 1) 超时 → 用 EDUCE_NO_IPV6=1 启动

# 3) 路由（确认 IPv6 默认路由是否存在）
ip route; ip -6 route
```

若 1) 中 curl 成功而 Educe 仍全部超时，请带上 `RUST_LOG=debug ./target/release-termux/educe serve` 的日志（现在错误已串联完整原因链，如 `请求失败(baidu): url ← ... ← tcp connect error: Operation timed out`，能直接看出 DNS / 连接 / TLS 哪一层失败）反馈到 Issue。

## 配置说明（config.example.toml）

```toml
[server]
host = "127.0.0.1"     # 对外提供服务请改 0.0.0.0
port = 8080

[search]
max_per_source = 30    # 每源最多结果数（覆盖优先调大）
timeout_ms = 10000     # 单源超时
max_results = 100      # 聚合后返回上限
dedup = true           # 跨源去重合并
max_concurrent = 8     # 同时请求的源数量上限

[cache]                # 聚合结果 TTL 缓存
enabled = true
ttl_seconds = 300      # 缓存有效期（秒）
max_entries = 200      # 最大条目数（0 = 不限制）

[proxy]                # 代理池：http/https/socks5，轮换或随机
enabled = false
urls = ["http://127.0.0.1:7890"]
rotate = "round_robin" # round_robin | random

[rate_limit]           # 每源限速（次/分钟），default 兜底
default = 30
baidu = 10

[weights]              # 引擎权重，影响跨源排序
arxiv = 1.2

[js_render]            # JS 渲染桥（见下文）
enabled = false
command = "node js-exec/render.js"
sources = ["zhihu", "csdn", "jianshu", "wechat", "google"]

[engines]              # 启用白名单，不写则全部启用
enabled = ["baidu", "bing", "..."]
```

**环境变量覆盖**：任何配置项都可用 `EDUCE_` + 双下划线层级覆盖，如 `EDUCE_SERVER__PORT=9000`、`EDUCE_PROXY__ENABLED=true`。

## TLS 信任链（内置 Mozilla 根 / 系统根回退 / 私有 CA）

出站 HTTPS 请求的证书信任由 `src/tls.rs` 统一管理（`tls-rustls` 后端）：

- **内置 Mozilla 根证书**：默认信任 `webpki-roots` 编译期快照，无需任何外部 CA 文件即可访问主流站点
- **证书校验失败自动回退**：内置根证书过期/缺失导致校验失败时，自动改用**系统根证书**重连一次——
  Windows ROOT 证书库（schannel）、macOS keychain、Linux / Termux 系统 CA bundle
  （含 Termux `$PREFIX/etc/tls/cert.pem` 兜底）
- **私有 CA（`EDUCE_CA_PEM` 环境变量）**：指向一个 PEM 文件，可同时包含**根证书与中间证书**；
  中间证书会被注入信任锚集合参与链构建，服务器即使不随链下发中间证书也能完成验证

```bash
# 信任私有 CA（根证书 + 中间证书可放在同一个 PEM 文件）
export EDUCE_CA_PEM=/path/to/private-ca.pem
educe serve --config config.toml
```

> 说明：
> - `tls-native` 后端（Windows schannel 默认）本就只信任系统根证书，无需回退；`EDUCE_CA_PEM`
>   同样生效（追加到系统根证书集）。
> - `EDUCE_CA_PEM` 文件解析失败或没有可用作信任锚的证书时，服务启动会报错退出（不静默降级）。

## JS 渲染桥（可选）

对需要执行 JS 的源（知乎/CSDN/简书/微信/Google），Rust 侧会调用外部命令渲染页面再解析：

```bash
cd js-exec && npm i        # 安装 puppeteer-core
export CHROME_PATH=/path/to/chrome   # 指定本机 Chrome/Chromium
```

然后配置 `[js_render] enabled = true`。若专用选择器失效，会自动启用"通用兜底提取器"尽量捞取结果。

## REST API

### `GET /api/search?q=<关键词>&sources=<id1,id2>&max=50&offset=0`

```json
{
  "query": "rust 异步",
  "total": 42,
  "time_ms": 3200,
  "cached": true,
  "results": [
    {
      "title": "…",
      "url": "https://…",
      "snippet": "…",
      "source": "bing,github",
      "rank": 0,
      "score": 1.73,
      "published": "2026-08-01"
    }
  ],
  "engines": [
    { "id": "bing", "count": 12, "time_ms": 800, "error": null },
    { "id": "sogou", "count": 0, "time_ms": 900, "error": "无结果或触发反爬" }
  ]
}
```

- `sources` 留空 = 全部启用源；`error` 非空表示该源失败（不影响其他源）
- 结果 `source` 为逗号分隔的合并来源列表
- `cached` 为 true 表示命中 TTL 缓存（`engines` 为空数组）；未命中/未启用时该字段省略
- 结果 `url` 已做**跳转链还原**（百度/Google/Bing/搜狗/360/知乎/CSDN 等的 link?url= 跳转还原为真实目标地址）

### `GET /api/search/stream?q=…&sources=…&max=50&offset=0`（SSE）

流式接口：每完成一个源推送一条 `engine` 事件，全部完成后推送 `done` 事件（含最终排序分页结果）。前端据此渐进渲染，首屏显著提前；不支持 SSE 的客户端可回退普通接口。

```
data: {"type":"engine","id":"bing","count":12,"time_ms":800,"error":null,"retried":false,"results":[…]}
data: {"type":"done","total":42,"time_ms":3200,"results":[…]}
```

### `GET /api/suggest?q=<关键词>`

搜索建议（自动补全），代理 DuckDuckGo/百度建议接口：

```json
{ "query": "rust", "suggestions": ["rust 教程", "rust 语言", "…"] }
```

### `GET /api/sources`

全部引擎元信息（id/名称/分类/needs_js/enabled/权重），前端据此渲染筛选器。带偏好 cookie 时，enabled/权重返回用户偏好。

### `GET /healthz`

健康检查，返回 `ok`。

## 用户偏好与设置页（SearXNG 风格）

`/settings.html` 提供浏览器端设置（无需登录），偏好存于 cookie `educe_prefs`（percent 编码 JSON）：

- **常规**：界面语言、主题（深/浅）、每页结果数（`max` 缺省值）、结果新标签打开、单源超时
- **引擎**：每个源独立 启用/禁用 与 权重（权重留空 = 用配置文件），支持一键全选/全不选

| 接口 | 说明 |
| --- | --- |
| `GET /api/prefs` | 读取当前生效偏好（无 cookie 时返回配置默认） |
| `POST /api/prefs` | 保存偏好（JSON body，部分字段覆盖），写入 cookie |
| `DELETE /api/prefs` | 清除偏好 cookie（恢复默认） |

`/api/search` 会应用偏好：`max` 缺省取每页结果数、`sources` 缺省取启用的引擎、权重覆盖配置、单源超时可调。CLI 与 API 调用方不受 cookie 影响。

## 扩展新搜索源

1. 新建 `src/engines/<id>.rs`，实现 `Engine` trait（`meta()` + `search()`）
2. 在 `src/engines/mod.rs` 的 `all_metas()` 与 `build()` 中注册
3. 可选：在 `config.example.toml` 的 `[weights]` / `[rate_limit]` 中给出推荐值

## 目录结构

```
src/
├── main.rs        # CLI 入口（serve/sources/search/gen-config）
├── config.rs      # 配置加载（默认值+文件+环境变量覆盖）
├── cache.rs       # 聚合结果 TTL 内存缓存
├── http.rs        # HTTP 客户端池、UA 轮换、代理池、限速器
├── jsrender.rs    # JS 渲染桥（外部命令渲染 HTML）
├── models.rs      # 数据模型
├── search.rs      # 聚合调度（含 SSE 流式）、去重合并、评分排序、搜索建议
├── server.rs      # axum API（含 /api/search/stream SSE）+ 内嵌前端
├── engines/       # 20 个搜索源适配器
└── web/index.html # 单页前端（SearXNG 风格，兼容安卓 8.1）
js-exec/           # Node 渲染脚本（puppeteer-core）
scripts/           # 各平台构建脚本
```

## 合规提示

各搜索源对抓取行为有自己的条款与反爬策略。本项目定位为个人/内网工具，默认频率保守；请控制请求频率、遵守目标站点 robots.txt 与服务条款，勿用于商业爬取。代理池、UA 轮换等能力用于规避误封与限速，请合法合理使用。
