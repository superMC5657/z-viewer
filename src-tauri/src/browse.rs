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

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

/// M1 常见格式（RAW 由 decode::RAW_EXTS 单一来源维护）
const COMMON_EXTS: &[&str] = &["jpg", "jpeg", "png", "gif", "webp", "bmp", "ico", "svg"];

fn is_image_file(name: &str) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    COMMON_EXTS.contains(&ext.as_str()) || crate::decode::is_raw_ext(&ext)
}

fn natord_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    natord::compare(a, b)
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

impl BrowseModel {
    /// 以某张图片为起点建立浏览模型；图片或其父目录无效时返回 None
    /// 仅同步枚举当前文件夹定位图片，兄弟文件夹后台填充
    pub fn open(path: &Path, on_ready: Option<OnReady>) -> Option<Self> {
        if !path.is_file() || !is_image_file(&path.file_name()?.to_string_lossy()) {
            return None;
        }
        let parent = path.parent()?;
        if !parent.is_dir() {
            return None;
        }

        let current_folder_canon = canonical(parent);

        // 同级文件夹（含自身）：枚举图片所在目录的兄弟目录；盘根目录时仅自身
        // 此步骤仅列目录名，开销小，同步完成
        let mut dirs: Vec<PathBuf> = Vec::new();
        match parent.parent() {
            Some(base) => {
                if let Ok(rd) = std::fs::read_dir(base) {
                    for entry in rd.flatten() {
                        let p = entry.path();
                        if p.is_dir() {
                            dirs.push(p);
                        }
                    }
                }
            }
            None => dirs.push(parent.to_path_buf()),
        }
        dirs.sort_by(|a, b| {
            let an = a
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let bn = b
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            natord_cmp(&an, &bn)
        });

        // 同步枚举当前文件夹（定位当前图片必需）
        let current_images = Self::list_images(parent);
        let file_name = path.file_name()?.to_string_lossy().to_string();
        let image_index = current_images.iter().position(|img| {
            img.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .is_some_and(|n| n.eq_ignore_ascii_case(&file_name))
        })?;

        // 构建 folders：当前文件夹已填充，其余 None（后台填充）
        let mut folder_index = None;
        let mut folders = Vec::with_capacity(dirs.len());
        for (i, d) in dirs.iter().enumerate() {
            if canonical(d) == current_folder_canon {
                folder_index = Some(i);
                folders.push(Folder {
                    path: d.clone(),
                    images: Some(current_images.clone()),
                });
            } else {
                folders.push(Folder {
                    path: d.clone(),
                    images: None,
                });
            }
        }
        let folder_index = folder_index?;
        let pending_total = folders.len();

        let inner = Arc::new(ModelInner {
            m: Mutex::new(InnerData {
                folders,
                folder_index,
                image_index,
                loading: pending_total > 1,
                cancelled: false,
            }),
            cv: Condvar::new(),
        });

        // 后台扫描兄弟文件夹
        let scan_inner = Arc::clone(&inner);
        std::thread::spawn(move || {
            let pending: Vec<usize> = {
                let d = scan_inner.m.lock().unwrap();
                d.folders
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| f.images.is_none())
                    .map(|(i, _)| i)
                    .collect()
            };
            for idx in pending {
                {
                    let d = scan_inner.m.lock().unwrap();
                    if d.cancelled {
                        return;
                    }
                }
                let path = scan_inner.m.lock().unwrap().folders[idx].path.clone();
                let imgs = Self::list_images(&path);
                let mut d = scan_inner.m.lock().unwrap();
                if d.cancelled {
                    return;
                }
                d.folders[idx].images = Some(imgs);
            }

            // 全部填充完成：压缩空文件夹（open 时无法预知哪些兄弟为空）
            let mut d = scan_inner.m.lock().unwrap();
            if d.cancelled {
                return;
            }
            let old_folders = std::mem::take(&mut d.folders);
            let old_index = d.folder_index;
            let mut new_folders = Vec::with_capacity(old_folders.len());
            let mut removed_before = 0usize;
            for (i, f) in old_folders.into_iter().enumerate() {
                let is_empty = matches!(&f.images, Some(imgs) if imgs.is_empty());
                if is_empty && i != old_index {
                    if i < old_index {
                        removed_before += 1;
                    }
                    continue;
                }
                new_folders.push(f);
            }
            d.folders = new_folders;
            d.folder_index = old_index - removed_before;
            d.loading = false;
            drop(d); // 先释放锁，state_from_inner 会再次加锁
            let state = Self::state_from_inner(&scan_inner);
            if let Some(cb) = on_ready {
                cb(state);
            }
            scan_inner.cv.notify_all();
        });

        Some(Self { inner })
    }

    /// 打开目录：取目录内第一张图片（自然排序）作为起点
    pub fn open_first_in_dir(dir: &Path, on_ready: Option<OnReady>) -> Option<Self> {
        let first = Self::list_images(dir).into_iter().next()?;
        Self::open(&first, on_ready)
    }

    fn list_images(dir: &Path) -> Vec<PathBuf> {
        let mut imgs: Vec<PathBuf> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(dir) {
            for entry in rd.flatten() {
                let p = entry.path();
                if p.is_file()
                    && is_image_file(&p.file_name().unwrap_or_default().to_string_lossy())
                {
                    imgs.push(p);
                }
            }
        }
        imgs.sort_by(|a, b| {
            natord_cmp(
                &a.file_name().unwrap_or_default().to_string_lossy(),
                &b.file_name().unwrap_or_default().to_string_lossy(),
            )
        });
        imgs
    }

    /// 等待后台枚举完成（测试用）
    #[cfg(test)]
    pub fn wait_ready(&self) {
        let mut d = self.inner.m.lock().unwrap();
        while d.loading {
            d = self.inner.cv.wait(d).unwrap();
        }
    }

    // ---------- 导航 ----------

    /// 下一张：跨文件夹无缝衔接；全局最后一张返回 Boundary(LastImage)
    pub fn next(&mut self) -> Nav {
        let d = self.inner.m.lock().unwrap();
        let fi = d.folder_index;
        let (cur_len, mut d) = Self::wait_folder_len(&self.inner, d, fi);
        if d.image_index + 1 < cur_len {
            d.image_index += 1;
            Nav::Ok
        } else if fi + 1 < d.folders.len() {
            d.folder_index = fi + 1;
            d.image_index = 0;
            Nav::Ok
        } else {
            Nav::Boundary(Boundary::LastImage)
        }
    }

    /// 上一张：反向无缝衔接；全局第一张返回 Boundary(FirstImage)
    pub fn prev(&mut self) -> Nav {
        let d = self.inner.m.lock().unwrap();
        let fi = d.folder_index;
        if d.image_index > 0 {
            let mut d = d;
            d.image_index -= 1;
            Nav::Ok
        } else if fi > 0 {
            let (prev_len, mut d) = Self::wait_folder_len(&self.inner, d, fi - 1);
            d.folder_index = fi - 1;
            d.image_index = prev_len.saturating_sub(1);
            Nav::Ok
        } else {
            Nav::Boundary(Boundary::FirstImage)
        }
    }

    /// 文件夹级跳转（PgUp/PgDn / 首末按钮），目标显示该文件夹第一张
    pub fn jump_folder(&mut self, target: FolderTarget) -> Nav {
        let mut d = self.inner.m.lock().unwrap();
        match target {
            FolderTarget::First => {
                d.folder_index = 0;
                d.image_index = 0;
                Nav::Ok
            }
            FolderTarget::Last => {
                d.folder_index = d.folders.len() - 1;
                d.image_index = 0;
                Nav::Ok
            }
            FolderTarget::Prev => {
                if d.folder_index > 0 {
                    d.folder_index -= 1;
                    d.image_index = 0;
                    Nav::Ok
                } else {
                    Nav::Boundary(Boundary::FirstFolder)
                }
            }
            FolderTarget::Next => {
                if d.folder_index + 1 < d.folders.len() {
                    d.folder_index += 1;
                    d.image_index = 0;
                    Nav::Ok
                } else {
                    Nav::Boundary(Boundary::LastFolder)
                }
            }
        }
    }

    // ---------- 状态 ----------

    /// 当前图片上下文的路径（跨文件夹衔接），用于预加载缓存（8.2）
    /// 前 prev_n 张 + 后 next_n 张；未填充的文件夹方向跳过（预取是优化，不等待）
    pub fn context_paths(&self, prev_n: usize, next_n: usize) -> Vec<PathBuf> {
        let d = self.inner.m.lock().unwrap();
        let fi = d.folder_index;
        let ii = d.image_index;
        let mut out = Vec::with_capacity(prev_n + next_n);

        // 向前 prev_n 张（跨文件夹：当前文件夹内往前，耗尽则上一文件夹末尾）
        {
            let mut remaining = prev_n;
            let mut fi_cursor = fi;
            let mut prev_idx = ii; // 线性上一张索引
            while remaining > 0 {
                if prev_idx > 0 {
                    if let Some(imgs) = d.folders[fi_cursor].images.as_ref() {
                        if prev_idx - 1 < imgs.len() {
                            out.push(imgs[prev_idx - 1].clone());
                            prev_idx -= 1;
                            remaining -= 1;
                            continue;
                        }
                    }
                }
                if fi_cursor > 0 {
                    fi_cursor -= 1;
                    prev_idx = d.folders[fi_cursor]
                        .images
                        .as_ref()
                        .map(|imgs| imgs.len())
                        .unwrap_or(0);
                } else {
                    break;
                }
            }
        }

        // 向后 next_n 张（跨文件夹：当前文件夹内往后，耗尽则下一文件夹开头）
        {
            let mut remaining = next_n;
            let mut fi_cursor = fi;
            let mut next_idx = ii + 1; // 线性下一张索引
            while remaining > 0 {
                let cur_len = d.folders[fi_cursor]
                    .images
                    .as_ref()
                    .map(|imgs| imgs.len())
                    .unwrap_or(0);
                if next_idx < cur_len {
                    if let Some(imgs) = d.folders[fi_cursor].images.as_ref() {
                        out.push(imgs[next_idx].clone());
                    }
                    next_idx += 1;
                    remaining -= 1;
                } else if fi_cursor + 1 < d.folders.len() {
                    fi_cursor += 1;
                    next_idx = 0;
                } else {
                    break;
                }
            }
        }
        out
    }

    /// 当前文件夹相邻的文件夹路径（前 depth 个 + 后 depth 个，跨文件夹跳转预取队列 A 用）
    pub fn neighbor_folders(&self, depth: usize) -> Vec<PathBuf> {
        let d = self.inner.m.lock().unwrap();
        let fi = d.folder_index;
        let mut out = Vec::new();
        for i in (fi.saturating_sub(depth))..fi {
            out.push(d.folders[i].path.clone());
        }
        for i in (fi + 1)..=(fi + depth).min(d.folders.len() - 1) {
            out.push(d.folders[i].path.clone());
        }
        out
    }

    /// 取某文件夹的第一张图片（自然排序首张）
    pub fn first_image_of(dir: &Path) -> Option<PathBuf> {
        Self::list_images(dir).into_iter().next()
    }

    pub fn state(&self) -> BrowseState {
        Self::state_from_inner(&self.inner)
    }

    fn state_from_inner(inner: &Arc<ModelInner>) -> BrowseState {
        let mut d = inner.m.lock().unwrap();
        // 当前文件夹必须已填充（open 时同步）或等待
        loop {
            if let Some(imgs) = &d.folders[d.folder_index].images {
                let folder = &d.folders[d.folder_index];
                let img = imgs.get(d.image_index).cloned();
                let file_name = img
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let folder_name = folder
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| folder.path.to_string_lossy().to_string());

                // 全局位置：已填充文件夹累计 + 当前索引
                let mut global_index = 0usize;
                let mut global_total = 0usize;
                for (i, f) in d.folders.iter().enumerate() {
                    if let Some(imgs) = &f.images {
                        global_total += imgs.len();
                        if i < d.folder_index {
                            global_index += imgs.len();
                        }
                    }
                }
                global_index += d.image_index;

                let path = img
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                let file_size = img
                    .as_ref()
                    .and_then(|p| std::fs::metadata(p).ok())
                    .map(|m| m.len())
                    .unwrap_or(0);

                return BrowseState {
                    path,
                    file_name,
                    folder_name,
                    file_size,
                    global_index,
                    global_total,
                    folder_index: d.folder_index,
                    folder_total: d.folders.len(),
                    loading: d.loading,
                };
            }
            if d.cancelled {
                // 模型已废弃：返回空状态
                return BrowseState {
                    path: String::new(),
                    file_name: String::new(),
                    folder_name: String::new(),
                    file_size: 0,
                    global_index: 0,
                    global_total: 0,
                    folder_index: 0,
                    folder_total: 0,
                    loading: false,
                };
            }
            d = inner.cv.wait(d).unwrap();
        }
    }

    /// 在已持有锁的情况下等待某文件夹填充完成；返回 (长度, guard)
    fn wait_folder_len<'a>(
        inner: &Arc<ModelInner>,
        mut guard: std::sync::MutexGuard<'a, InnerData>,
        idx: usize,
    ) -> (usize, std::sync::MutexGuard<'a, InnerData>) {
        loop {
            if let Some(imgs) = &guard.folders[idx].images {
                return (imgs.len(), guard);
            }
            if guard.cancelled {
                return (0, guard);
            }
            guard = inner.cv.wait(guard).unwrap();
        }
    }
}

impl Drop for BrowseModel {
    fn drop(&mut self) {
        // 通知后台扫描线程尽快退出
        if let Ok(mut d) = self.inner.m.lock() {
            d.cancelled = true;
        }
        self.inner.cv.notify_all();
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    static SEQ: AtomicU32 = AtomicU32::new(0);

    fn temp_base() -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("image-viewer-test-{}-{n}", std::process::id()))
    }

    fn touch(p: &PathBuf) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"x").unwrap();
    }

    /// 树：A{a1,a2,a10}.png、B{b1,b2}.jpg、C{c1}.png、empty/（无图，应被排除）
    fn build_tree() -> PathBuf {
        let base = temp_base();
        for n in ["a1", "a2", "a10"] {
            touch(&base.join("A").join(format!("{n}.png")));
        }
        for n in ["b1", "b2"] {
            touch(&base.join("B").join(format!("{n}.jpg")));
        }
        touch(&base.join("C").join("c1.png"));
        fs::create_dir_all(base.join("empty")).unwrap();
        base
    }

    fn cleanup(base: &PathBuf) {
        let _ = fs::remove_dir_all(base);
    }

    fn open_sync(path: &PathBuf) -> BrowseModel {
        let m = BrowseModel::open(path, None).unwrap();
        m.wait_ready(); // 等待后台枚举完成，保证断言稳定
        m
    }

    #[test]
    fn natural_order_and_folders() {
        let base = build_tree();
        let m = open_sync(&base.join("A/a1.png"));
        {
            let d = m.inner.m.lock().unwrap();
            assert_eq!(d.folders.len(), 3, "无图文件夹应被排除");
            assert_eq!(d.folders[0].images.as_ref().unwrap().len(), 3);
            assert_eq!(
                d.folders[0].images.as_ref().unwrap()[2]
                    .file_name()
                    .unwrap(),
                "a10.png",
                "natord 自然排序：a1, a2, a10"
            );
        }
        cleanup(&base);
    }

    #[test]
    fn cross_folder_next_and_boundary() {
        let base = build_tree();
        let mut m = open_sync(&base.join("A/a10.png"));
        assert!(matches!(m.next(), Nav::Ok));
        assert_eq!(m.state().folder_name, "B", "A 末尾无缝进入 B 开头");
        assert_eq!(m.state().file_name, "b1.jpg");
        assert!(matches!(m.next(), Nav::Ok));
        assert_eq!(m.state().file_name, "b2.jpg", "B 内第二张");
        assert!(matches!(m.next(), Nav::Ok));
        assert_eq!(m.state().folder_name, "C", "B 末尾无缝进入 C 开头");
        assert!(
            matches!(m.next(), Nav::Boundary(Boundary::LastImage)),
            "C 末尾撞全局边界"
        );
        assert_eq!(m.state().file_name, "c1.png", "边界时图片不变");
        cleanup(&base);
    }

    #[test]
    fn prev_and_first_boundary() {
        let base = build_tree();
        let mut m = open_sync(&base.join("C/c1.png"));
        assert!(matches!(m.prev(), Nav::Ok));
        assert_eq!(m.state().folder_name, "B");
        assert_eq!(
            m.state().file_name,
            "b2.jpg",
            "反向跨文件夹进入上一文件夹最后一张"
        );
        while m.state().global_index > 0 {
            assert!(matches!(m.prev(), Nav::Ok));
        }
        assert!(matches!(m.prev(), Nav::Boundary(Boundary::FirstImage)));
        cleanup(&base);
    }

    #[test]
    fn jump_folder_and_folder_boundary() {
        let base = build_tree();
        let mut m = open_sync(&base.join("A/a2.png"));
        assert!(
            matches!(
                m.jump_folder(FolderTarget::Prev),
                Nav::Boundary(Boundary::FirstFolder)
            ),
            "首个文件夹再往前跳 = 文件夹级边界"
        );
        assert!(matches!(m.jump_folder(FolderTarget::Next), Nav::Ok));
        assert_eq!(m.state().folder_name, "B");
        assert!(matches!(m.jump_folder(FolderTarget::Last), Nav::Ok));
        assert_eq!(m.state().folder_name, "C");
        assert!(
            matches!(
                m.jump_folder(FolderTarget::Next),
                Nav::Boundary(Boundary::LastFolder)
            ),
            "末个文件夹再往后跳 = 文件夹级边界"
        );
        assert!(matches!(m.jump_folder(FolderTarget::First), Nav::Ok));
        assert_eq!(m.state().folder_name, "A");
        cleanup(&base);
    }

    #[test]
    fn global_index_accumulates() {
        let base = build_tree();
        let mut m = open_sync(&base.join("A/a1.png"));
        assert_eq!(m.state().global_index, 0);
        assert_eq!(m.state().global_total, 6, "A(3)+B(2)+C(1)");
        m.next();
        m.next();
        m.next(); // A 3 张走完 -> B 第一张
        assert_eq!(m.state().folder_name, "B");
        assert_eq!(m.state().global_index, 3, "跨文件夹后全局位置累计");
        cleanup(&base);
    }

    #[test]
    fn open_dir_takes_first_image() {
        let base = build_tree();
        let m = open_sync(&base.join("B").join("b1.jpg"));
        assert_eq!(m.state().file_name, "b1.jpg");
        cleanup(&base);
    }

    #[cfg(windows)]
    #[test]
    fn open_case_insensitive() {
        let base = build_tree();
        // Windows 文件系统大小写不敏感：传大写文件名应能定位
        let m = open_sync(&base.join("A/A2.PNG"));
        assert_eq!(m.state().file_name, "a2.png");
        cleanup(&base);
    }

    #[test]
    fn open_is_fast_on_current_folder() {
        // open() 应立即可用（不等待后台枚举）：state 可读且指向正确文件
        let base = build_tree();
        let m = BrowseModel::open(&base.join("A/a2.png"), None).unwrap();
        assert_eq!(
            m.state().file_name,
            "a2.png",
            "当前文件夹已同步枚举，立即可用"
        );
        assert!(m.state().loading, "后台枚举未完成时 loading=true");
        m.wait_ready();
        assert!(!m.state().loading, "完成后 loading=false");
        cleanup(&base);
    }

    #[test]
    fn on_ready_fires_after_scan() {
        let base = build_tree();
        let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ready2 = ready.clone();
        let _m = BrowseModel::open(
            &base.join("A/a1.png"),
            Some(Box::new(move |_state| {
                ready2.store(true, Ordering::SeqCst);
            })),
        )
        .unwrap();
        // 等待回调触发（轮询 + 短暂 sleep）
        for _ in 0..100 {
            if ready.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(ready.load(Ordering::SeqCst), "扫描完成后应触发 on_ready");
        cleanup(&base);
    }

    #[test]
    fn async_open_large_library_is_immediate() {
        // 模拟大图库：20 个文件夹 × 各 30 张图
        let base = temp_base();
        for d in 0..20 {
            for i in 0..30 {
                touch(
                    &base
                        .join(format!("D{d:02}"))
                        .join(format!("img_{i:03}.png")),
                );
            }
        }
        // open 应立即可用（不等后台枚举）
        let start = std::time::Instant::now();
        let m = BrowseModel::open(&base.join("D05/img_010.png"), None).unwrap();
        let open_elapsed = start.elapsed();
        assert!(
            open_elapsed < std::time::Duration::from_millis(50),
            "open 应 <50ms（仅枚举当前文件夹），实际 {open_elapsed:?}"
        );
        assert_eq!(m.state().file_name, "img_010.png", "首图立即可用");
        assert_eq!(
            m.state().global_total,
            30,
            "初始已知总数 = 当前文件夹图片数"
        );
        // 后台完成后总数 = 20×30=600
        m.wait_ready();
        assert_eq!(m.state().global_total, 600, "扫描完成后全局总数正确");
        assert_eq!(m.state().global_index, 5 * 30 + 10, "全局位置累计正确");
        cleanup(&base);
    }

    #[test]
    fn nav_waits_for_unscanned_folder() {
        // 打开后立即跨文件夹导航：应等待目标文件夹枚举完成（Condvar）
        let base = build_tree();
        let mut m = BrowseModel::open(&base.join("A/a10.png"), None).unwrap();
        assert!(
            matches!(m.next(), Nav::Ok),
            "A 末尾 next 应进入 B（等待枚举）"
        );
        assert_eq!(m.state().folder_name, "B");
        assert_eq!(m.state().file_name, "b1.jpg");
        cleanup(&base);
    }

    #[test]
    fn context_paths_forward_in_first_folder() {
        // 首文件夹中间图向前：应在当前文件夹内取（回归：fi==0 时被整段跳过）
        let base = build_tree();
        let m = open_sync(&base.join("A/a2.png"));
        let ctx = m.context_paths(2, 0);
        assert_eq!(ctx.len(), 1, "A 是第一文件夹，a2 向前只有 a1 一张");
        assert_eq!(ctx[0].file_name().unwrap(), "a1.png", "向前第 1 张");
        cleanup(&base);
    }

    #[test]
    fn context_paths_cross_folder_both_directions() {
        // B/b1：向前 1 应到 A 的 a10（跨文件夹末尾），向后 2 应到 b2 与 C 的 c1
        let base = build_tree();
        let m = open_sync(&base.join("B/b1.jpg"));
        let ctx = m.context_paths(1, 2);
        let names: Vec<String> = ctx
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect();
        assert_eq!(names, vec!["a10.png", "b2.jpg", "c1.png"], "前1后2（跨文件夹）");
        cleanup(&base);
    }

    #[test]
    fn context_paths_at_global_boundaries() {
        let base = build_tree();
        // 全局第一张：向前 0，向后应 C 内 2 张 + B 的 2 张...
        let m = open_sync(&base.join("A/a1.png"));
        assert!(m.context_paths(1, 0).is_empty(), "全局第一张无向前");
        // 全局最后一张：向后 0
        let m2 = open_sync(&base.join("C/c1.png"));
        assert!(m2.context_paths(0, 2).is_empty(), "全局最后一张无向后");
        cleanup(&base);
    }
}
