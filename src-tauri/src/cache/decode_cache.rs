//! 队列 B：解码结果缓存（LRU）+ 预取去重登记
//! 命中 load_image 返回零解码；in_flight 登记防止快速翻页重复解码同一路径。
//! 预取并发门控：后台预取（Rust 队列 A/B）与实时解码共用 spawn_blocking 线程池，
//! 无限制并发会与用户正在看的实时解码争 CPU —— 限 PREFETCH_MAX 个并发预取，
//! 超出跳过（预取是低优先级优化，下一轮导航会再触发）。

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::decode::LoadResult;

use super::LruQueue;

/// 后台预取最大并发数（实时解码不受限，优先级永远更高）
const PREFETCH_MAX: usize = 2;

/// 缓存条目：解码结果 + 打包好的 IPC 信封（惰性）。
/// 信封只在首次发送时打包一次（serde 序列化 header + 拼 payload），
/// 后续缓存命中直接 Arc 复用，免重复序列化与双重分配。
#[derive(Clone)]
pub(crate) struct CachedEntry {
    pub result: Arc<LoadResult>,
    envelope: OnceLock<Arc<Vec<u8>>>,
}

impl CachedEntry {
    pub(crate) fn new(result: Arc<LoadResult>) -> Self {
        Self {
            result,
            envelope: OnceLock::new(),
        }
    }

    pub(crate) fn envelope(&self) -> Arc<Vec<u8>> {
        self.envelope
            .get_or_init(|| Arc::new(crate::decode::pack_envelope(&self.result)))
            .clone()
    }
}

#[derive(Clone)]
pub struct DecodeCache {
    lru: LruQueue<CachedEntry>,
    /// 动画拆帧结果独立槽位（只保留最近 1 条）：一段长 GIF 的帧序列可达几十上百
    /// MB，混入主流 LRU 会一张就挤掉全部 RAW/静态条目（容量 4-8）
    animated: Arc<Mutex<Option<(String, CachedEntry)>>>,
    /// 正在后台解码的路径（预取去重，防止快速翻页重复解码 RAW）
    in_flight: Arc<Mutex<HashSet<String>>>,
    /// 当前进行中的预取任务数（配合 in_flight 锁做并发门控，见 begin_prefetch）
    prefetch_active: Arc<AtomicUsize>,
}

impl DecodeCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            lru: LruQueue::new(capacity),
            animated: Arc::new(Mutex::new(None)),
            in_flight: Arc::new(Mutex::new(HashSet::new())),
            prefetch_active: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// 命中返回缓存值并移到末尾；未命中返回 None（生产路径走 get_entry 复用信封）
    #[allow(dead_code)] // 测试与诊断使用
    pub fn get(&self, path: &str) -> Option<Arc<LoadResult>> {
        self.get_entry(path).map(|e| e.result)
    }

    /// 命中返回完整条目（含惰性信封，供 load_image 复用打包结果）并移到末尾
    pub fn get_entry(&self, path: &str) -> Option<CachedEntry> {
        if let Some(e) = self.lru.get(path) {
            return Some(e);
        }
        if let Ok(guard) = self.animated.lock() {
            if let Some((k, e)) = guard.as_ref() {
                if k == path {
                    return Some(e.clone());
                }
            }
        }
        None
    }

    /// 存在性检查（不移位，供预取判断用）
    pub fn peek(&self, path: &str) -> bool {
        if self.lru.peek(path) {
            return true;
        }
        self.animated
            .lock()
            .map(|g| g.as_ref().is_some_and(|(k, _)| k == path))
            .unwrap_or(false)
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

    /// 写入缓存；动画结果进独立单槽（互相替换），其余进 LRU（已存在则更新并
    /// 移到末尾，超出容量淘汰队首）
    pub fn put(&self, path: String, result: Arc<LoadResult>) {
        if result.mode == "animated" {
            if let Ok(mut g) = self.animated.lock() {
                *g = Some((path, CachedEntry::new(result)));
            }
        } else {
            self.lru.put(path, CachedEntry::new(result));
        }
    }

    /// 调整容量（缓存强度变化时调用）；超出新容量的条目淘汰；
    /// 容量 0（缓存关闭）同时清空动画槽位
    pub fn set_capacity(&self, capacity: usize) {
        self.lru.set_capacity(capacity);
        if capacity == 0 {
            if let Ok(mut g) = self.animated.lock() {
                *g = None;
            }
        }
    }
}
