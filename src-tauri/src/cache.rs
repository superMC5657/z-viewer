//! 解码结果 LRU 缓存（《需求报告与技术方案.md》8.2）
//!
//! 缓存 load_image 的返回（RAW 解码尤其昂贵），容量默认 8 张，
//! 命中即移到末尾（MRU），超容量淘汰队首（LRU）。
//! Arc 共享避免深拷贝；Clone 派生供 spawn_blocking 闭包移动。

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use crate::decode::LoadResult;

pub struct DecodeCache {
    inner: Arc<Mutex<VecDeque<(String, Arc<LoadResult>)>>>,
    /// 正在后台解码的路径（预取去重，防止快速翻页重复解码 RAW）
    in_flight: Arc<Mutex<HashSet<String>>>,
    capacity: usize,
}

impl Clone for DecodeCache {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            in_flight: Arc::clone(&self.in_flight),
            capacity: self.capacity,
        }
    }
}

impl DecodeCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(capacity))),
            in_flight: Arc::new(Mutex::new(HashSet::new())),
            capacity,
        }
    }

    /// 命中返回缓存值并移到末尾；未命中返回 None
    pub fn get(&self, path: &str) -> Option<Arc<LoadResult>> {
        let mut q = self.inner.lock().ok()?;
        let pos = q.iter().position(|(p, _)| p == path)?;
        let item = q.remove(pos)?;
        q.push_back(item.clone());
        Some(item.1)
    }

    /// 存在性检查（不移位，供预取判断用）
    pub fn peek(&self, path: &str) -> bool {
        self.inner
            .lock()
            .map(|q| q.iter().any(|(p, _)| p == path))
            .unwrap_or(false)
    }

    /// 预取去重：路径不在解码中则登记并返回 true；已在解码中返回 false
    pub fn begin_prefetch(&self, path: &str) -> bool {
        if self.peek(path) {
            return false;
        }
        self.in_flight
            .lock()
            .map(|mut s| s.insert(path.to_string()))
            .unwrap_or(false)
    }

    /// 预取完成/失败后释放登记
    pub fn end_prefetch(&self, path: &str) {
        if let Ok(mut s) = self.in_flight.lock() {
            s.remove(path);
        }
    }

    /// 写入缓存；已存在则更新并移到末尾，超出容量淘汰队首
    pub fn put(&self, path: String, result: Arc<LoadResult>) {
        if let Ok(mut q) = self.inner.lock() {
            if let Some(pos) = q.iter().position(|(p, _)| *p == path) {
                q.remove(pos);
            }
            q.push_back((path, result));
            while q.len() > self.capacity {
                q.pop_front();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(path: &str) -> Arc<LoadResult> {
        Arc::new(LoadResult {
            mode: "raw".into(),
            data: Some(format!("data-{path}")),
            frames: None,
        })
    }

    #[test]
    fn lru_hit_and_eviction() {
        let cache = DecodeCache::new(3);
        cache.put("a".into(), sample("a"));
        cache.put("b".into(), sample("b"));
        cache.put("c".into(), sample("c"));
        assert!(cache.get("a").is_some(), "最近写入可命中");
        // 访问 a 后 a 变为 MRU，写入 d 应淘汰最久未用的 b
        cache.put("d".into(), sample("d"));
        assert!(cache.get("b").is_none(), "超出容量淘汰 LRU(b)");
        assert!(cache.get("a").is_some());
        assert!(cache.get("c").is_some());
        assert!(cache.get("d").is_some());
    }

    #[test]
    fn put_updates_existing() {
        let cache = DecodeCache::new(2);
        cache.put("a".into(), sample("a1"));
        cache.put("a".into(), sample("a2"));
        let hit = cache.get("a").expect("命中");
        assert_eq!(hit.data.as_deref(), Some("data-a2"), "重复写入应更新");
        // 容量 2：a(更新后) + b
        cache.put("b".into(), sample("b"));
        cache.put("c".into(), sample("c"));
        assert!(cache.get("a").is_none(), "a 被新写入挤出");
        assert!(cache.get("b").is_some());
        assert!(cache.get("c").is_some());
    }

    #[test]
    fn cache_survives_clone() {
        let cache = DecodeCache::new(2);
        cache.put("a".into(), sample("a"));
        let c2 = cache.clone();
        assert!(c2.get("a").is_some(), "clone 共享内部 Arc");
        c2.put("b".into(), sample("b"));
        assert!(cache.get("b").is_some(), "clone 写入对原实例可见");
    }

    #[test]
    fn prefetch_dedup() {
        let cache = DecodeCache::new(4);
        assert!(cache.begin_prefetch("a"), "首次登记成功");
        assert!(!cache.begin_prefetch("a"), "重复登记被拒");
        assert!(!cache.begin_prefetch("a"), "仍被拒（未释放）");
        cache.end_prefetch("a");
        assert!(cache.begin_prefetch("a"), "释放后可再次登记");
        cache.end_prefetch("a");
    }

    #[test]
    fn peek_does_not_move() {
        let cache = DecodeCache::new(3);
        cache.put("a".into(), sample("a"));
        cache.put("b".into(), sample("b"));
        cache.put("c".into(), sample("c"));
        assert!(cache.peek("a"), "peek 应命中");
        // a 仍是最旧：写入 d 应淘汰 a（若 peek 移了位则淘汰 b）
        cache.put("d".into(), sample("d"));
        assert!(cache.get("a").is_none(), "peek 不应把 a 移到 MRU");
        assert!(cache.get("b").is_some());
    }
}
