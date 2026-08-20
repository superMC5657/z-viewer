//! 队列 B：解码结果缓存（LRU）+ 预取去重登记
//! 命中 load_image 返回零解码；in_flight 登记防止快速翻页重复解码同一路径。
//! 预取并发门控：后台预取（Rust 队列 A/B）与实时解码共用 spawn_blocking 线程池，
//! 无限制并发会与用户正在看的实时解码争 CPU —— 限 PREFETCH_MAX 个并发预取，
//! 超出跳过（预取是低优先级优化，下一轮导航会再触发）。

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::decode::LoadResult;

use super::LruQueue;

/// 后台预取最大并发数（实时解码不受限，优先级永远更高）
const PREFETCH_MAX: usize = 2;

#[derive(Clone)]
pub struct DecodeCache {
    lru: LruQueue<Arc<LoadResult>>,
    /// 正在后台解码的路径（预取去重，防止快速翻页重复解码 RAW）
    in_flight: Arc<Mutex<HashSet<String>>>,
    /// 当前进行中的预取任务数（配合 in_flight 锁做并发门控，见 begin_prefetch）
    prefetch_active: Arc<AtomicUsize>,
}

impl DecodeCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            lru: LruQueue::new(capacity),
            in_flight: Arc::new(Mutex::new(HashSet::new())),
            prefetch_active: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// 命中返回缓存值并移到末尾；未命中返回 None
    pub fn get(&self, path: &str) -> Option<Arc<LoadResult>> {
        self.lru.get(path)
    }

    /// 存在性检查（不移位，供预取判断用）
    pub fn peek(&self, path: &str) -> bool {
        self.lru.peek(path)
    }

    /// 预取去重 + 并发门控：路径不在解码中且并发未达上限则登记并返回 true；
    /// 已在解码中 / 并发已满返回 false（跳过本次预取）。
    /// 检查与登记在同一把 in_flight 锁内完成：并发调用串行化，计数不超限。
    pub fn begin_prefetch(&self, path: &str) -> bool {
        if self.peek(path) {
            return false;
        }
        let mut guard = match self.in_flight.lock() {
            Ok(g) => g,
            Err(_) => return false,
        };
        if self.prefetch_active.load(Ordering::Relaxed) >= PREFETCH_MAX {
            return false;
        }
        if !guard.insert(path.to_string()) {
            return false;
        }
        self.prefetch_active.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// 预取完成/失败后释放登记（实时加载完成同样调用，语义一致）。
    /// 仅当该路径确实在预取登记中才递减并发计数（load_image 对非预取路径
    /// 也会调用本函数，此时不应影响计数）。
    pub fn end_prefetch(&self, path: &str) {
        let removed = match self.in_flight.lock() {
            Ok(mut s) => s.remove(path),
            Err(_) => false,
        };
        if removed {
            self.prefetch_active.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// 写入缓存；已存在则更新并移到末尾，超出容量淘汰队首
    pub fn put(&self, path: String, result: Arc<LoadResult>) {
        self.lru.put(path, result)
    }

    /// 调整容量（缓存强度变化时调用）；超出新容量的条目淘汰
    pub fn set_capacity(&self, capacity: usize) {
        self.lru.set_capacity(capacity)
    }
}
