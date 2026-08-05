//! 队列 A：每个文件夹第一张图片的解码缓存（LRU）
//! 跨文件夹跳转（PgUp/PgDn/⏮⏭）时命中首图缓存 → 无延迟显示
//! key = 文件夹路径，value = 该文件夹第一张图的解码结果（raw/animated）

use std::sync::Arc;

use crate::decode::LoadResult;

use super::LruQueue;

#[derive(Clone)]
pub struct FolderFirstCache(LruQueue);

impl FolderFirstCache {
    pub fn new(capacity: usize) -> Self {
        Self(LruQueue::new(capacity))
    }

    pub fn get(&self, folder: &str) -> Option<Arc<LoadResult>> {
        self.0.get(folder)
    }

    pub fn peek(&self, folder: &str) -> bool {
        self.0.peek(folder)
    }

    pub fn put(&self, folder: String, result: Arc<LoadResult>) {
        self.0.put(folder, result)
    }

    pub fn set_capacity(&self, capacity: usize) {
        self.0.set_capacity(capacity)
    }
}
