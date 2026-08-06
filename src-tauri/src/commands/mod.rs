//! Tauri IPC commands：前端浏览逻辑的桥梁
//!
//! 文件组织（可维护性拆分）：
//! - mod.rs：Tauri 命令 + 全局状态类型 + 导航壳
//! - settings.rs：用户设置（AppSettings）
//! - prefetch.rs：预取三函数（队列 A/B）
//! - tests.rs：测试（#[cfg(test)] 独立文件，不混入产品代码）

mod prefetch;
mod settings;
#[cfg(test)]
mod tests;

use std::path::Path;
use std::sync::{Arc, Mutex};

use tauri::{Emitter, State};

use crate::browse::{Boundary, BrowseModel, BrowseState, FolderTarget, Nav};
use crate::cache::{DecodeCache, FolderFirstCache};

pub use settings::AppSettings;

#[cfg(debug_assertions)]
use crate::dev_log;
use prefetch::{prefetch_context, prefetch_folder_firsts, promote_folder_first};

/// 后台扫描完成事件（携带最新 BrowseState，前端刷新位置计数）
pub const BROWSE_SCAN_READY: &str = "browse://scan-ready";

/// 全局浏览模型（Mutex：Tauri 命令在独立线程执行）
pub struct AppState(pub Mutex<Option<BrowseModel>>);

/// 用户设置（缓存开关/深度）
pub struct SettingsState(pub Mutex<AppSettings>);

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
        let msg = match b {
            Boundary::FirstImage => "first-image",
            Boundary::LastImage => "last-image",
            Boundary::FirstFolder => "first-folder",
            Boundary::LastFolder => "last-folder",
        };
        Self {
            boundary: Some(msg.into()),
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

fn nav_ok_or_none(model: &mut BrowseModel, nav: Nav) -> NavResult {
    match nav {
        Nav::Ok => NavResult::ok(model.state()),
        Nav::Boundary(b) => NavResult::boundary(b, model.state()),
    }
}

/// 构造后台扫描完成回调：emit BROWSE_SCAN_READY 供前端刷新位置计数
/// （main.rs 启动注入与 open_path 共用同一实现）
pub fn on_ready_callback(app: tauri::AppHandle) -> crate::browse::OnReady {
    Box::new(move |st: BrowseState| {
        let _ = app.emit(BROWSE_SCAN_READY, st);
    })
}

/// 执行导航并处理后置动作（首图 promote + 邻居预取）；三个导航命令共用
fn navigate(
    state: &AppState,
    cache: &DecodeCache,
    first_cache: &FolderFirstCache,
    settings: &SettingsState,
    nav: impl FnOnce(&mut BrowseModel) -> Nav,
) -> Result<NavResult, String> {
    let mut guard = state.0.lock().map_err(|e| e.to_string())?;
    match guard.as_mut() {
        Some(m) => {
            let nav = nav(m);
            let r = nav_ok_or_none(m, nav);
            if r.state.is_some() {
                // 跨文件夹进入新文件夹：先把预取的首图并入队列 B（前端 load_image 命中）
                promote_folder_first(m, cache, first_cache);
                let s = settings.0.lock().map_err(|e| e.to_string())?;
                prefetch_context(m, cache, &s);
                prefetch_folder_firsts(m, first_cache, &s);
            }
            Ok(r)
        }
        None => Ok(NavResult::none()),
    }
}

/// 打开指定路径（拖拽/命令行）：文件定位到浏览模型，目录取其首张图片
/// 首图立即返回；兄弟文件夹后台枚举完成后 emit 事件供前端刷新计数
#[tauri::command]
pub fn open_path(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    cache: State<'_, DecodeCache>,
    first_cache: State<'_, FolderFirstCache>,
    settings: State<'_, SettingsState>,
    path: String,
) -> Result<NavResult, String> {
    let p = Path::new(&path);
    let on_ready = Some(on_ready_callback(app.clone()));
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
        prefetch_folder_firsts(&model, first_cache.inner(), &s);
    }
    *state.0.lock().map_err(|e| e.to_string())? = Some(model);
    Ok(NavResult::ok(st))
}

#[tauri::command]
pub fn next_image(
    state: State<'_, AppState>,
    cache: State<'_, DecodeCache>,
    first_cache: State<'_, FolderFirstCache>,
    settings: State<'_, SettingsState>,
) -> Result<NavResult, String> {
    let r = navigate(state.inner(), cache.inner(), first_cache.inner(), settings.inner(), |m| m.next());
    // 仅幻灯片自动播放使用的 next_image 打印跳转日志（手动 prev/next/jump_folder 不打）
    // 整个 if-let 用 cfg 门控：release 下连绑定一起剥离，避免 dev_log 空展开产生未使用变量警告
    #[cfg(debug_assertions)]
    if let Ok(result) = &r {
        if let Some(st) = &result.state {
            dev_log!("next_image 跳转: {} [{}] ({}/{})", st.file_name, st.folder_name, st.global_index + 1, st.global_total);
        } else if let Some(b) = &result.boundary {
            dev_log!("next_image 边界: {}", b);
        }
    }
    r
}

#[tauri::command]
pub fn prev_image(
    state: State<'_, AppState>,
    cache: State<'_, DecodeCache>,
    first_cache: State<'_, FolderFirstCache>,
    settings: State<'_, SettingsState>,
) -> Result<NavResult, String> {
    navigate(state.inner(), cache.inner(), first_cache.inner(), settings.inner(), |m| m.prev())
}

/// 文件夹级跳转：target ∈ "first" | "prev" | "next" | "last"
#[tauri::command]
pub fn jump_folder(
    state: State<'_, AppState>,
    cache: State<'_, DecodeCache>,
    first_cache: State<'_, FolderFirstCache>,
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
    navigate(state.inner(), cache.inner(), first_cache.inner(), settings.inner(), |m| m.jump_folder(t))
}

/// 启动时查询初始状态（命令行参数已由 main.rs 注入模型）
#[tauri::command]
pub fn get_initial_state(state: State<'_, AppState>) -> Option<BrowseState> {
    state.0.lock().ok()?.as_ref().map(|m| m.state())
}

/// 图片加载通道分发：常见格式 → asset（前端直读）；RAW → 解码 JPEG；动画 → 帧序列
/// 命中缓存直接返回（≤50ms 目标）；spawn_blocking 避免 CPU 密集解码占用 runtime 线程
/// 缓存关闭（enabled=false）时跳过缓存读写（不缓存）
#[tauri::command]
pub async fn load_image(
    path: String,
    cache: State<'_, DecodeCache>,
    settings: State<'_, SettingsState>,
) -> Result<crate::decode::LoadResult, String> {
    let caching = settings.0.lock().map_err(|e| e.to_string())?.is_enabled();
    if caching {
        if let Some(hit) = cache.get(&path) {
            return Ok((*hit).clone());
        }
    }
    // 预取可能正在解码同一路径 —— 不等待：预取是低优先级后台优化，
    // 实时加载（翻页/幻灯片）永远优先，直接自己解码（代价是重复解码一次，
    // 但绝不让实时路径被预取拖住或排队死锁）。
    let path2 = path.clone();
    let result = tauri::async_runtime::spawn_blocking(move || crate::decode::load_image(&path2).map(Arc::new))
        .await
        .map_err(|e| format!("解码任务失败: {e}"))??;
    if caching {
        // asset 空结果（单帧 gif/png/webp）不占缓存槽位
        if result.mode != "asset" {
            cache.put(path.clone(), Arc::clone(&result));
        }
        // 实时解码完成：清除该路径的预取登记（预取任务 put 前 peek 会跳过）
        cache.end_prefetch(&path);
    }
    Ok((*result).clone())
}

/// 设置缓存等级：0=关闭 1=开启（前后各1） 2=高（前1后3）
/// 关闭清空双队列；开启/高等级恢复容量
#[tauri::command]
pub fn set_cache_level(
    cache: State<'_, DecodeCache>,
    first_cache: State<'_, FolderFirstCache>,
    settings: State<'_, SettingsState>,
    level: usize,
) -> Result<AppSettings, String> {
    let level = level.min(2);
    let mut g = settings.0.lock().map_err(|e| e.to_string())?;
    g.cache_level = level;
    if level == 0 {
        // 关闭时清空队列
        cache.set_capacity(0);
        first_cache.set_capacity(0);
    } else {
        // 开启/高等级：容量覆盖窗口（高等级前1后3=4张 + 当前 + 余量）
        cache.set_capacity(if level >= 2 { 8 } else { 4 });
        first_cache.set_capacity(4);
    }
    Ok(*g)
}

/// 查询当前设置
#[tauri::command]
pub fn get_settings(settings: State<'_, SettingsState>) -> AppSettings {
    *settings.0.lock().unwrap_or_else(|e| e.into_inner())
}

/// 查询当前图片上下文路径（按缓存强度），供前端预解码池（方案四）预热
/// P3-1：相邻文件夹首图的目录枚举移入 spawn_blocking，不在持有 AppState 锁时阻塞命令线程
#[tauri::command]
pub async fn get_context(
    state: State<'_, AppState>,
    settings: State<'_, SettingsState>,
) -> Result<Vec<String>, String> {
    // 锁内仅收集路径（快速）；目录枚举在释放锁后由后台线程完成
    let (mut paths, neighbors) = {
        let guard = state.0.lock().map_err(|e| e.to_string())?;
        let Some(model) = guard.as_ref() else {
            return Ok(Vec::new());
        };
        // 单次锁读取设置（预取窗口 + 首图队列深度）
        let (prev_n, next_n, folder_depth, enabled) = {
            let s = settings.0.lock().map_err(|e| e.to_string())?;
            let (p, n) = s.neighbor_window();
            (p, n, s.folder_first_depth, s.is_enabled())
        };
        let paths: Vec<String> = model
            .context_paths(prev_n, next_n)
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        // P3-2：缓存关闭（cache_level=0）时不返回任何首图路径，
        // 前端不得继续预热（与「0=关闭不缓存不预取」语义一致）
        let neighbors = if folder_depth > 0 && enabled {
            model.neighbor_folders(1)
        } else {
            Vec::new()
        };
        (paths, neighbors)
    };
    // 锁外：后台枚举相邻文件夹首图（asset 供前端池预热，非 asset 由 Rust 队列 A 处理）
    if !neighbors.is_empty() {
        let firsts = tauri::async_runtime::spawn_blocking(move || {
            let mut out = Vec::with_capacity(neighbors.len());
            for folder in neighbors {
                if let Some(first) = BrowseModel::first_image_of(&folder) {
                    out.push(first.to_string_lossy().to_string());
                }
            }
            out
        })
        .await
        .map_err(|e| format!("首图枚举任务失败: {e}"))?;
        for p in firsts {
            if !paths.contains(&p) {
                paths.push(p);
            }
        }
    }
    Ok(paths)
}
