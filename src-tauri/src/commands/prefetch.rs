//! 预取逻辑：导航成功后后台填充解码缓存（队列 B：上下文邻居；队列 A：文件夹首图）

use std::sync::Arc;

use crate::browse::BrowseModel;
use crate::cache::{DecodeCache, FolderFirst, FolderFirstCache};

use super::settings::AppSettings;

/// 导航成功后后台预取上下文图片（按缓存强度），填充解码缓存（8.2）
/// 仅缓存 RAW/动画（asset 通道由 WebView 自身缓存）；失败静默忽略
/// 队列 B：当前文件夹前后邻居预取（enabled 且 depth>0 时）
pub(super) fn prefetch_context(model: &BrowseModel, cache: &DecodeCache, settings: &AppSettings) {
    let (prev_n, next_n) = settings.neighbor_window();
    if prev_n + next_n == 0 {
        return; // 缓存关闭或 depth=0：不预加载
    }
    for p in model.context_paths(prev_n, next_n) {
        let path = p.to_string_lossy().to_string();
        // asset 通道（jpg/bmp/ico/svg）由前端 PrefetchPool 预热，Rust 不重复解码
        if crate::decode::is_asset_ext(&path) {
            continue;
        }
        if !cache.begin_prefetch(&path) {
            continue;
        }
        let cache = cache.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let result = crate::decode::load_image(&path, true).ok();
            if let Some(result) = result {
                // 实时加载可能已填充缓存（end_prefetch 清登记后我们 put 会覆盖，内容相同无害）
                // 仅缓存非 asset（asset 走 WebView 自带缓存）
                if result.mode != "asset" && !cache.peek(&path) {
                    cache.put(path.clone(), Arc::new(result));
                }
            }
            cache.end_prefetch(&path);
        });
    }
}

/// 队列 A：每个文件夹第一张图预取（跨文件夹跳转无延迟）
/// 当前文件夹的邻居文件夹首图 → FolderFirstCache
/// P3-1：目录枚举（first_image_of）移入 spawn_blocking，不在持有 AppState 锁时阻塞命令线程
pub(super) fn prefetch_folder_firsts(
    model: &BrowseModel,
    first_cache: &FolderFirstCache,
    settings: &AppSettings,
) {
    if !settings.is_enabled() || settings.folder_first_depth == 0 {
        return;
    }
    let depth = settings.folder_first_depth.min(3); // 首图队列深度上限 3
    // 锁内仅收集邻居文件夹路径（快速克隆），枚举与解码全部移入后台线程
    let folders = model.neighbor_folders(depth);
    for folder in folders {
        let folder_str = folder.to_string_lossy().to_string();
        if first_cache.peek(&folder_str) {
            continue;
        }
        let first_cache = first_cache.clone();
        tauri::async_runtime::spawn_blocking(move || {
            // 取该文件夹第一张图并解码入队（后台线程，不阻塞导航命令）
            let Some(first) = BrowseModel::first_image_of(&folder) else { return };
            let path = first.to_string_lossy().to_string();
            if crate::decode::is_asset_ext(&path) {
                return; // asset 由前端池预热
            }
            if let Ok(result) = crate::decode::load_image(&path, true) {
                if result.mode != "asset" {
                    first_cache.put(
                        folder_str,
                        Arc::new(FolderFirst {
                            path,
                            result: Arc::new(result),
                        }),
                    );
                }
            }
        });
    }
}

/// 导航进入新文件夹时，把 FolderFirstCache 命中的首图并入 DecodeCache
/// （前端 load_image 按图片路径查队列 B，命中即零解码）
/// 优化：首图路径已随缓存值保存（FolderFirst.path），无需再 list_images 扫目录定位，
/// 避免在持有 AppState 锁的导航路径上做同步文件系统枚举（旧版会阻塞全部导航命令）
pub(super) fn promote_folder_first(
    model: &BrowseModel,
    cache: &DecodeCache,
    first_cache: &FolderFirstCache,
) {
    let folder = model.current_folder_path();
    let Some(first) = first_cache.get(&folder.to_string_lossy()) else {
        return; // 该文件夹首图未预取（无命中）
    };
    // 首图路径（同一张图）并入队列 B，前端 load_image 查 path 命中
    cache.put(first.path.clone(), first.result.clone());
}
