//! 导航：图片级（next/prev）跨文件夹无缝衔接 + 文件夹级跳转（jump_folder）。
//! 阻塞等待只经 BrowseModel::wait_filled（mod.rs 唯一等待点）：当前文件夹
//! 长度与跨文件夹跳过都由此等待；空文件夹跳过、压缩重验内建其中。
//! 压缩致索引失效（Stale）时整个操作重试：压缩单次运行且压缩后全填充，
//! 重试必收敛（无等待、无自旋）。
//! 本文件无 pub(super)：公开方法即接口的一部分，其余均为私有实现。

use std::sync::{Arc, MutexGuard};

use super::{Boundary, BrowseModel, FolderTarget, Found, InnerData, ModelInner, Nav, Waited};

impl BrowseModel {
    /// 下一张：跨文件夹无缝衔接；全局最后一张返回 Boundary(LastImage)
    /// &self：仅经内部锁改状态，AppState 快照出 Arc 后即可导航（不阻塞其他命令）
    pub fn next(&self) -> Nav {
        loop {
            let guard = self
                .inner
                .m
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let entry_len = guard.folders.len();
            let fi = guard.folder_index;
            let (cur_len, d) = match Self::wait_filled(&self.inner, guard, fi, entry_len) {
                Waited::Ready(len, d) => (len, d),
                Waited::Over => return Nav::Boundary(Boundary::LastImage),
                Waited::Stale => continue,
            };
            let mut d = d;
            if d.image_index + 1 < cur_len {
                d.image_index += 1;
                d.global_index += 1;
                return Nav::Ok;
            }
            match Self::find_nonempty(&self.inner, d, fi + 1, true, entry_len) {
                Found::Hit(nf, _len, mut d) => {
                    Self::move_to_folder(&mut d, nf, 0);
                    return Nav::Ok;
                }
                Found::End => return Nav::Boundary(Boundary::LastImage),
                Found::Stale => continue,
            }
        }
    }

    /// 上一张：反向无缝衔接；全局第一张返回 Boundary(FirstImage)
    pub fn prev(&self) -> Nav {
        loop {
            let guard = self
                .inner
                .m
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let entry_len = guard.folders.len();
            let fi = guard.folder_index;
            let d = match Self::wait_filled(&self.inner, guard, fi, entry_len) {
                Waited::Ready(_, d) => d,
                Waited::Over => return Nav::Boundary(Boundary::FirstImage),
                Waited::Stale => continue,
            };
            let mut d = d;
            if d.image_index > 0 {
                d.image_index -= 1;
                d.global_index -= 1;
                return Nav::Ok;
            }
            if fi == 0 {
                return Nav::Boundary(Boundary::FirstImage);
            }
            match Self::find_nonempty(&self.inner, d, fi - 1, false, entry_len) {
                Found::Hit(pf, plen, mut d) => {
                    Self::move_to_folder(&mut d, pf, plen - 1);
                    return Nav::Ok;
                }
                Found::End => return Nav::Boundary(Boundary::FirstImage),
                Found::Stale => continue,
            }
        }
    }

    /// 跳过空文件夹：跳转到实际含图的首/末/相邻文件夹
    pub fn jump_folder(&self, target: FolderTarget) -> Nav {
        loop {
            let guard = self
                .inner
                .m
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let entry_len = guard.folders.len();
            // 四分支统一映射为 (start_idx, forward, boundary)，走同一执行路径
            let (start_idx, forward, boundary) = match target {
                FolderTarget::First => (0, true, Boundary::FirstFolder),
                FolderTarget::Last => (
                    guard.folders.len().saturating_sub(1),
                    false,
                    Boundary::LastFolder,
                ),
                FolderTarget::Prev => {
                    if guard.folder_index == 0 {
                        return Nav::Boundary(Boundary::FirstFolder);
                    }
                    (guard.folder_index - 1, false, Boundary::FirstFolder)
                }
                FolderTarget::Next => {
                    if guard.folder_index + 1 >= guard.folders.len() {
                        return Nav::Boundary(Boundary::LastFolder);
                    }
                    (guard.folder_index + 1, true, Boundary::LastFolder)
                }
            };
            match Self::find_nonempty(&self.inner, guard, start_idx, forward, entry_len) {
                Found::Hit(f, _len, mut d) => {
                    Self::move_to_folder(&mut d, f, 0);
                    return Nav::Ok;
                }
                Found::End => return Nav::Boundary(boundary),
                Found::Stale => continue,
            }
        }
    }

    /// 从 start 起沿 direction（forward=true 向后 / false 向前）查找第一个**非空**文件夹；
    /// 返回命中 / 走到尽头 / 压缩致索引失效（调用方整体重试）。
    /// 经 wait_filled 逐个等待：len==0 跳过并步进。entry_len 为操作入口长度，
    /// 每次返回前复验——压缩只缩表，长度变化即索引失效。
    fn find_nonempty<'a>(
        inner: &Arc<ModelInner>,
        guard: MutexGuard<'a, InnerData>,
        mut idx: usize,
        forward: bool,
        entry_len: usize,
    ) -> Found<'a> {
        let mut guard = guard;
        loop {
            let (len, g) = match Self::wait_filled(inner, guard, idx, entry_len) {
                Waited::Ready(len, g) => (len, g),
                Waited::Over => return Found::End,
                Waited::Stale => return Found::Stale,
            };
            guard = g;
            if len > 0 {
                return Found::Hit(idx, len, guard);
            }
            if forward {
                idx += 1;
            } else {
                if idx == 0 {
                    return Found::End;
                }
                idx -= 1;
            }
        }
    }

    /// 计算 folders[min(a,b)..max(a,b)) 中已填充图片数（未填充的 None 计 0）。
    /// 下标由调用方保证有效（find_nonempty 返回的索引 < folders.len()）。
    fn filled_in_range(d: &InnerData, a: usize, b: usize) -> usize {
        let (lo, hi) = if a < b { (a, b) } else { (b, a) };
        d.folders[lo..hi]
            .iter()
            .map(|f| f.images.as_ref().map_or(0, |v| v.len()))
            .sum()
    }

    /// 变更 folder_index 后校正 global_index（增量，不用全量重算）：
    /// prefix(new_fi) = prefix(old_fi) ± Σ[old_fi..new_fi)（已填充数），再加 new_ii。
    /// 相邻文件夹跳转 O(1)；First/Last 长距离跳转 O(range)（低频，可接受）。
    /// 只做增量前缀修正：不压缩、不重算总数（总数由扫描线程统一维护）。
    fn move_to_folder(d: &mut InnerData, new_fi: usize, new_ii: usize) {
        let old_fi = d.folder_index;
        let old_prefix = d.global_index - d.image_index; // 先取出旧 prefix（image_index 尚未改）
        let mid = Self::filled_in_range(d, old_fi, new_fi);
        let new_prefix = if new_fi > old_fi {
            old_prefix + mid
        } else {
            old_prefix - mid
        };
        d.folder_index = new_fi;
        d.image_index = new_ii;
        d.global_index = new_prefix + new_ii;
    }
}
