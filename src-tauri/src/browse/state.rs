//! 状态计算：当前浏览状态（BrowseState）、上下文路径（预取用）、
//! 文件夹路径查询，以及 Drop 时通知扫描线程退出。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{BrowseModel, BrowseState, ModelInner};

impl BrowseModel {
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

    /// 当前文件夹路径
    pub fn current_folder_path(&self) -> PathBuf {
        let d = self.inner.m.lock().unwrap();
        d.folders[d.folder_index].path.clone()
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

    /// 等待后台枚举完成（测试用）
    #[cfg(test)]
    pub fn wait_ready(&self) {
        let mut d = self.inner.m.lock().unwrap();
        while d.loading {
            d = self.inner.cv.wait(d).unwrap();
        }
    }

    pub fn state(&self) -> BrowseState {
        Self::state_from_inner(&self.inner)
    }

    pub(super) fn state_from_inner(inner: &Arc<ModelInner>) -> BrowseState {
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

                // 全局位置/总数：增量维护（open 初始化，导航/后台填充时更新），读取 O(1)
                let global_index = d.global_index;
                let global_total = d.global_total;

                let path = img
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                // 文件大小：按路径缓存（命中零系统调用；翻回上一张/幻灯片循环同一图
                // 不再 stat）。未命中才 fs::metadata 一次并写入缓存。
                let file_size = img
                    .as_ref()
                    .map(|p| {
                        if let Some(sz) = d.file_sizes.get(p) {
                            *sz
                        } else {
                            let sz = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
                            d.file_sizes.insert(p.clone(), sz);
                            sz
                        }
                    })
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
