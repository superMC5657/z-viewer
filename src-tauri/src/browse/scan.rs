//! 模型构建与后台扫描：open 同步枚举当前文件夹定位首图，
//! 兄弟文件夹由后台线程逐个填充；扫描完成回调 + 压缩空文件夹。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{canonical, is_image_file, BrowseModel, Folder, InnerData, ModelInner, OnReady};

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
            natord::compare(&an, &bn)
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
            m: std::sync::Mutex::new(InnerData {
                folders,
                folder_index,
                image_index,
                loading: pending_total > 1,
                cancelled: false,
            }),
            cv: std::sync::Condvar::new(),
        });

        // 后台扫描兄弟文件夹
        let scan_inner = Arc::clone(&inner);
        std::thread::spawn(move || background_scan(scan_inner, on_ready));

        Some(Self { inner })
    }

    /// 打开目录：取目录内第一张图片（自然排序）作为起点
    pub fn open_first_in_dir(dir: &Path, on_ready: Option<OnReady>) -> Option<Self> {
        let first = Self::list_images(dir).into_iter().next()?;
        Self::open(&first, on_ready)
    }

    pub(super) fn list_images(dir: &Path) -> Vec<PathBuf> {
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
            natord::compare(
                &a.file_name().unwrap_or_default().to_string_lossy(),
                &b.file_name().unwrap_or_default().to_string_lossy(),
            )
        });
        imgs
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
        d.folders[idx].images = Some(imgs);
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
    drop(d); // 先释放锁，state_from_inner 会再次加锁
    let state = BrowseModel::state_from_inner(&scan_inner);
    if let Some(cb) = on_ready {
        cb(state);
    }
    scan_inner.cv.notify_all();
}
