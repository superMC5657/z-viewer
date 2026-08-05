//! Tauri IPC commands：前端浏览逻辑的桥梁

use std::path::Path;
use std::sync::{Arc, Mutex};

use tauri::{Emitter, State};

use crate::browse::{Boundary, BrowseModel, BrowseState, FolderTarget};
use crate::cache::DecodeCache;

/// 后台扫描完成事件（携带最新 BrowseState，前端刷新位置计数）
pub const BROWSE_SCAN_READY: &str = "browse://scan-ready";

/// 全局浏览模型（Mutex：Tauri 命令在独立线程执行）
pub struct AppState(pub Mutex<Option<BrowseModel>>);

/// 用户设置（缓存强度等）
pub struct SettingsState(pub Mutex<AppSettings>);

/// 缓存强度（0..=10）：
/// - 0：不缓存不预加载
/// - 1：前后各 1 张
/// - 2..=10：前 1 张，后 2..=10 张（维护 LRU 缓存队列）
#[derive(serde::Serialize, Clone, Copy)]
pub struct AppSettings {
    pub cache_strength: usize,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self { cache_strength: 1 }
    }
}

impl AppSettings {
    /// 按强度换算预取数量：返回 (prev_n, next_n)
    pub fn prefetch_window(&self) -> (usize, usize) {
        let s = self.cache_strength.min(10);
        match s {
            0 => (0, 0),
            1 => (1, 1),
            n => (1, n), // 2..=10：前 1 后 n
        }
    }

    /// 按强度换算缓存容量（覆盖预取窗口 + 当前图 + 余量）
    pub fn cache_capacity(&self) -> usize {
        let s = self.cache_strength.min(10);
        match s {
            0 => 0,
            1 => 4,
            n => (2 * n + 1).max(8),
        }
    }
}

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

/// 导航成功后后台预取上下文图片（按缓存强度），填充解码缓存（8.2）
/// 仅缓存 RAW/动画（asset 通道由 WebView 自身缓存）；失败静默忽略
/// begin_prefetch 去重：同一路径已在解码中则跳过
fn prefetch_context(model: &BrowseModel, cache: &DecodeCache, settings: &AppSettings) {
    let (prev_n, next_n) = settings.prefetch_window();
    if prev_n + next_n == 0 {
        return; // 强度 0：不预加载
    }
    for p in model.context_paths(prev_n, next_n) {
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
/// 首图立即返回；兄弟文件夹后台枚举完成后 emit 事件供前端刷新计数
#[tauri::command]
pub fn open_path(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    cache: State<'_, DecodeCache>,
    settings: State<'_, SettingsState>,
    path: String,
) -> Result<NavResult, String> {
    let p = Path::new(&path);
    let app2 = app.clone();
    let on_ready = Some(Box::new(move |st: BrowseState| {
        let _ = app2.emit(BROWSE_SCAN_READY, st);
    }) as crate::browse::OnReady);
    let model = if p.is_dir() {
        BrowseModel::open_first_in_dir(p, on_ready)
    } else {
        BrowseModel::open(p, on_ready)
    };
    let model = model.ok_or_else(|| "无法打开：不是支持的图片格式".to_string())?;
    let st = model.state();
    {
        let s = settings.0.lock().map_err(|e| e.to_string())?;
        prefetch_context(&model, cache.inner(), &s);
    }
    *state.0.lock().map_err(|e| e.to_string())? = Some(model);
    Ok(NavResult::ok(st))
}

#[tauri::command]
pub fn next_image(
    state: State<'_, AppState>,
    cache: State<'_, DecodeCache>,
    settings: State<'_, SettingsState>,
) -> Result<NavResult, String> {
    let mut guard = state.0.lock().map_err(|e| e.to_string())?;
    match guard.as_mut() {
        Some(m) => {
            let nav = m.next();
            let r = nav_ok_or_none(m, nav);
            if r.state.is_some() {
                let s = settings.0.lock().map_err(|e| e.to_string())?;
                prefetch_context(m, cache.inner(), &s);
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
    settings: State<'_, SettingsState>,
) -> Result<NavResult, String> {
    let mut guard = state.0.lock().map_err(|e| e.to_string())?;
    match guard.as_mut() {
        Some(m) => {
            let nav = m.prev();
            let r = nav_ok_or_none(m, nav);
            if r.state.is_some() {
                let s = settings.0.lock().map_err(|e| e.to_string())?;
                prefetch_context(m, cache.inner(), &s);
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
    settings: State<'_, SettingsState>,
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
                let s = settings.0.lock().map_err(|e| e.to_string())?;
                prefetch_context(m, cache.inner(), &s);
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
/// 缓存强度 0 时跳过缓存读写（不缓存）
#[tauri::command]
pub async fn load_image(
    path: String,
    cache: State<'_, DecodeCache>,
    settings: State<'_, SettingsState>,
) -> Result<crate::decode::LoadResult, String> {
    let caching = settings.0.lock().map_err(|e| e.to_string())?.cache_strength > 0;
    if caching {
        if let Some(hit) = cache.get(&path) {
            return Ok((*hit).clone());
        }
    }
    let path2 = path.clone();
    let result = tauri::async_runtime::spawn_blocking(move || crate::decode::load_image(&path2))
        .await
        .map_err(|e| format!("解码任务失败: {e}"))??;
    if caching {
        // Arc 包装避免深拷贝（结果含 base64 JPEG/帧数据）
        let arc = Arc::new(result);
        // asset 空结果（单帧 gif/png/webp）不占缓存槽位
        if arc.mode != "asset" {
            cache.put(path, Arc::clone(&arc));
        }
        Ok((*arc).clone())
    } else {
        Ok(result)
    }
}

/// 设置缓存强度（0..=10），并联动缓存容量
/// - 0：不缓存不预加载；1：前后各 1；2..=10：前 1 后 n（LRU 队列）
#[tauri::command]
pub fn set_cache_strength(
    cache: State<'_, DecodeCache>,
    settings: State<'_, SettingsState>,
    strength: usize,
) -> Result<AppSettings, String> {
    let s = strength.min(10);
    let mut g = settings.0.lock().map_err(|e| e.to_string())?;
    g.cache_strength = s;
    cache.set_capacity(g.cache_capacity());
    Ok(*g)
}

/// 查询当前设置
#[tauri::command]
pub fn get_settings(settings: State<'_, SettingsState>) -> AppSettings {
    *settings.0.lock().unwrap_or_else(|e| e.into_inner())
}

/// 查询当前图片上下文路径（按缓存强度），供前端预解码池（方案四）预热
#[tauri::command]
pub fn get_context(
    state: State<'_, AppState>,
    settings: State<'_, SettingsState>,
) -> Result<Vec<String>, String> {
    let guard = state.0.lock().map_err(|e| e.to_string())?;
    let Some(model) = guard.as_ref() else {
        return Ok(Vec::new());
    };
    let (prev_n, next_n) = settings
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .prefetch_window();
    Ok(model
        .context_paths(prev_n, next_n)
        .into_iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefetch_window_rules() {
        let s = |n| AppSettings { cache_strength: n };
        assert_eq!(s(0).prefetch_window(), (0, 0), "0：不预加载");
        assert_eq!(s(1).prefetch_window(), (1, 1), "1：前后各 1");
        assert_eq!(s(2).prefetch_window(), (1, 2), "2：前 1 后 2");
        assert_eq!(s(5).prefetch_window(), (1, 5), "5：前 1 后 5");
        assert_eq!(s(10).prefetch_window(), (1, 10), "10：前 1 后 10");
        assert_eq!(s(99).prefetch_window(), (1, 10), "超限钳制到 10");
    }

    #[test]
    fn cache_capacity_rules() {
        let s = |n| AppSettings { cache_strength: n };
        assert_eq!(s(0).cache_capacity(), 0, "0：不缓存");
        assert_eq!(s(1).cache_capacity(), 4);
        assert_eq!(s(2).cache_capacity(), 8, "2*2+1=5 → 下限 8");
        assert_eq!(s(5).cache_capacity(), 11);
        assert_eq!(s(10).cache_capacity(), 21);
    }
}
