//! 全局浏览模型：跨同级文件夹无缝浏览（核心特色）
//!
//! 模型结构（见《需求报告与技术方案.md》8.1）：
//! - folders：父目录下所有「含图片」的同级文件夹，按 natord 自然排序（等价资源管理器）
//! - 每个文件夹内的图片按扩展名白名单过滤 + natord 自然排序
//! - next/prev 在图片级无缝跨文件夹衔接，全局首尾触发边界事件
//!
//! 异步枚举：
//! - open() 只同步枚举当前文件夹（定位首图），兄弟文件夹由后台线程逐个填充
//! - 导航到未填充文件夹时阻塞等待（Condvar）；neighbor_paths 预取则非阻塞
//! - 扫描完成回调 onReady(BrowseState)：前端刷新位置计数
//! - Drop 时置 cancelled，扫描线程尽快退出
//!
//! 本文件即公共接口：外部只见 BrowseModel / BrowseState / Boundary / Nav /
//! FolderTarget / OnReady 及 BrowseModel 上的公开方法；其余（InnerData、锁、
//! Condvar、等待点）均为模块私有实现。

mod nav;
mod scan;
mod state;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use crate::decode::is_image_file;

/// 目录项 → (文件名, 路径)：仅接受常规图片文件。
/// list_images 与 first_image_of 共用的单条目判定（过滤行为唯一来源）。
/// 模块私有：子模块经 super:: 引用，外部不可见。
/// 快路径：DirEntry::file_type 直接来自目录枚举元数据（Windows 零额外系统调用）；
/// 常规文件短路通过，符号链接/未知类型回退 Path::is_file()（stat 语义，跟随链接）。
fn image_entry(entry: &std::fs::DirEntry) -> Option<(String, PathBuf)> {
    let p = entry.path();
    let is_file = match entry.file_type() {
        Ok(ft) => ft.is_file() || p.is_file(),
        Err(_) => p.is_file(),
    };
    if !is_file {
        return None;
    }
    let name = entry.file_name().to_string_lossy().to_string();
    if is_image_file(&name) {
        Some((name, p))
    } else {
        None
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Boundary {
    /// 图片级：已到全局第一张（再往前）
    FirstImage,
    /// 图片级：已到全局最后一张（再往后）
    LastImage,
    /// 文件夹级：已是第一个文件夹（再往前跳）
    FirstFolder,
    /// 文件夹级：已是最后一个文件夹（再往后跳）
    LastFolder,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Nav {
    /// 正常切换，无边界
    Ok,
    /// 撞到边界（当前图片不变，前端弹 Toast）
    Boundary(Boundary),
}

/// 单个文件夹：路径 + 图片列表（None = 后台填充中）
struct Folder {
    path: PathBuf,
    images: Option<Vec<PathBuf>>,
}

struct ModelInner {
    m: Mutex<InnerData>,
    cv: Condvar,
    /// 已查过的图片大小缓存（key=图片路径）：state() 读取免重复 fs::metadata
    /// 系统调用 —— 翻回上一张/幻灯片循环同一张图时零 syscall。
    /// 每张图最多 8 字节 + PathBuf，仅含本次会话访问过的图，内存有界。
    /// 独立锁：未命中时可在**不持模型主锁**的情况下 stat + 回写（慢盘上 stat
    /// 可达毫秒级，放主锁内会阻塞导航与扫描线程）。
    file_sizes: Mutex<HashMap<PathBuf, u64>>,
}

struct InnerData {
    folders: Vec<Folder>,
    folder_index: usize,
    image_index: usize,
    /// 已填充图片总数（增量维护，状态读取 O(1)）
    global_total: usize,
    /// 当前图片全局位置 = folder_index 之前已填充图片数 + image_index（增量维护）
    global_index: usize,
    /// 是否仍有文件夹在后台填充
    loading: bool,
    /// Drop 后置位：扫描线程尽快退出
    cancelled: bool,
}

/// 扫描完成回调（携带最新状态，供前端刷新计数）
pub type OnReady = Box<dyn FnOnce(BrowseState) + Send>;

/// Clone 仅复制 Arc 句柄（同一模型），导航/状态方法走 &self + 内部锁；
/// AppState 持 Arc<Option<_>>：命令可快照出模型后释放全局锁再慢慢等填充
#[derive(Clone)]
pub struct BrowseModel {
    inner: Arc<ModelInner>,
}

impl BrowseModel {
    /// 全模块唯一的阻塞等待点：等待 folders[idx] 填充完成，返回其长度与锁。
    /// 取消/走到尽头返回 Over（调用方映射为 Boundary/空状态，不再复查标志）；
    /// 等待中若发生压缩（唯一缩表者，长度变化可观测）则返回 Stale，
    /// 调用方整体重试——压缩单次运行且压缩后全填充，重试必收敛、无等待。
    /// 睡眠中压缩可能让 idx 指向另一文件夹（别名），故命中亦复验长度，
    /// 不可将错就错。导航（next/prev/jump_folder）与状态快照都经此，
    /// 任何处不手写 lock/wait/cancel。
    fn wait_filled<'a>(
        inner: &Arc<ModelInner>,
        mut guard: MutexGuard<'a, InnerData>,
        idx: usize,
        entry_len: usize,
    ) -> Waited<'a> {
        loop {
            if guard.cancelled {
                return Waited::Over;
            }
            if idx >= guard.folders.len() {
                // 长度变化 ⟹ 睡眠中发生压缩，索引已失效 → 整体重试；
                // 长度不变 ⟹ 真走到尽头 → 收尾。
                return if guard.folders.len() != entry_len {
                    Waited::Stale
                } else {
                    Waited::Over
                };
            }
            if let Some(imgs) = &guard.folders[idx].images {
                if guard.folders.len() != entry_len {
                    return Waited::Stale;
                }
                return Waited::Ready(imgs.len(), guard);
            }
            guard = inner
                .cv
                .wait(guard)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

/// wait_filled 结果（模块私有三态）：就绪 / 终结（取消或走到尽头，
/// 调用方按 Boundary/空状态收尾）/ 失效（等待中发生压缩，调用方整体重试）。
enum Waited<'a> {
    Ready(usize, MutexGuard<'a, InnerData>),
    Over,
    Stale,
}

/// 跳过式查找结果（模块私有三态）：命中 / 走到尽头 / 压缩致索引失效（整体重试）。
enum Found<'a> {
    Hit(usize, usize, MutexGuard<'a, InnerData>),
    End,
    Stale,
}

impl Drop for BrowseModel {
    fn drop(&mut self) {
        // 通知后台扫描线程尽快退出（与 wait_filled 紧邻：每次等待都能醒来见取消）。
        // 与等待者同法恢复中毒锁：静默跳过会让阻塞的 wait_filled 守着已死的
        // 扫描线程空转（poison 后永不 notify）。
        let mut d = self
            .inner
            .m
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        d.cancelled = true;
        self.inner.cv.notify_all();
    }
}

/// 给前端的当前浏览状态（纯数据）
#[derive(serde::Serialize, Clone)]
pub struct BrowseState {
    pub path: String,
    pub file_name: String,
    pub folder_name: String,
    pub file_size: u64,
    /// 全局位置（0-based，跨文件夹累计；未加载部分按已填充累计）
    pub global_index: usize,
    /// 全局总数（后台枚举进行中为「已填充部分」）
    pub global_total: usize,
    /// 当前文件夹在同级文件夹中的位置
    pub folder_index: usize,
    pub folder_total: usize,
    /// 后台枚举是否仍在进行（前端显示 "3/…"）
    pub loading: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FolderTarget {
    First,
    Prev,
    Next,
    Last,
}

/// Windows 下路径比较：canonicalize 归一化（处理 \\?\ 前缀与大小写）
fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}
