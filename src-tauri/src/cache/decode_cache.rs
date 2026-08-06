//! 队列 B：解码结果缓存（LRU）+ 预取去重登记
//! 命中 load_image 返回零解码；in_flight 登记防止快速翻页重复解码同一路径。

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::decode::LoadResult;

use super::LruQueue;

#[derive(Clone)]
pub struct DecodeCache {
    lru: LruQueue<Arc<LoadResult>>,
    /// 正在后台解码的路径（预取去重，防止快速翻页重复解码 RAW）
    in_flight: Arc<Mutex<HashSet<String>>>,
}

impl DecodeCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            lru: LruQueue::new(capacity),
            in_flight: Arc::new(Mutex::new(HashSet::new())),
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

    /// 预取完成/失败后释放登记（实时加载完成同样调用，语义一致）
    pub fn end_prefetch(&self, path: &str) {
        if let Ok(mut s) = self.in_flight.lock() {
            s.remove(path);
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
