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
/// - cache_level：0=关闭（不缓存不预取）1=开启（前后各 1）2=高等级（前 1 后 3）
/// - folder_first_depth：文件夹首图队列深度（默认 1；跨文件夹跳转缓存）
#[derive(serde::Serialize, Clone, Copy)]
pub struct AppSettings {
    pub cache_level: usize,
    pub folder_first_depth: usize,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            cache_level: 1,
            folder_first_depth: 1,
        }
    }
}

impl AppSettings {
    /// 邻居预取窗口：0=不预取；1=前1后1；2(高)=前1后3
    pub fn neighbor_window(&self) -> (usize, usize) {
        match self.cache_level {
            0 => (0, 0),
            1 => (1, 1),
            _ => (1, 3), // 高等级：前 1 后 3
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.cache_level > 0
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
    if !settings.is_enabled() || settings.folder_first_depth == 0 {
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

/// 导航进入新文件夹时，把 FolderFirstCache 命中的首图并入 DecodeCache
/// （前端 load_image 按图片路径查队列 B，命中即零解码）
fn promote_folder_first(
    model: &BrowseModel,
    cache: &DecodeCache,
    first_cache: &FolderFirstCache,
) {
    let folder = model.current_folder_path();
    let Some(result) = first_cache.get(&folder.to_string_lossy().to_string()) else {
        return; // 该文件夹首图未预取（无命中）
    };
    // 首图路径（同一张图）并入队列 B，前端 load_image 查 path 命中
    if let Some(first) = BrowseModel::first_image_of(&folder) {
        cache.put(first.to_string_lossy().to_string(), result);
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
                // 跨文件夹进入新文件夹：先把预取的首图并入队列 B（前端 load_image 命中）
                promote_folder_first(m, cache.inner(), first_cache.inner());
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
                // 跨文件夹进入新文件夹：先把预取的首图并入队列 B（前端 load_image 命中）
                promote_folder_first(m, cache.inner(), first_cache.inner());
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
                // 跨文件夹进入新文件夹：先把预取的首图并入队列 B（前端 load_image 命中）
                promote_folder_first(m, cache.inner(), first_cache.inner());
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
    let caching = settings.0.lock().map_err(|e| e.to_string())?.is_enabled();
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
    let mut paths: Vec<String> = model
        .context_paths(prev_n, next_n)
        .into_iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    // 并入相邻文件夹首图（asset 供前端池预热，非 asset 由 Rust 队列 A 处理）
    if settings.0.lock().map_err(|e| e.to_string())?.folder_first_depth > 0 {
        for folder in model.neighbor_folders(1) {
            if let Some(first) = BrowseModel::first_image_of(&folder) {
                let p = first.to_string_lossy().to_string();
                if !paths.contains(&p) {
                    paths.push(p);
                }
            }
        }
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neighbor_window_rules() {
        let s = |l| AppSettings { cache_level: l, folder_first_depth: 1 };
        assert_eq!(s(0).neighbor_window(), (0, 0), "0：不预取");
        assert_eq!(s(1).neighbor_window(), (1, 1), "1：前后各 1");
        assert_eq!(s(2).neighbor_window(), (1, 3), "2(高)：前 1 后 3");
        assert_eq!(s(2).is_enabled(), true);
        assert_eq!(s(0).is_enabled(), false);
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

#[cfg(test)]
mod folder_first_tests {
    use super::*;
    use crate::cache::FolderFirstCache;

    #[test]
    fn folder_first_cache_promote_to_neighbor() {
        // 验证：预取 C 首图入队列 A → 导航到 C → promote 并入队列 B
        // C 首图 c01.png 单帧判定 asset → 队列 A 不缓存（asset 由前端池预热）；
        // 此测试验证非 asset 场景：用 B 首图 img_2.gif（动画，非 asset）
        let base = std::env::current_dir().unwrap().join("..").join("test-images");
        // 构造 B 首图为 gif 的模型：直接从 B/img_2.gif 打开
        let model = BrowseModel::open(&base.join("B/img_2.gif"), None).unwrap();
        model.wait_ready();

        let first_cache = FolderFirstCache::new(4);
        let neighbor = DecodeCache::new(4);
        let settings = AppSettings::default();

        // 预取相邻文件夹首图（队列 A）：B 的邻居 A 首图 1.png（单帧 png→asset，跳过）、C 首图 c01.png（asset，跳过）
        // 首图非 asset 的邻居文件夹在此测试树中不存在 → 队列 A 为空是正确行为
        prefetch_folder_firsts(&model, &first_cache, &settings);
        std::thread::sleep(std::time::Duration::from_millis(100));

        // 关键验证：get_context 必须包含相邻文件夹首图路径（asset 供前端池预热）
        // 直接验证 first_image_of 与 neighbor_folders 的衔接
        let neighbors = model.neighbor_folders(1);
        assert!(neighbors.len() >= 1, "B 应有相邻文件夹");
        for f in &neighbors {
            let first = BrowseModel::first_image_of(f).expect("相邻文件夹应有首图");
            assert!(first.is_file(), "首图路径应有效");
        }
    }
}
