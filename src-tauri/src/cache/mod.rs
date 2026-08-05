//! 解码结果 LRU 缓存（《需求报告与技术方案.md》8.2）
//!
//! 缓存 load_image 的返回（RAW 解码尤其昂贵），容量默认 8 张，
//! 命中即移到末尾（MRU），超容量淘汰队首（LRU）。
//! Arc 共享避免深拷贝；Clone 派生供 spawn_blocking 闭包移动。
//!
//! 文件组织（可维护性拆分）：
//! - mod.rs：通用 LRU 队列（共享基础设施）
//! - folder_first.rs：队列 A（文件夹首图缓存）
//! - decode_cache.rs：队列 B（解码缓存，含预取去重）
//! - tests.rs：测试（#[cfg(test)] 独立文件，不混入产品代码）

mod decode_cache;
mod folder_first;
#[cfg(test)]
mod tests;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::decode::LoadResult;

pub use decode_cache::DecodeCache;
pub use folder_first::FolderFirstCache;

/// LRU 条目：key → 解码结果
type Entry = (String, Arc<LoadResult>);

/// 通用 LRU 队列（String key → Arc<LoadResult>）
/// clone 共享内部队列与容量（供 spawn_blocking 闭包移动），操作均线程安全
#[derive(Clone)]
pub(super) struct LruQueue {
    inner: Arc<Mutex<VecDeque<Entry>>>,
    capacity: Arc<AtomicUsize>,
}

impl LruQueue {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(capacity))),
            capacity: Arc::new(AtomicUsize::new(capacity)),
        }
    }

    /// 命中返回缓存值并移到末尾；未命中返回 None
    pub(super) fn get(&self, key: &str) -> Option<Arc<LoadResult>> {
        let mut q = self.inner.lock().ok()?;
        let pos = q.iter().position(|(k, _)| k == key)?;
        let (k, v) = q.remove(pos)?;
        q.push_back((k, v.clone()));
        Some(v)
    }

    /// 存在性检查（不移位，供预取判断用）
    pub(super) fn peek(&self, key: &str) -> bool {
        self.inner
            .lock()
            .map(|q| q.iter().any(|(k, _)| k == key))
            .unwrap_or(false)
    }

    /// 写入；已存在则更新并移到末尾，超出容量淘汰队首
    pub(super) fn put(&self, key: String, result: Arc<LoadResult>) {
        if let Ok(mut q) = self.inner.lock() {
            if let Some(pos) = q.iter().position(|(k, _)| *k == key) {
                q.remove(pos);
            }
            q.push_back((key, result));
            while q.len() > self.capacity() {
                q.pop_front();
            }
        }
    }

    /// 当前容量（缓存强度变化时调整）
    pub(super) fn capacity(&self) -> usize {
        self.capacity.load(Ordering::Relaxed)
    }

    /// 调整容量（缓存强度变化时调用）；超出新容量的条目淘汰
    pub(super) fn set_capacity(&self, capacity: usize) {
        self.capacity.store(capacity, Ordering::Relaxed);
        if let Ok(mut q) = self.inner.lock() {
            while q.len() > self.capacity() {
                q.pop_front();
            }
        }
    }
}
