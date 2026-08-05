//! 导航：图片级（next/prev）跨文件夹无缝衔接 + 文件夹级跳转（jump_folder）。
//! 导航到未填充文件夹时阻塞等待（Condvar）；跨文件夹时跳过空文件夹（P2-2）。

use std::sync::{Arc, MutexGuard};

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
        } else if let Some((nf, _len, mut d)) =
            Self::find_nonempty_folder(&self.inner, d, fi + 1, true)
        {
            d.folder_index = nf;
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
            if let Some((pf, plen, mut d)) =
                Self::find_nonempty_folder(&self.inner, d, fi - 1, false)
            {
                d.folder_index = pf;
                d.image_index = plen - 1;
                Nav::Ok
            } else {
                Nav::Boundary(Boundary::FirstImage)
            }
        } else {
            Nav::Boundary(Boundary::FirstImage)
        }
    }

    /// 文件夹级跳转（PgUp/PgDn / 首末按钮），目标显示该文件夹第一张
    /// 跳过空文件夹：跳转到实际含图的首/末/相邻文件夹（P2-2）
    pub fn jump_folder(&mut self, target: FolderTarget) -> Nav {
        let d = self.inner.m.lock().unwrap();
        match target {
            FolderTarget::First => {
                if let Some((f0, _len, mut d)) = Self::find_nonempty_folder(&self.inner, d, 0, true)
                {
                    d.folder_index = f0;
                    d.image_index = 0;
                    Nav::Ok
                } else {
                    Nav::Boundary(Boundary::FirstFolder)
                }
            }
            FolderTarget::Last => {
                let last = d.folders.len().saturating_sub(1);
                if let Some((lf, _len, mut d)) =
                    Self::find_nonempty_folder(&self.inner, d, last, false)
                {
                    d.folder_index = lf;
                    d.image_index = 0;
                    Nav::Ok
                } else {
                    Nav::Boundary(Boundary::LastFolder)
                }
            }
            FolderTarget::Prev => {
                let cur = d.folder_index;
                if cur > 0 {
                    if let Some((pf, _len, mut d)) =
                        Self::find_nonempty_folder(&self.inner, d, cur - 1, false)
                    {
                        d.folder_index = pf;
                        d.image_index = 0;
                        Nav::Ok
                    } else {
                        Nav::Boundary(Boundary::FirstFolder)
                    }
                } else {
                    Nav::Boundary(Boundary::FirstFolder)
                }
            }
            FolderTarget::Next => {
                let cur = d.folder_index;
                if cur + 1 < d.folders.len() {
                    if let Some((nf, _len, mut d)) =
                        Self::find_nonempty_folder(&self.inner, d, cur + 1, true)
                    {
                        d.folder_index = nf;
                        d.image_index = 0;
                        Nav::Ok
                    } else {
                        Nav::Boundary(Boundary::LastFolder)
                    }
                } else {
                    Nav::Boundary(Boundary::LastFolder)
                }
            }
        }
    }

    /// 在已持有锁的情况下等待某文件夹填充完成；返回 (长度, guard)。
    /// 若等待期间压缩已把列表变短（idx 越界），返回 (0, guard) —— 不 panic（P2-2 竞态）。
    fn wait_folder_len<'a>(
        inner: &Arc<ModelInner>,
        mut guard: MutexGuard<'a, InnerData>,
        idx: usize,
    ) -> (usize, MutexGuard<'a, InnerData>) {
        loop {
            if idx >= guard.folders.len() {
                return (0, guard); // 压缩已发生，目标索引失效
            }
            if let Some(imgs) = &guard.folders[idx].images {
                return (imgs.len(), guard);
            }
            if guard.cancelled {
                return (0, guard);
            }
            guard = inner.cv.wait(guard).unwrap();
        }
    }

    /// 从 start 起沿 direction（forward=true 向后 / false 向前）查找第一个**非空**文件夹；
    /// 返回 (索引, 图片数, guard)。越界、被取消或全部为空返回 None。
    fn find_nonempty_folder<'a>(
        inner: &Arc<ModelInner>,
        mut guard: MutexGuard<'a, InnerData>,
        start: usize,
        forward: bool,
    ) -> Option<(usize, usize, MutexGuard<'a, InnerData>)> {
        let mut idx = start;
        loop {
            let (len, g) = Self::wait_folder_len(inner, guard, idx);
            if g.cancelled {
                return None;
            }
            if len > 0 {
                return Some((idx, len, g));
            }
            guard = g;
            // 等待期间压缩可能已把列表变短：idx 越界 → 目标方向已无文件夹
            if idx >= guard.folders.len() {
                return None;
            }
            if forward {
                idx += 1;
            } else {
                if idx == 0 {
                    return None;
                }
                idx -= 1;
            }
        }
    }
}
