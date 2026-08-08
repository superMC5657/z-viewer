//! 模型构建与后台扫描：open 同步枚举当前文件夹定位首图，
//! 兄弟文件夹由后台线程逐个填充；扫描完成回调 + 压缩空文件夹。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{canonical, is_image_file, BrowseModel, Folder, InnerData, ModelInner, OnReady};

impl BrowseModel {
    /// 以某张图片为起点建立浏览模型（完整功能：跨文件夹扫描）
    /// 等价于 open_gated(..., cross_folder=true)；仅测试使用
    #[cfg(test)]
    pub fn open(path: &Path, on_ready: Option<OnReady>) -> Option<Self> {
        Self::open_gated(path, on_ready, true)
    }

    /// 门控打开：cross_folder=false（免费版）时只浏览当前文件夹 ——
    /// 兄弟文件夹不扫描、不后台填充、不启动扫描线程，
    /// next/prev 自然停在文件夹边界，jump_folder 亦无目标可跳。
    /// 付费解锁（专业版）后调用方传入 true 恢复跨文件夹无缝浏览。
    pub fn open_gated(path: &Path, on_ready: Option<OnReady>, cross_folder: bool) -> Option<Self> {
        if !path.is_file() || !is_image_file(&path.file_name()?.to_string_lossy()) {
            return None;
        }
        let parent = path.parent()?;
        if !parent.is_dir() {
            return None;
        }

        let current_folder_canon = canonical(parent);

        // 同级文件夹（含自身）：免费版仅当前文件夹；专业版枚举兄弟目录
        // （免费版不枚举兄弟：既不泄露功能存在，也省一次目录遍历）
        let mut dirs: Vec<(String, PathBuf)> = Vec::new();
        if cross_folder {
            match parent.parent() {
                Some(base) => {
                    if let Ok(rd) = std::fs::read_dir(base) {
                        for entry in rd.flatten() {
                            let p = entry.path();
                            if p.is_dir() {
                                if let Some(name) = p.file_name() {
                                    dirs.push((name.to_string_lossy().to_string(), p));
                                }
                            }
                        }
                    }
                }
                None => {
                    let name = parent
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    dirs.push((name, parent.to_path_buf()));
                }
            }
        } else {
            let name = parent
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            dirs.push((name, parent.to_path_buf()));
        }
        dirs.sort_by(|a, b| natord::compare(&a.0, &b.0));
        let dirs: Vec<PathBuf> = dirs.into_iter().map(|(_, p)| p).collect();

        // 同步枚举当前文件夹（定位当前图片必需）
        let current_images = Self::list_images(parent);
        let file_name = path.file_name()?.to_string_lossy().to_string();
        let image_index = current_images.iter().position(|img| {
            img.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .is_some_and(|n| n.eq_ignore_ascii_case(&file_name))
        })?;

        // 构建 folders：当前文件夹已填充，其余 None（后台填充）
        // 优化：用文件夹名定位当前文件夹（同级目录名唯一），
        // 免去对每个兄弟目录做 std::fs::canonicalize 系统调用（N 目录 = N 次 fs 访问）
        let current_folder_name = current_folder_canon
            .file_name()
            .unwrap_or_default()
            .to_os_string();
        let mut folder_index = None;
        let mut folders = Vec::with_capacity(dirs.len());
        for (i, d) in dirs.iter().enumerate() {
            if d.file_name().unwrap_or_default() == current_folder_name.as_os_str() {
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
        // 免费版（cross_folder=false）无待填充文件夹，loading=false
        let loading = cross_folder && pending_total > 1;

        let inner = Arc::new(ModelInner {
            m: std::sync::Mutex::new(InnerData {
                folders,
                folder_index,
                image_index,
                // 初始仅当前文件夹已填充：总数 = 当前图片数，位置 = 当前索引
                global_total: current_images.len(),
                global_index: image_index,
                loading,
                cancelled: false,
            }),
            cv: std::sync::Condvar::new(),
        });

        // 后台扫描兄弟文件夹（免费版无兄弟可扫，跳过）
        if cross_folder && pending_total > 1 {
            let scan_inner = Arc::clone(&inner);
            std::thread::spawn(move || background_scan(scan_inner, on_ready));
        } else if let Some(cb) = on_ready {
            // 免费版/单文件夹：无后台扫描，立即回调（状态即最终态）
            cb(BrowseModel::state_from_inner(&inner));
        }

        Some(Self { inner })
    }

    /// 打开目录（门控）：取目录内第一张图片（自然排序）作为起点
    pub fn open_first_in_dir_gated(dir: &Path, on_ready: Option<OnReady>, cross_folder: bool) -> Option<Self> {
        let first = Self::list_images(dir).into_iter().next()?;
        Self::open_gated(&first, on_ready, cross_folder)
    }

    /// 打开目录（完整功能，等价 cross_folder=true；仅测试使用）
    #[cfg(test)]
    pub fn open_first_in_dir(dir: &Path, on_ready: Option<OnReady>) -> Option<Self> {
        Self::open_first_in_dir_gated(dir, on_ready, true)
    }

    pub(super) fn list_images(dir: &Path) -> Vec<PathBuf> {
        // 预计算文件名（每图片一次分配）排序后再丢弃，避免比较器内 O(n log n) 次分配
        let mut imgs: Vec<(String, PathBuf)> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(dir) {
            for entry in rd.flatten() {
                let p = entry.path();
                if p.is_file() {
                    if let Some(name) = p.file_name() {
                        let name = name.to_string_lossy().to_string();
                        if is_image_file(&name) {
                            imgs.push((name, p));
                        }
                    }
                }
            }
        }
        imgs.sort_by(|a, b| natord::compare(&a.0, &b.0));
        imgs.into_iter().map(|(_, p)| p).collect()
    }
}

/// 后台扫描线程主循环：逐个填充兄弟文件夹 → 压缩空文件夹 → 回调 on_ready
fn background_scan(scan_inner: Arc<ModelInner>, on_ready: Option<OnReady>) {
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
        let imgs = BrowseModel::list_images(&path);
        let mut d = scan_inner.m.lock().unwrap();
        if d.cancelled {
            return;
        }
        // 增量维护全局计数：总数 +len；当前文件夹之前的新填充 → 全局位置前移 +len
        let filled = imgs.len();
        d.folders[idx].images = Some(imgs);
        d.global_total += filled;
        if idx < d.folder_index {
            d.global_index += filled;
        }
    }

    // 全部填充完成：压缩空文件夹（open 时无法预知哪些兄弟为空）。
    // P2-2：若用户已导航进空文件夹，同样移除它并指向相邻非空文件夹，
    // 避免空文件夹永久残留、folder_total 虚高。
    let mut d = scan_inner.m.lock().unwrap();
    if d.cancelled {
        return;
    }
    let old_folders = std::mem::take(&mut d.folders);
    let old_index = d.folder_index;

    // 极端防御：全部为空（正常不会发生，open 时当前文件夹必有图）→ 保留原列表避免空索引
    let all_empty = old_folders
        .iter()
        .all(|f| matches!(&f.images, Some(imgs) if imgs.is_empty()));
    if all_empty {
        d.folders = old_folders;
        d.loading = false;
        drop(d);
        let state = BrowseModel::state_from_inner(&scan_inner);
        if let Some(cb) = on_ready {
            cb(state);
        }
        scan_inner.cv.notify_all();
        return;
    }

    let mut new_folders = Vec::with_capacity(old_folders.len());
    let mut removed_before = 0usize;
    let mut cur_removed = false;
    for (i, f) in old_folders.into_iter().enumerate() {
        let is_empty = matches!(&f.images, Some(imgs) if imgs.is_empty());
        if is_empty {
            if i == old_index {
                cur_removed = true;
            } else if i < old_index {
                removed_before += 1;
            }
            continue;
        }
        new_folders.push(f);
    }
    d.folders = new_folders;
    if cur_removed {
        // 当前空文件夹被移除：指向其后最近的非空文件夹（新索引 = old_index - removed_before）；
        // 若其后没有，指向前一个（末位）
        let after = old_index - removed_before;
        d.folder_index = if after < d.folders.len() {
            after
        } else {
            d.folders.len().saturating_sub(1)
        };
        d.image_index = 0;
    } else {
        d.folder_index = old_index - removed_before;
    }
    d.loading = false;
    // 压缩后一次性重算全局计数（此后为增量维护的基准）
    let (mut total, mut prefix) = (0usize, 0usize);
    for (i, f) in d.folders.iter().enumerate() {
        if let Some(imgs) = &f.images {
            total += imgs.len();
            if i < d.folder_index {
                prefix += imgs.len();
            }
        }
    }
    d.global_total = total;
    d.global_index = prefix + d.image_index;
    drop(d); // 先释放锁，state_from_inner 会再次加锁
    let state = BrowseModel::state_from_inner(&scan_inner);
    if let Some(cb) = on_ready {
        cb(state);
    }
    scan_inner.cv.notify_all();
}
