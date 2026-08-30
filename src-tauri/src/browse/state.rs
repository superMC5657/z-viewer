//! 状态计算：当前浏览状态（BrowseState）、上下文路径（预取用）、
//! 文件夹路径查询，以及 Drop 时通知扫描线程退出。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{BrowseModel, BrowseState, ModelInner};

impl BrowseModel {
    /// 当前图片上下文的路径（跨文件夹衔接），用于预加载缓存（8.2）
    /// 前 prev_n 张 + 后 next_n 张；未填充的文件夹方向跳过（预取是优化，不等待）
    pub fn context_paths(&self, prev_n: usize, next_n: usize) -> Vec<PathBuf> {
        let d = self.inner.m.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
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
        let d = self.inner.m.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        d.folders[d.folder_index].path.clone()
    }

    /// 当前文件夹相邻的文件夹路径（前 depth 个 + 后 depth 个，跨文件夹跳转预取队列 A 用）
    pub fn neighbor_folders(&self, depth: usize) -> Vec<PathBuf> {
        let d = self.inner.m.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let fi = d.folder_index;
        let mut out = Vec::new();
        for i in (fi.saturating_sub(depth))..fi {
            out.push(d.folders[i].path.clone());
        }
        // 防御式上界：len 为 0 或 fi 越界时静默返回空（等价旧式的 ..=(fi+depth).min(len-1)，
        // 但空列表不再 len-1 下溢 panic —— open/压缩正常保证非空，此处只兜异常路径）
        let end = (fi + depth + 1).min(d.folders.len());
        for i in (fi + 1)..end {
            out.push(d.folders[i].path.clone());
        }
        out
    }

    /// 取某文件夹的第一张图片（自然排序首张）。
    /// 流式取最小者：单次遍历维护当前最小，不做全量排序
    /// （get_context/预取只取首图，大目录下省 O(n log n) 次比较与全量分配）
    pub fn first_image_of(dir: &Path) -> Option<PathBuf> {
        let rd = std::fs::read_dir(dir).ok()?;
        let mut best: Option<(String, PathBuf)> = None;
        for entry in rd.flatten() {
            let p = entry.path();
            // 与 list_images 相同的文件判定（file_type 快路径 + is_file 兜底）
            let is_file = match entry.file_type() {
                Ok(ft) => ft.is_file() || p.is_file(),
                Err(_) => p.is_file(),
            };
            if !is_file {
                continue;
            }
            if let Some(name) = p.file_name() {
                let name = name.to_string_lossy().to_string();
                if !super::is_image_file(&name) {
                    continue;
                }
                let replace = match &best {
                    Some((bn, _)) => natord::compare(&name, bn) == std::cmp::Ordering::Less,
                    None => true,
                };
                if replace {
                    best = Some((name, p));
                }
            }
        }
        best.map(|(_, p)| p)
    }

    /// 等待后台枚举完成（测试用）
    #[cfg(test)]
    pub fn wait_ready(&self) {
        let mut d = self.inner.m.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        while d.loading {
            d = self.inner.cv.wait(d).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    pub fn state(&self) -> BrowseState {
        Self::state_from_inner(&self.inner)
    }

    pub(super) fn state_from_inner(inner: &Arc<ModelInner>) -> BrowseState {
        // 第一段（持模型主锁）：等待当前文件夹填充，取一份 O(1) 快照后立即释放锁
        let snapshot = {
            let mut d = inner.m.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            loop {
                if let Some(imgs) = &d.folders[d.folder_index].images {
                    let folder = &d.folders[d.folder_index];
                    let img = imgs.get(d.image_index).cloned();
                    break (
                        folder.path.clone(),
                        img,
                        d.global_index,
                        d.global_total,
                        d.folder_index,
                        d.folders.len(),
                        d.loading,
                    );
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
                d = inner
                    .cv
                    .wait(d)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        };
        let (folder_path, img, global_index, global_total, folder_index, folder_total, loading) =
            snapshot;

        // 第二段（无锁）：文件大小按路径缓存（命中零系统调用；翻回上一张/幻灯片
        // 循环同一图不再 stat）。未命中才 fs::metadata —— 不持模型主锁（慢盘上
        // stat 可达毫秒级，不能阻塞导航/扫描线程），结果回写独立锁缓存。
        // 即使 stat 期间用户已翻页，这也是一份自洽的历史快照（前端 showSeq 兜底）。
        let file_size = match &img {
            Some(p) => {
                let cached = inner
                    .file_sizes
                    .lock()
                    .ok()
                    .and_then(|fs| fs.get(p).copied());
                match cached {
                    Some(sz) => sz,
                    None => {
                        let sz = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
                        if let Ok(mut fs) = inner.file_sizes.lock() {
                            fs.insert(p.clone(), sz);
                        }
                        sz
                    }
                }
            }
            None => 0,
        };

        let file_name = img
            .as_ref()
            .and_then(|p| p.file_name())
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let folder_name = folder_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| folder_path.to_string_lossy().to_string());
        let path = img
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        BrowseState {
            path,
            file_name,
            folder_name,
            file_size,
            global_index,
            global_total,
            folder_index,
            folder_total,
            loading,
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
