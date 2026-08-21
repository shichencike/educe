//! 命令行入口解析（clap derive）。

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "educe",
    version,
    about = "Educe 元搜索引擎：多源聚合搜索服务（Web + REST API + CLI）",
    long_about = "聚合通用/代码/中文/学术四类共 18 个搜索源，去重合并、加权评分。\
                  \n静态编译：Linux musl / Windows / Termux 均可部署。"
)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// 启动 HTTP 服务（Web 页面 + REST API）
    Serve {
        /// 配置文件路径
        #[arg(long, default_value = "config.toml")]
        config: String,
    },
    /// 列出全部搜索源（启用状态、分类、权重）
    Sources {
        #[arg(long, default_value = "config.toml")]
        config: String,
    },
    /// 命令行直接执行一次聚合搜索
    Search {
        /// 搜索关键词
        query: String,
        #[arg(long, default_value = "config.toml")]
        config: String,
        /// 引擎白名单（逗号分隔），如 --sources bing,github,arxiv
        #[arg(long)]
        sources: Option<String>,
        /// 最多返回结果数
        #[arg(long, default_value_t = 20)]
        max: usize,
        /// 以 JSON 输出
        #[arg(long)]
        json: bool,
    },
    /// 输出默认配置到 stdout（可重定向为 config.toml）
    GenConfig,
}
