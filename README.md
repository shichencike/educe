# Educe 元搜索引擎

多源聚合的元搜索引擎：一次搜索并发请求多个搜索引擎/社区/学术源，**跨源去重合并、加权评分排序**，输出统一结果的 Web 页面 + REST API。

用 Rust 编写，**静态编译为单二进制**，可部署于 **Windows / Linux / Termux(Android)**，无任何运行时依赖；默认 TLS 用 rustls（无 OpenSSL），Windows 可选 schannel（零 C 依赖编译）。

## 特性

- 🌐 聚合 **18 个搜索源**（通用 / 代码技术 / 中文内容 / 学术媒体），模块化适配器，新增源只需一个文件
- 🔀 **并发调度**：各源独立执行，单源超时/失败只影响该源，其余正常返回
- 🧹 **去重合并**：URL 规范化（去跟踪参数/片段）后跨源合并，来源并列展示
- ⭐ **加权评分**：引擎权重 × 排名位置 × 标题命中关键词，多源佐证自动加分
- 🕶 **基础反爬**：UA 轮换、随机请求头、会话 cookie、每源限速、代理池（HTTP/SOCKS5 轮换）
- 🖥 **可选 JS 渲染桥**：知乎/CSDN/简书/微信公众号/Google 等需要执行 JS 的源，交给外部 Node+puppeteer 渲染后回传 HTML 解析，并带通用兜底提取器
- 📦 **单二进制分发**：内嵌单页前端（零静态文件）、Web + API + CLI 三合一

## 搜索源一览

| 分类 | id | 名称 | 需 JS 桥 |
| --- | --- | --- | --- |
| 通用 | baidu / bing / duckduckgo / sogou / so360 | 百度 / 必应 / DuckDuckGo / 搜狗 / 360 搜索 | 否 |
| 通用 | google | Google | 是（可回退轻量 HTML） |
| 代码 | github / stackoverflow / gitee / juejin | GitHub / Stack Overflow / Gitee / 掘金 | 否 |
| 中文 | zhihu / csdn / jianshu / wechat | 知乎 / CSDN / 简书 / 微信公众号 | 是 |
| 学术 | arxiv / pubmed / wikipedia / googlenews | arXiv / PubMed / 维基百科 / Google 新闻 | 否 |

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
| Linux (x86_64/ARM64) | `bash scripts/build-musl.sh` | musl 全静态，需 cargo-zigbuild 或 docker |
| Termux (Android) | `pkg install rust && bash scripts/build-termux.sh` | 本机构建（bionic） |
| 全平台 | 打 `v*` tag 推 GitHub，走 `.github/workflows/release.yml` | 自动产出三平台二进制 |

默认 feature（`tls-rustls`）用于 Linux/Termux 静态构建；Windows 用 `--no-default-features --features tls-native`（系统 schannel，不引入任何 C 依赖）。

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

### `GET /api/sources`

全部引擎元信息（id/名称/分类/needs_js/enabled/权重），前端据此渲染筛选器。带偏好 cookie 时，enabled/权重返回用户偏好。

### `GET /healthz`

健康检查，返回 `ok`。

## 用户偏好与设置页（SearXNG 风格）

`/settings.html` 提供浏览器端设置（无需登录），偏好存于 cookie `educe_prefs`（percent 编码 JSON）：

- **常规**：界面语言、主题（深/浅）、每页结果数（`max` 缺省值）、结果新标签打开、单源超时
- **引擎**：每个源独立 启用/禁用 与 权重（权重留空 = 用配置文件）

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
├── http.rs        # HTTP 客户端池、UA 轮换、代理池、限速器
├── jsrender.rs    # JS 渲染桥（外部命令渲染 HTML）
├── models.rs      # 数据模型
├── search.rs      # 聚合调度、去重合并、评分排序
├── server.rs      # axum API + 内嵌前端
├── engines/       # 18 个搜索源适配器
└── web/index.html # 单页前端
js-exec/           # Node 渲染脚本（puppeteer-core）
scripts/           # 各平台构建脚本
```

## 合规提示

各搜索源对抓取行为有自己的条款与反爬策略。本项目定位为个人/内网工具，默认频率保守；请控制请求频率、遵守目标站点 robots.txt 与服务条款，勿用于商业爬取。代理池、UA 轮换等能力用于规避误封与限速，请合法合理使用。
