mod cache;
mod cli;
mod config;
mod dns;
mod custom;
mod engines;
mod http;
mod jsrender;
mod models;
mod prefs;
mod runtime;
mod search;
mod server;
mod tls;

use std::sync::Arc;

use clap::Parser;

use crate::cli::{Cli, Cmd};
use crate::config::AppConfig;
use crate::search::Aggregator;

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::GenConfig => {
            print!("{}", config::DEFAULT_CONFIG_TOML);
            Ok(())
        }
        Cmd::Serve { config } => {
            let cfg = Arc::new(AppConfig::load(Some(&config))?);
            init_tracing(&cfg.logging.level);
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            rt.block_on(server::serve(cfg))
        }
        Cmd::Sources { config } => {
            let cfg = Arc::new(AppConfig::load(Some(&config))?);
            init_tracing(&cfg.logging.level);
            let agg = Aggregator::new(cfg)?;
            println!(
                "{:<14} {:<12} {:<10} {:<7} {:<5} 权重",
                "ID", "名称", "分类", "需要JS", "启用"
            );
            for info in agg.source_infos() {
                println!(
                    "{:<14} {:<12} {:<10} {:<7} {:<5} {:.1}",
                    info.id,
                    info.name,
                    info.category,
                    if info.needs_js { "是" } else { "否" },
                    if info.enabled { "是" } else { "否" },
                    info.weight
                );
            }
            Ok(())
        }
        Cmd::Search {
            query,
            config,
            sources,
            max,
            json,
        } => {
            let cfg = Arc::new(AppConfig::load(Some(&config))?);
            init_tracing(&cfg.logging.level);
            let filter: Option<Vec<String>> = sources.as_ref().map(|s| {
                s.split(',')
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect()
            });
            let agg = Aggregator::new(cfg)?;
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            let resp = rt.block_on(agg.search(&query, filter.as_deref(), 0, max, None));

            if json {
                println!("{}", serde_json::to_string_pretty(&resp)?);
                return Ok(());
            }

            println!(
                "查询「{}」：共 {} 条结果，耗时 {}ms，来源 {} 个\n",
                resp.query,
                resp.total,
                resp.time_ms,
                resp.engines.len()
            );
            for (i, r) in resp.results.iter().enumerate() {
                println!(
                    "{}.[{}] {}\n   {}\n   来源: {}  {}\n",
                    i + 1,
                    r.source,
                    r.title,
                    r.url,
                    r.published.clone().unwrap_or_default(),
                    if r.snippet.is_empty() {
                        String::new()
                    } else {
                        format!("\n   {}", r.snippet)
                    }
                );
            }
            for e in &resp.engines {
                if let Some(err) = &e.error {
                    eprintln!("[{}] ✗ {err}", e.id);
                } else {
                    println!("[{}] ✓ {} 条, {}ms", e.id, e.count, e.time_ms);
                }
            }
            Ok(())
        }
    }
}

/// 依据配置初始化 tracing（可用 RUST_LOG 环境变量覆盖）。
fn init_tracing(level: &str) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
