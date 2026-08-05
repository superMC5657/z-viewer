//! 全局浏览模型：跨同级文件夹无缝浏览（核心特色）
//!
//! 模型结构（见《需求报告与技术方案.md》8.1）：
//! - folders：父目录下所有「含图片」的同级文件夹，按 natord 自然排序（等价资源管理器）
//! - 每个文件夹内的图片按扩展名白名单过滤 + natord 自然排序
//! - next/prev 在图片级无缝跨文件夹衔接，全局首尾触发边界事件

use std::path::{Path, PathBuf};

/// M1 支持的常见格式（RAW 为 M3 范围）
const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "gif", "webp", "bmp", "ico", "svg"];

fn is_image_file(name: &str) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    IMAGE_EXTS.contains(&ext.as_str())
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

/// 单个文件夹：路径 + 自然排序后的图片列表
struct Folder {
    path: PathBuf,
    images: Vec<PathBuf>,
}

pub struct BrowseModel {
    folders: Vec<Folder>,
    folder_index: usize,
    image_index: usize,
}

/// 给前端的当前浏览状态（纯数据）
#[derive(serde::Serialize, Clone)]
pub struct BrowseState {
    pub path: String,
    pub file_name: String,
    pub folder_name: String,
    pub file_size: u64,
    /// 全局位置（0-based，跨文件夹累计）
    pub global_index: usize,
    pub global_total: usize,
    /// 当前文件夹在同级文件夹中的位置
    pub folder_index: usize,
    pub folder_total: usize,
}

impl BrowseModel {
    /// 以某张图片为起点建立浏览模型；图片或其父目录无效时返回 None
    pub fn open(path: &Path) -> Option<Self> {
        if !path.is_file() || !is_image_file(&path.file_name()?.to_string_lossy()) {
            return None;
        }
        let parent = path.parent()?;
        if !parent.is_dir() {
            return None;
        }

        let current_folder_canon = canonical(parent);

        // 同级文件夹（含自身）：枚举图片所在目录的兄弟目录；盘根目录时仅自身
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

        let mut folders: Vec<Folder> = Vec::new();
        for d in &dirs {
            let images = Self::list_images(d);
            if !images.is_empty() {
                folders.push(Folder {
                    path: d.clone(),
                    images,
                });
            }
        }

        let folder_index = folders
            .iter()
            .position(|f| canonical(&f.path) == current_folder_canon)?;

        let folder = &folders[folder_index];
        let file_name = path.file_name()?.to_string_lossy().to_string();
        let image_index = folder.images.iter().position(|img| {
            img.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .is_some_and(|n| n.eq_ignore_ascii_case(&file_name))
        })?;

        Some(Self {
            folders,
            folder_index,
            image_index,
        })
    }

    /// 打开目录：取目录内第一张图片（自然排序）作为起点
    pub fn open_first_in_dir(dir: &Path) -> Option<Self> {
        let first = Self::list_images(dir).into_iter().next()?;
        Self::open(&first)
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

    // ---------- 导航 ----------

    /// 下一张：跨文件夹无缝衔接；全局最后一张返回 Boundary(LastImage)
    pub fn next(&mut self) -> Nav {
        let cur_len = self.folders[self.folder_index].images.len();
        if self.image_index + 1 < cur_len {
            self.image_index += 1;
            Nav::Ok
        } else if self.folder_index + 1 < self.folders.len() {
            self.folder_index += 1;
            self.image_index = 0;
            Nav::Ok
        } else {
            Nav::Boundary(Boundary::LastImage)
        }
    }

    /// 上一张：反向无缝衔接；全局第一张返回 Boundary(First)
    pub fn prev(&mut self) -> Nav {
        if self.image_index > 0 {
            self.image_index -= 1;
            Nav::Ok
        } else if self.folder_index > 0 {
            self.folder_index -= 1;
            self.image_index = self.folders[self.folder_index].images.len() - 1;
            Nav::Ok
        } else {
            Nav::Boundary(Boundary::FirstImage)
        }
    }

    /// 文件夹级跳转（PgUp/PgDn / 首末按钮），目标显示该文件夹第一张
    pub fn jump_folder(&mut self, target: FolderTarget) -> Nav {
        match target {
            FolderTarget::First => {
                self.folder_index = 0;
                self.image_index = 0;
                Nav::Ok
            }
            FolderTarget::Last => {
                self.folder_index = self.folders.len() - 1;
                self.image_index = 0;
                Nav::Ok
            }
            FolderTarget::Prev => {
                if self.folder_index > 0 {
                    self.folder_index -= 1;
                    self.image_index = 0;
                    Nav::Ok
                } else {
                    Nav::Boundary(Boundary::FirstFolder)
                }
            }
            FolderTarget::Next => {
                if self.folder_index + 1 < self.folders.len() {
                    self.folder_index += 1;
                    self.image_index = 0;
                    Nav::Ok
                } else {
                    Nav::Boundary(Boundary::LastFolder)
                }
            }
        }
    }

    // ---------- 状态 ----------

    pub fn state(&self) -> BrowseState {
        let folder = &self.folders[self.folder_index];
        let img = &folder.images[self.image_index];
        let file_name = img
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let folder_name = folder
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| folder.path.to_string_lossy().to_string());

        let global_index: usize = self
            .folders
            .iter()
            .take(self.folder_index)
            .map(|f| f.images.len())
            .sum::<usize>()
            + self.image_index;
        let global_total: usize = self.folders.iter().map(|f| f.images.len()).sum();

        BrowseState {
            path: img.to_string_lossy().to_string(),
            file_name,
            folder_name,
            file_size: std::fs::metadata(img).map(|m| m.len()).unwrap_or(0),
            global_index,
            global_total,
            folder_index: self.folder_index,
            folder_total: self.folders.len(),
        }
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

    #[test]
    fn natural_order_and_folders() {
        let base = build_tree();
        let m = BrowseModel::open(&base.join("A/a1.png")).unwrap();
        assert_eq!(m.folders.len(), 3, "无图文件夹应被排除");
        assert_eq!(m.folders[0].images.len(), 3);
        assert_eq!(
            m.folders[0].images[2].file_name().unwrap(),
            "a10.png",
            "natord 自然排序：a1, a2, a10"
        );
        cleanup(&base);
    }

    #[test]
    fn cross_folder_next_and_boundary() {
        let base = build_tree();
        let mut m = BrowseModel::open(&base.join("A/a10.png")).unwrap();
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
        let mut m = BrowseModel::open(&base.join("C/c1.png")).unwrap();
        assert!(matches!(m.prev(), Nav::Ok));
        assert_eq!(m.state().folder_name, "B");
        assert_eq!(
            m.state().file_name,
            "b2.jpg",
            "反向跨文件夹进入上一文件夹最后一张"
        );
        while m.folder_index > 0 || m.image_index > 0 {
            assert!(matches!(m.prev(), Nav::Ok));
        }
        assert!(matches!(m.prev(), Nav::Boundary(Boundary::FirstImage)));
        cleanup(&base);
    }

    #[test]
    fn jump_folder_and_folder_boundary() {
        let base = build_tree();
        let mut m = BrowseModel::open(&base.join("A/a2.png")).unwrap();
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
        let mut m = BrowseModel::open(&base.join("A/a1.png")).unwrap();
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
        let m = BrowseModel::open_first_in_dir(&base.join("B")).unwrap();
        assert_eq!(m.state().file_name, "b1.jpg");
        cleanup(&base);
    }

    #[cfg(windows)]
    #[test]
    fn open_case_insensitive() {
        let base = build_tree();
        // Windows 文件系统大小写不敏感：传大写文件名应能定位
        let m = BrowseModel::open(&base.join("A/A2.PNG")).expect("Windows 上应大小写不敏感");
        assert_eq!(m.state().file_name, "a2.png");
        cleanup(&base);
    }
}
