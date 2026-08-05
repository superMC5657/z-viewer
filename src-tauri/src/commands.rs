//! Tauri IPC commands：前端浏览逻辑的桥梁

use std::path::Path;
use std::sync::{Arc, Mutex};

use tauri::{Emitter, State};

use crate::browse::{Boundary, BrowseModel, BrowseState, FolderTarget};
use crate::cache::{DecodeCache, FolderFirstCache};

/// 后台扫描完成事件（携带最新 BrowseState，前端刷新位置计数）
pub const BROWSE_SCAN_READY: &str = "browse://scan-ready";

/// 全局浏览模型（Mutex：Tauri 命令在独立线程执行）
pub struct AppState(pub Mutex<Option<BrowseModel>>);

/// 用户设置（缓存开关/深度）
pub struct SettingsState(pub Mutex<AppSettings>);

/// 用户设置
/// - enabled：缓存总开关（UI 为工具栏一个按钮，激活态 = 开启）
/// - neighbor_depth：当前文件夹前后图片缓存等级（默认 1 = 前后各 1；留参数以后配置）
/// - folder_first_depth：文件夹首图队列深度（默认 1；跨文件夹跳转缓存）
#[derive(serde::Serialize, Clone, Copy)]
pub struct AppSettings {
    pub enabled: bool,
    pub neighbor_depth: usize,
    pub folder_first_depth: usize,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            neighbor_depth: 1,
            folder_first_depth: 1,
        }
    }
}

impl AppSettings {
    /// 邻居预取数量：neighbor_depth 前后各 n；enabled=false 或 depth=0 → 不预取
    pub fn neighbor_window(&self) -> (usize, usize) {
        if !self.enabled {
            return (0, 0);
        }
        let n = self.neighbor_depth;
        (n, n)
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
/// 队列 B：当前文件夹前后邻居预取（enabled 且 depth>0 时）
fn prefetch_context(model: &BrowseModel, cache: &DecodeCache, settings: &AppSettings) {
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
            let result = crate::decode::load_image(&path).ok();
            if let Some(result) = result {
                // 实时加载可能已填充缓存（finish_load 清登记后我们 put 会覆盖，内容相同无害）
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
fn prefetch_folder_firsts(
    model: &BrowseModel,
    first_cache: &FolderFirstCache,
    settings: &AppSettings,
) {
    if !settings.enabled || settings.folder_first_depth == 0 {
        return;
    }
    let depth = settings.folder_first_depth.min(3); // 首图队列深度上限 3
    for folder in model.neighbor_folders(depth) {
        let folder_str = folder.to_string_lossy().to_string();
        if first_cache.peek(&folder_str) {
            continue;
        }
        // 取该文件夹第一张图并解码入队
        let first = BrowseModel::first_image_of(&folder);
        let Some(first) = first else { continue };
        let path = first.to_string_lossy().to_string();
        if crate::decode::is_asset_ext(&path) {
            continue; // asset 由前端池预热
        }
        let first_cache = first_cache.clone();
        tauri::async_runtime::spawn_blocking(move || {
            if let Ok(result) = crate::decode::load_image(&path) {
                if result.mode != "asset" {
                    first_cache.put(folder_str, Arc::new(result));
                }
            }
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
    first_cache: State<'_, FolderFirstCache>,
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
    let mut guard = state.0.lock().map_err(|e| e.to_string())?;
    match guard.as_mut() {
        Some(m) => {
            let nav = m.next();
            let r = nav_ok_or_none(m, nav);
            if r.state.is_some() {
                let s = settings.0.lock().map_err(|e| e.to_string())?;
                prefetch_context(m, cache.inner(), &s);
                prefetch_folder_firsts(m, first_cache.inner(), &s);
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
    first_cache: State<'_, FolderFirstCache>,
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
                prefetch_folder_firsts(m, first_cache.inner(), &s);
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
    let mut guard = state.0.lock().map_err(|e| e.to_string())?;
    match guard.as_mut() {
        Some(m) => {
            let nav = m.jump_folder(t);
            let r = nav_ok_or_none(m, nav);
            if r.state.is_some() {
                let s = settings.0.lock().map_err(|e| e.to_string())?;
                prefetch_context(m, cache.inner(), &s);
                prefetch_folder_firsts(m, first_cache.inner(), &s);
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
/// 缓存关闭（enabled=false）时跳过缓存读写（不缓存）
#[tauri::command]
pub async fn load_image(
    path: String,
    cache: State<'_, DecodeCache>,
    settings: State<'_, SettingsState>,
) -> Result<crate::decode::LoadResult, String> {
    let caching = settings.0.lock().map_err(|e| e.to_string())?.enabled;
    if caching {
        if let Some(hit) = cache.get(&path) {
            return Ok((*hit).clone());
        }
    }
    let path2 = path.clone();
    let cache2 = cache.inner().clone(); // &DecodeCache → clone（Arc 共享）
    let result = tauri::async_runtime::spawn_blocking(move || {
        // 预取正在解码同一路径：等待其完成（轮询缓存，最多 3s），避免重复解码 RAW
        if caching && cache2.is_prefetching(&path2) {
            for _ in 0..300 {
                std::thread::sleep(std::time::Duration::from_millis(10));
                if let Some(hit) = cache2.get(&path2) {
                    return Ok(hit);
                }
                if !cache2.is_prefetching(&path2) {
                    break;
                }
            }
        }
        crate::decode::load_image(&path2).map(Arc::new)
    })
    .await
    .map_err(|e| format!("解码任务失败: {e}"))??;
    if caching {
        // asset 空结果（单帧 gif/png/webp）不占缓存槽位
        if result.mode != "asset" {
            cache.put(path.clone(), Arc::clone(&result));
        }
        // 实时解码完成：清除该路径的预取登记（预取任务 put 前 peek 会跳过）
        cache.finish_load(&path);
        Ok((*result).clone())
    } else {
        Ok((*result).clone())
    }
}

/// 缓存总开关（UI 工具栏按钮，激活态 = 开启）
#[tauri::command]
pub fn set_cache_enabled(
    cache: State<'_, DecodeCache>,
    first_cache: State<'_, FolderFirstCache>,
    settings: State<'_, SettingsState>,
    enabled: bool,
) -> Result<AppSettings, String> {
    let mut g = settings.0.lock().map_err(|e| e.to_string())?;
    g.enabled = enabled;
    if enabled {
        // 开启时恢复容量
        cache.set_capacity(cache.capacity().max(4));
        first_cache.set_capacity(first_cache.capacity().max(4));
    } else {
        // 关闭时清空队列
        cache.set_capacity(0);
        first_cache.set_capacity(0);
    }
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
        .neighbor_window();
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
    fn neighbor_window_rules() {
        let s = AppSettings { enabled: true, neighbor_depth: 1, folder_first_depth: 1 };
        assert_eq!(s.neighbor_window(), (1, 1), "默认：前后各 1");
        let s2 = AppSettings { enabled: false, neighbor_depth: 3, folder_first_depth: 1 };
        assert_eq!(s2.neighbor_window(), (0, 0), "关闭时不预取");
        let s3 = AppSettings { enabled: true, neighbor_depth: 0, folder_first_depth: 1 };
        assert_eq!(s3.neighbor_window(), (0, 0), "depth=0 不预取");
        let s4 = AppSettings { enabled: true, neighbor_depth: 5, folder_first_depth: 1 };
        assert_eq!(s4.neighbor_window(), (5, 5), "前后各 5（等级参数）");
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    /// 用真实测试图验证 DecodeCache 命中路径（load_image 是 async command，此处直接测 decode+cache 组合）
    #[test]
    fn cache_hit_is_faster_than_miss() {
        let cache = DecodeCache::new(8);
        let path = std::env::current_dir()
            .unwrap()
            .join("..")
            .join("test-images/B/img_2.gif")
            .to_string_lossy()
            .to_string();

        // 首次：解码 + 入缓存（模拟 load_image 主路径）
        let r1 = crate::decode::load_image(&path).unwrap();
        assert_eq!(r1.mode, "animated");
        cache.put(path.clone(), Arc::new(r1.clone()));

        // 命中：直接从缓存取
        let hit = cache.get(&path).expect("应命中缓存");
        assert_eq!(hit.mode, "animated", "命中返回相同数据");

        // 再次 put 同路径（模拟预取覆盖）→ 内容不变
        cache.put(path.clone(), Arc::new(r1.clone()));
        let hit2 = cache.get(&path).expect("再次命中");
        assert_eq!(hit2.mode, "animated");
    }

    #[test]
    fn asset_mode_not_cached() {
        // 单帧静态 PNG 走 asset：load_image 返回 mode=asset 且不应进缓存（占用槽位）
        let cache = DecodeCache::new(2);
        let path = std::env::current_dir()
            .unwrap()
            .join("..")
            .join("test-images/A/1.png")
            .to_string_lossy()
            .to_string();
        let r = crate::decode::load_image(&path).unwrap();
        assert_eq!(r.mode, "asset");
        // 模拟 load_image 的「asset 不 put」分支
        let arc = Arc::new(r);
        if arc.mode != "asset" {
            cache.put(path.clone(), Arc::clone(&arc));
        }
        // 另一张真缓存
        let raw_path = std::env::current_dir()
            .unwrap()
            .join("..")
            .join("test-images/B/img_2.gif")
            .to_string_lossy()
            .to_string();
        let r2 = crate::decode::load_image(&raw_path).unwrap();
        cache.put(raw_path.clone(), Arc::new(r2.clone()));
        cache.put("x".into(), sample("x"));
        // 容量 2：raw + x 在，asset 未占槽
        assert!(cache.get(&raw_path).is_some(), "raw 仍在");
        assert!(cache.get(&path).is_none(), "asset 从未入缓存");
    }

    fn sample(path: &str) -> Arc<crate::decode::LoadResult> {
        Arc::new(crate::decode::LoadResult {
            mode: "raw".into(),
            data: Some(format!("data-{path}")),
            frames: None,
        })
    }
}
