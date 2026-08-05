//! 解码结果 LRU 缓存（《需求报告与技术方案.md》8.2）
//!
//! 缓存 load_image 的返回（RAW 解码尤其昂贵），容量默认 8 张，
//! 命中即移到末尾（MRU），超容量淘汰队首（LRU）。
//! Arc 共享避免深拷贝；Clone 派生供 spawn_blocking 闭包移动。

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use crate::decode::LoadResult;

/// 队列 A：每个文件夹第一张图片的解码缓存（LRU）
/// 跨文件夹跳转（PgUp/PgDn/⏮⏭）时命中首图缓存 → 无延迟显示
/// key = 文件夹路径，value = 该文件夹第一张图的解码结果（raw/animated）
pub struct FolderFirstCache {
    inner: Arc<Mutex<VecDeque<(String, Arc<LoadResult>)>>>,
    capacity: std::sync::atomic::AtomicUsize,
}

impl Clone for FolderFirstCache {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            capacity: std::sync::atomic::AtomicUsize::new(self.capacity()),
        }
    }
}

impl FolderFirstCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(capacity))),
            capacity: std::sync::atomic::AtomicUsize::new(capacity),
        }
    }

    pub fn get(&self, folder: &str) -> Option<Arc<LoadResult>> {
        let mut q = self.inner.lock().ok()?;
        let pos = q.iter().position(|(f, _)| f == folder)?;
        let item = q.remove(pos)?;
        q.push_back(item.clone());
        Some(item.1)
    }

    pub fn peek(&self, folder: &str) -> bool {
        self.inner
            .lock()
            .map(|q| q.iter().any(|(f, _)| f == folder))
            .unwrap_or(false)
    }

    pub fn put(&self, folder: String, result: Arc<LoadResult>) {
        if let Ok(mut q) = self.inner.lock() {
            if let Some(pos) = q.iter().position(|(f, _)| *f == folder) {
                q.remove(pos);
            }
            q.push_back((folder, result));
            while q.len() > self.capacity() {
                q.pop_front();
            }
        }
    }

    pub fn set_capacity(&self, capacity: usize) {
        self.capacity.store(capacity, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut q) = self.inner.lock() {
            while q.len() > self.capacity() {
                q.pop_front();
            }
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity.load(std::sync::atomic::Ordering::Relaxed)
    }
}

pub struct DecodeCache {
    inner: Arc<Mutex<VecDeque<(String, Arc<LoadResult>)>>>,
    /// 正在后台解码的路径（预取去重，防止快速翻页重复解码 RAW）
    in_flight: Arc<Mutex<HashSet<String>>>,
    /// 容量（缓存强度变化时调整；原子读，锁内写）
    capacity: std::sync::atomic::AtomicUsize,
}

impl Clone for DecodeCache {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            in_flight: Arc::clone(&self.in_flight),
            capacity: std::sync::atomic::AtomicUsize::new(self.capacity()),
        }
    }
}

impl DecodeCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(capacity))),
            in_flight: Arc::new(Mutex::new(HashSet::new())),
            capacity: std::sync::atomic::AtomicUsize::new(capacity),
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

    /// 实时加载完成：清除预取登记（实时解码已覆盖预取语义，预取任务应跳过 put）
    pub fn finish_load(&self, path: &str) {
        self.end_prefetch(path);
    }

    /// 是否正在预取解码中（load_image 实时路径可选等待，避免重复解码）
    pub fn is_prefetching(&self, path: &str) -> bool {
        self.in_flight
            .lock()
            .map(|s| s.contains(path))
            .unwrap_or(false)
    }

    /// 写入缓存；已存在则更新并移到末尾，超出容量淘汰队首
    pub fn put(&self, path: String, result: Arc<LoadResult>) {
        if let Ok(mut q) = self.inner.lock() {
            if let Some(pos) = q.iter().position(|(p, _)| *p == path) {
                q.remove(pos);
            }
            q.push_back((path, result));
            while q.len() > self.capacity() {
                q.pop_front();
            }
        }
    }

    /// 调整容量（缓存强度变化时调用）；超出新容量的条目淘汰
    pub fn set_capacity(&self, capacity: usize) {
        self.capacity.store(capacity, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut q) = self.inner.lock() {
            while q.len() > self.capacity() {
                q.pop_front();
            }
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity.load(std::sync::atomic::Ordering::Relaxed)
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

    #[test]
    fn finish_load_clears_prefetch_registration() {
        let cache = DecodeCache::new(4);
        assert!(cache.begin_prefetch("a"));
        assert!(cache.is_prefetching("a"), "登记后 in_flight");
        // 实时加载完成 → finish_load 清登记
        cache.finish_load("a");
        assert!(!cache.is_prefetching("a"), "finish_load 后不再预取中");
        // 预取任务此时 put（peek 检查在 commands 层）：登记已清，可再次 begin
        assert!(cache.begin_prefetch("a"), "可重新登记");
        cache.end_prefetch("a");
    }

    #[test]
    fn prefetch_put_skips_if_already_cached() {
        // 模拟：实时加载先 put，预取任务后 decode 完成 → peek 命中则跳过 put（不覆盖/不重复 LRU 操作）
        let cache = DecodeCache::new(4);
        cache.put("a".into(), sample("realtime"));
        // 预取任务逻辑：decode 完成后 peek 检查
        let skip = cache.peek("a");
        assert!(skip, "实时已缓存 → 预取应跳过 put");
        let hit = cache.get("a").expect("命中实时数据");
        assert_eq!(hit.data.as_deref(), Some("data-realtime"), "未被预取覆盖");
    }

    #[test]
    fn set_capacity_evicts_immediately() {
        let cache = DecodeCache::new(4);
        for i in 0..4 {
            cache.put(format!("p{i}"), sample(&format!("p{i}")));
        }
        cache.set_capacity(2);
        assert_eq!(cache.capacity(), 2);
        assert!(cache.get("p0").is_none(), "容量收紧后淘汰最旧");
        assert!(cache.get("p1").is_none());
        assert!(cache.get("p2").is_some());
        assert!(cache.get("p3").is_some());
    }

    #[test]
    fn folder_first_cache_lru() {
        let cache = FolderFirstCache::new(2);
        cache.put("A".into(), sample("a"));
        cache.put("B".into(), sample("b"));
        assert!(cache.peek("A"));
        assert!(cache.get("A").is_some(), "命中 A 并移到 MRU");
        cache.put("C".into(), sample("c"));
        assert!(cache.get("B").is_none(), "A 被访问后 B 是 LRU 被淘汰");
        assert!(cache.get("A").is_some());
        assert!(cache.get("C").is_some());
        // 关闭缓存：set_capacity(0) 清空
        cache.set_capacity(0);
        assert!(cache.get("A").is_none(), "容量 0 全清");
    }
}
