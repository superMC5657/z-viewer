//! 全局浏览模型：跨同级文件夹无缝浏览（核心特色）
//!
//! 模型结构（见《需求报告与技术方案.md》8.1）：
//! - folders：父目录下所有「含图片」的同级文件夹，按 natord 自然排序（等价资源管理器）
//! - 每个文件夹内的图片按扩展名白名单过滤 + natord 自然排序
//! - next/prev 在图片级无缝跨文件夹衔接，全局首尾触发边界事件
//!
//! 异步枚举（M4.5 优化）：
//! - open() 只同步枚举当前文件夹（定位首图），兄弟文件夹由后台线程逐个填充
//! - 导航到未填充文件夹时阻塞等待（Condvar）；neighbor_paths 预取则非阻塞
//! - 扫描完成回调 on_ready(BrowseState)：前端刷新位置计数
//! - Drop 时置 cancelled，旧扫描线程尽快退出
//!
//! 文件组织（可维护性拆分）：
//! - mod.rs：类型定义与辅助函数
//! - scan.rs：open / 后台扫描线程
//! - nav.rs：图片级 / 文件夹级导航
//! - state.rs：状态计算与上下文路径
//! - tests.rs：测试（#[cfg(test)] 独立文件，不混入产品代码）

mod nav;
mod scan;
mod state;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

/// M1 常见格式（RAW 由 decode::RAW_EXTS 单一来源维护）
const COMMON_EXTS: &[&str] = &["jpg", "jpeg", "png", "gif", "webp", "bmp", "ico", "svg"];

fn is_image_file(name: &str) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    COMMON_EXTS.contains(&ext.as_str()) || crate::decode::is_raw_ext(&ext)
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

#[derive(Debug)]
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
}

struct InnerData {
    folders: Vec<Folder>,
    folder_index: usize,
    image_index: usize,
    /// 是否仍有文件夹在后台填充
    loading: bool,
    /// Drop 后置位：扫描线程尽快退出
    cancelled: bool,
}

/// 扫描完成回调（携带最新状态，供前端刷新计数）
pub type OnReady = Box<dyn FnOnce(BrowseState) + Send>;

pub struct BrowseModel {
    inner: Arc<ModelInner>,
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
