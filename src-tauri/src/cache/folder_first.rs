//! 队列 A：每个文件夹第一张图片的解码缓存（LRU）
//! 跨文件夹跳转（PgUp/PgDn/⏮⏭）时命中首图缓存 → 无延迟显示
//! key = 文件夹路径，value = 该文件夹第一张图的解码结果（raw/animated）+ 首图路径。
//! 路径随值保存：promote 并入队列 B 时无需再 list_images 扫目录定位首图，
//! 避免在持有 AppState 锁的导航路径上做同步文件系统枚举。

use std::sync::Arc;

use crate::decode::LoadResult;

use super::LruQueue;

/// 文件夹首图缓存条目：解码结果 + 首图路径（并入 DecodeCache 时的 key）
#[derive(Clone)]
pub struct FolderFirst {
    /// 该文件夹第一张图片的路径
    pub path: String,
    /// 解码结果（raw/animated；asset 不缓存）
    pub result: Arc<LoadResult>,
}

#[derive(Clone)]
pub struct FolderFirstCache(LruQueue<Arc<FolderFirst>>);

impl FolderFirstCache {
    pub fn new(capacity: usize) -> Self {
        Self(LruQueue::new(capacity))
    }

    pub fn get(&self, folder: &str) -> Option<Arc<FolderFirst>> {
        self.0.get(folder)
    }

    pub fn peek(&self, folder: &str) -> bool {
        self.0.peek(folder)
    }

    pub fn put(&self, folder: String, first: Arc<FolderFirst>) {
        self.0.put(folder, first)
    }

    pub fn set_capacity(&self, capacity: usize) {
        self.0.set_capacity(capacity)
    }
}
