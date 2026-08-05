//! Tauri IPC commands：前端浏览逻辑的桥梁

use std::path::Path;
use std::sync::{Arc, Mutex};

use tauri::State;

use crate::browse::{Boundary, BrowseModel, BrowseState, FolderTarget};
use crate::cache::DecodeCache;

/// 全局浏览模型（Mutex：Tauri 命令在独立线程执行）
pub struct AppState(pub Mutex<Option<BrowseModel>>);

/// 导航结果：boundary 为 Some 时表示撞到全局边界（前端弹 Toast），state 不变
#[derive(serde::Serialize)]
pub struct NavResult {
    pub boundary: Option<String>, // "first" | "last"
    pub state: Option<BrowseState>,
}

impl NavResult {
    fn ok(state: BrowseState) -> Self {
        Self {
            boundary: None,
            state: Some(state),
        }
    }
    fn boundary(b: Boundary, state: BrowseState) -> Self {
        Self {
            boundary: Some(
                match b {
                    Boundary::FirstImage => "first-image",
                    Boundary::LastImage => "last-image",
                    Boundary::FirstFolder => "first-folder",
                    Boundary::LastFolder => "last-folder",
                }
                .to_string(),
            ),
            state: Some(state),
        }
    }
    fn none() -> Self {
        Self {
            boundary: None,
            state: None,
        }
    }
}

fn nav_ok_or_none(model: &mut BrowseModel, nav: crate::browse::Nav) -> NavResult {
    match nav {
        crate::browse::Nav::Ok => NavResult::ok(model.state()),
        crate::browse::Nav::Boundary(b) => NavResult::boundary(b, model.state()),
    }
}

/// 导航成功后后台预取相邻图片（前1后1，跨文件夹），填充解码缓存（8.2）
/// 仅缓存 RAW/动画（asset 通道由 WebView 自身缓存）；失败静默忽略
/// begin_prefetch 去重：同一路径已在解码中则跳过
fn prefetch_neighbors(model: &BrowseModel, cache: &DecodeCache) {
    let (prev, next) = model.neighbor_paths();
    for p in [prev, next].into_iter().flatten() {
        let path = p.to_string_lossy().to_string();
        if !cache.begin_prefetch(&path) {
            continue;
        }
        let cache = cache.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let result = crate::decode::load_image(&path).ok();
            if let Some(result) = result {
                if result.mode != "asset" {
                    cache.put(path.clone(), Arc::new(result));
                }
            }
            cache.end_prefetch(&path);
        });
    }
}

/// 打开指定路径（拖拽/命令行）：文件定位到浏览模型，目录取其首张图片
#[tauri::command]
pub fn open_path(
    state: State<'_, AppState>,
    cache: State<'_, DecodeCache>,
    path: String,
) -> Result<NavResult, String> {
    let p = Path::new(&path);
    let model = if p.is_dir() {
        BrowseModel::open_first_in_dir(p)
    } else {
        BrowseModel::open(p)
    };
    let model = model.ok_or_else(|| "无法打开：不是支持的图片格式".to_string())?;
    let st = model.state();
    prefetch_neighbors(&model, cache.inner());
    *state.0.lock().map_err(|e| e.to_string())? = Some(model);
    Ok(NavResult::ok(st))
}

#[tauri::command]
pub fn next_image(
    state: State<'_, AppState>,
    cache: State<'_, DecodeCache>,
) -> Result<NavResult, String> {
    let mut guard = state.0.lock().map_err(|e| e.to_string())?;
    match guard.as_mut() {
        Some(m) => {
            let nav = m.next();
            let r = nav_ok_or_none(m, nav);
            if r.state.is_some() {
                prefetch_neighbors(m, cache.inner());
            }
            Ok(r)
        }
        None => Ok(NavResult::none()),
    }
}

#[tauri::command]
pub fn prev_image(
    state: State<'_, AppState>,
    cache: State<'_, DecodeCache>,
) -> Result<NavResult, String> {
    let mut guard = state.0.lock().map_err(|e| e.to_string())?;
    match guard.as_mut() {
        Some(m) => {
            let nav = m.prev();
            let r = nav_ok_or_none(m, nav);
            if r.state.is_some() {
                prefetch_neighbors(m, cache.inner());
            }
            Ok(r)
        }
        None => Ok(NavResult::none()),
    }
}

/// 文件夹级跳转：target ∈ "first" | "prev" | "next" | "last"
#[tauri::command]
pub fn jump_folder(
    state: State<'_, AppState>,
    cache: State<'_, DecodeCache>,
    target: String,
) -> Result<NavResult, String> {
    let t = match target.as_str() {
        "first" => FolderTarget::First,
        "prev" => FolderTarget::Prev,
        "next" => FolderTarget::Next,
        "last" => FolderTarget::Last,
        _ => return Err(format!("未知跳转目标: {target}")),
    };
    let mut guard = state.0.lock().map_err(|e| e.to_string())?;
    match guard.as_mut() {
        Some(m) => {
            let nav = m.jump_folder(t);
            let r = nav_ok_or_none(m, nav);
            if r.state.is_some() {
                prefetch_neighbors(m, cache.inner());
            }
            Ok(r)
        }
        None => Ok(NavResult::none()),
    }
}

/// 启动时查询初始状态（命令行参数已由 main.rs 注入模型）
#[tauri::command]
pub fn get_initial_state(state: State<'_, AppState>) -> Option<BrowseState> {
    state.0.lock().ok()?.as_ref().map(|m| m.state())
}

/// 图片加载通道分发：常见格式 → asset（前端直读）；RAW → 解码 JPEG；动画 → 帧序列
/// 命中缓存直接返回（≤50ms 目标）；spawn_blocking 避免 CPU 密集解码占用 runtime 线程
#[tauri::command]
pub async fn load_image(
    path: String,
    cache: State<'_, DecodeCache>,
) -> Result<crate::decode::LoadResult, String> {
    if let Some(hit) = cache.get(&path) {
        return Ok((*hit).clone());
    }
    let path2 = path.clone();
    let result = tauri::async_runtime::spawn_blocking(move || crate::decode::load_image(&path2))
        .await
        .map_err(|e| format!("解码任务失败: {e}"))??;
    // Arc 包装避免深拷贝（结果含 base64 JPEG/帧数据）
    let arc = Arc::new(result);
    cache.put(path, Arc::clone(&arc));
    Ok((*arc).clone())
}
