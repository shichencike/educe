//! 简单 TTL 内存缓存：聚合搜索的最终排序结果按 (query, sources) 缓存。
//!
//! 内存优化：为避免 20000 条 × 多查询占满内存，每个缓存条目只保存
//! 一个**滑动窗口**（默认当前页 ±1 页，即 3 页结果）而非整份排序列表；
//! 请求的 [offset, offset+len) 落在窗口内才命中，落在窗口外则视为未命中
//! （由调用方重新聚合，并按新 offset 刷新窗口）。
//! 其余页面的"本地暂存"由浏览器端 localStorage 承担（见前端）。
//!
//! 线程安全：内部 `Mutex<HashMap>`；条目含过期时间与插入序号，
//! 插入超限时淘汰最旧条目（FIFO 近似）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::models::SearchResult;

/// 缓存条目：完整排序列表中的一段连续窗口。
struct CacheEntry {
    /// 窗口第一个结果在完整排序列表中的偏移
    base: usize,
    /// 完整排序列表总条数
    total: usize,
    /// 窗口内结果（base 起的连续切片，长度 ≤ 3 页）
    results: Arc<Vec<SearchResult>>,
    expires: Instant,
    order: u64,
}

/// 线程安全的 TTL 窗口缓存。
#[derive(Clone, Default)]
pub struct SearchCache {
    inner: Arc<Mutex<HashMap<String, CacheEntry>>>,
    /// 插入序号（用于淘汰最旧）
    counter: Arc<AtomicU64>,
    /// 条目 TTL
    ttl: Duration,
    /// 最大条目数（0 = 不限制）
    max_entries: usize,
}

impl SearchCache {
    pub fn new(ttl: Duration, max_entries: usize) -> Self {
        SearchCache {
            inner: Arc::new(Mutex::new(HashMap::new())),
            counter: Arc::new(AtomicU64::new(0)),
            ttl,
            max_entries,
        }
    }

    pub fn enabled(&self) -> bool {
        !self.ttl.is_zero()
    }

    /// 命中返回缓存窗口（未过期），否则 None。
    pub fn get(&self, key: &str) -> Option<(usize, usize, Arc<Vec<SearchResult>>)> {
        if !self.enabled() {
            return None;
        }
        let mut map = self.inner.lock().unwrap();
        let now = Instant::now();
        match map.get(key) {
            Some(e) if e.expires > now => Some((e.base, e.total, e.results.clone())),
            _ => {
                map.remove(key); // 过期条目清理
                None
            }
        }
    }

    /// 写入缓存窗口：`results` 是完整列表 `[base, base+len)` 的切片。
    /// 超限时先清理过期条目，再淘汰最旧。
    pub fn set(&self, key: String, base: usize, total: usize, results: Vec<SearchResult>) {
        if !self.enabled() {
            return;
        }
        let order = self.counter.fetch_add(1, Ordering::Relaxed);
        let mut map = self.inner.lock().unwrap();

        // 已存在则直接覆盖（刷新 TTL）
        map.insert(
            key.clone(),
            CacheEntry {
                base,
                total,
                results: Arc::new(results),
                expires: Instant::now() + self.ttl,
                order,
            },
        );

        if self.max_entries > 0 && map.len() > self.max_entries {
            // 先清过期
            let now = Instant::now();
            map.retain(|_, e| e.expires > now);
            // 仍超限：淘汰 order 最小的若干条
            if map.len() > self.max_entries {
                let mut orders: Vec<u64> = map.values().map(|e| e.order).collect();
                orders.sort_unstable();
                let cutoff = orders[map.len() - self.max_entries];
                map.retain(|_, e| e.order >= cutoff);
            }
        }
    }

    /// 清空缓存（如运行时设置变更后调用）。
    #[allow(dead_code)]
    pub fn clear(&self) {
        self.inner.lock().unwrap().clear();
    }

    /// 当前条目数（调试用）。
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn res(url: &str) -> Vec<SearchResult> {
        vec![SearchResult::new(
            "t".into(),
            url.into(),
            "".into(),
            "bing",
            0,
        )]
    }

    fn window(urls: &[&str]) -> Vec<SearchResult> {
        urls.iter()
            .map(|u| SearchResult::new("t".into(), u.to_string(), "".into(), "bing", 0))
            .collect()
    }

    #[test]
    fn disabled_when_ttl_zero() {
        let c = SearchCache::new(Duration::ZERO, 10);
        assert!(!c.enabled());
        assert!(c.get("k").is_none());
    }

    #[test]
    fn set_get_roundtrip() {
        let c = SearchCache::new(Duration::from_secs(60), 10);
        c.set("k".into(), 50, 500, res("https://a.com"));
        let (base, total, got) = c.get("k").expect("应命中");
        assert_eq!(base, 50);
        assert_eq!(total, 500);
        assert_eq!(got[0].url, "https://a.com");
    }

    #[test]
    fn window_keeps_three_page_slice() {
        let c = SearchCache::new(Duration::from_secs(60), 10);
        // 窗口 = 完整列表 [50, 50+3) 三页（每页 1 条）
        c.set(
            "k".into(),
            50,
            100,
            window(&["https://a.com", "https://b.com", "https://c.com"]),
        );
        let (base, total, got) = c.get("k").expect("应命中");
        assert_eq!((base, total), (50, 100));
        assert_eq!(
            got.iter().map(|r| r.url.as_str()).collect::<Vec<_>>(),
            vec!["https://a.com", "https://b.com", "https://c.com"]
        );
    }

    #[test]
    fn expires_after_ttl() {
        let c = SearchCache::new(Duration::from_millis(10), 10);
        c.set("k".into(), 0, 1, res("https://a.com"));
        std::thread::sleep(Duration::from_millis(30));
        assert!(c.get("k").is_none());
    }

    #[test]
    fn evicts_oldest_when_full() {
        let c = SearchCache::new(Duration::from_secs(60), 2);
        c.set("a".into(), 0, 1, res("https://a.com"));
        c.set("b".into(), 0, 1, res("https://b.com"));
        c.set("c".into(), 0, 1, res("https://c.com"));
        assert!(c.get("a").is_none()); // 最旧被淘汰
        assert!(c.get("b").is_some());
        assert!(c.get("c").is_some());
    }
}
