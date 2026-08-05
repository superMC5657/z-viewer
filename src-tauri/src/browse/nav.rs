//! 导航：图片级（next/prev）跨文件夹无缝衔接 + 文件夹级跳转（jump_folder）。
//! 导航到未填充文件夹时阻塞等待（Condvar）。

use std::sync::Arc;

use super::{Boundary, BrowseModel, FolderTarget, InnerData, ModelInner, Nav};

impl BrowseModel {
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
