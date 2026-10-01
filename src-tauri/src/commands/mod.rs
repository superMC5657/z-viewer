//! Tauri IPC 命令：前端浏览逻辑的桥梁
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
use prefetch::{prefetch_context, prefetch_folder_firsts, promote_folder_first};

/// 后台扫描完成事件（携带最新 BrowseState，前端刷新位置计数）
pub const BROWSE_SCAN_READY: &str = "browse://scan-ready";

/// 全局浏览模型（Mutex：Tauri 命令在独立线程执行）。
/// 存 Arc：命令先快照出模型再释放本锁 —— 导航可能在 Condvar 上等待慢盘文件夹
/// 填充，若持本锁等待会阻塞 load_image/get_context 等所有命令（模型内部自有锁
/// 串行化并发导航，前端 showSeq 兜底乱序响应）。
pub struct AppState(pub Mutex<Option<Arc<BrowseModel>>>);

/// 用户设置（缓存开关/深度）
pub struct SettingsState(pub Mutex<AppSettings>);

impl AppState {
    /// 快照浏览模型：持锁克隆 Arc 后立即释放（Top4 锁收口）。
    /// 导航/上下文读取可能在模型内部 Condvar 上等待慢盘填充，绝不能持本锁等待。
    /// 锁中毒时返回 Err（与原 map_err 透传语义一致，前端契约不变）。
    pub fn snapshot(&self) -> Result<Option<Arc<BrowseModel>>, String> {
        self.0
            .lock()
            .map(|g| g.as_ref().cloned())
            .map_err(|e| e.to_string())
    }

    /// 有模型时执行 f，无模型时返回 default()（未打开/空模型契约保持不变）。
    /// navigate / get_initial_state / get_context 共用，消除重复的 lock 模板。
    pub fn with_model<T>(
        &self,
        default: impl FnOnce() -> T,
        f: impl FnOnce(&BrowseModel) -> T,
    ) -> Result<T, String> {
        match self.snapshot()? {
            Some(m) => Ok(f(&m)),
            None => Ok(default()),
        }
    }

    /// 替换全局模型（open_path 共用；启动注入走 manage 前直接构造不经此）。
    pub fn replace(&self, model: BrowseModel) -> Result<(), String> {
        *self.0.lock().map_err(|e| e.to_string())? = Some(Arc::new(model));
        Ok(())
    }
}

impl SettingsState {
    /// 只读快照：单次加锁，闭包内读取所需字段后释放（navigate / load_image 共用）。
    pub fn read<T>(&self, f: impl FnOnce(&AppSettings) -> T) -> Result<T, String> {
        self.0.lock().map(|g| f(&g)).map_err(|e| e.to_string())
    }

    /// 可写更新：闭包内修改设置后返回快照（set_cache_level 共用）。
    pub fn update<T>(&self, f: impl FnOnce(&mut AppSettings) -> T) -> Result<T, String> {
        self.0.lock().map(|mut g| f(&mut g)).map_err(|e| e.to_string())
    }

    /// 快照缓存等级：analytics 上报等只读方复用；锁中毒回退 0（关闭）。
    pub fn cache_level_or_closed(&self) -> usize {
        self.0.lock().ok().map(|g| g.cache_level).unwrap_or(0)
    }
}

/// 专业版门控唯一入口（Top4 收口）：pro 则执行 pro()，否则返回 free。
/// 各命令的 free 契约保持不变：
/// - jump_folder → Ok(pro-required)、get_context → Ok(空)
/// - set_cache_level → Err(解锁提示)、get_settings/load_image → 降级值
pub(crate) fn require_pro<T>(
    license: &crate::license::LicenseManager,
    free: T,
    pro: impl FnOnce() -> T,
) -> T {
    if license.is_pro() {
        pro()
    } else {
        free
    }
}

/// 导航结果：boundary 为 Some 时表示撞到全局边界（前端弹 Toast），state 不变
/// boundary 取值："first-image" | "last-image" | "first-folder" | "last-folder" | "pro-required"
#[derive(serde::Serialize)]
pub struct NavResult {
    pub boundary: Option<String>,
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
    /// 专业版功能锁定（免费版调用 jump_folder）：前端弹出解锁引导
    fn pro_required() -> Self {
        Self {
            boundary: Some("pro-required".into()),
            state: None,
        }
    }
    fn none() -> Self {
        Self {
            boundary: None,
            state: None,
        }
    }
}

fn nav_ok_or_none(model: &BrowseModel, nav: Nav) -> NavResult {
    match nav {
        Nav::Ok => NavResult::ok(model.state()),
        Nav::Boundary(b) => NavResult::boundary(b, model.state()),
    }
}

use crate::util::{basename_of, first_line_of};

/// 构造后台扫描完成回调：emit BROWSE_SCAN_READY 供前端刷新位置计数
/// （main.rs 启动注入与 open_path 共用同一实现）
pub fn on_ready_callback(app: tauri::AppHandle) -> crate::browse::OnReady {
    Box::new(move |st: BrowseState| {
        let _ = app.emit(BROWSE_SCAN_READY, st);
    })
}

/// 按路径形态分发打开浏览模型：目录 → open_first_in_dir_gated（取首图），
/// 文件 → open_gated（定位到该文件）；main.rs 启动注入与 open_path 共用。
/// 只做分发：日志、advance_generation、prefetch、缓存代次保持在调用点各自处理。
pub fn open_model_from_path(
    p: &Path,
    on_ready: Option<crate::browse::OnReady>,
    pro: bool,
) -> Option<BrowseModel> {
    if p.is_dir() {
        BrowseModel::open_first_in_dir_gated(p, on_ready, pro)
    } else {
        BrowseModel::open_gated(p, on_ready, pro)
    }
}

/// 执行导航并处理后置动作（首图 promote + 邻居预取）；三个导航命令共用。
/// 先快照模型（Arc clone）再释放 AppState 锁：导航的等待填充只占模型内部锁
fn navigate(
    state: &AppState,
    cache: &DecodeCache,
    first_cache: &FolderFirstCache,
    settings: &SettingsState,
    nav: impl FnOnce(&BrowseModel) -> Nav,
) -> Result<NavResult, String> {
    let Some(model) = state.snapshot()? else {
        return Ok(NavResult::none());
    };
    let prev_folder = model.state().folder_name;
    let nav = nav(&model);
    let r = nav_ok_or_none(&model, nav);
    if let Some(b) = r.boundary.as_deref() {
        // 导航边界：高频且用户可见 toast，只 debug（release 不可见）
        log::debug!("browse boundary {b}");
    }
    if let Some(st) = r.state.as_ref() {
        if st.folder_name != prev_folder {
            log::info!("browse nav crossed folder to {}", st.folder_name);
        }
        // 单图切换：高频交互，只 debug（dev 模式可见，release 不可见防刷盘）
        log::debug!(
            "browse nav {}/{} {}",
            st.global_index + 1,
            st.global_total,
            st.file_name
        );
        cache.advance_generation();
        // 跨文件夹进入新文件夹：先把预取的首图并入队列 B（前端 load_image 命中）
        promote_folder_first(&model, cache, first_cache);
        settings.read(|s| {
            prefetch_context(&model, cache, s);
            prefetch_folder_firsts(&model, cache, first_cache, s);
        })?;
    }
    Ok(r)
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
    license: State<'_, crate::license::LicenseManager>,
    path: String,
) -> Result<NavResult, String> {
    let p = Path::new(&path);
    let pro = require_pro(&license, false, || true);
    let on_ready = Some(on_ready_callback(app.clone()));
    let model = open_model_from_path(p, on_ready, pro);
    let Some(model) = model else {
        // 浏览模型打开失败：error，只记 basename
        log::error!("browse open failed {}", basename_of(&path));
        return Err("无法打开：不是支持的图片格式".to_string());
    };
    let st = model.state();
    log::info!(
        "browse open ok {} (folder: {}, total: {})",
        basename_of(&path),
        st.folder_name,
        st.global_total
    );
    cache.advance_generation();
    settings.read(|s| {
        prefetch_context(&model, cache.inner(), s);
        prefetch_folder_firsts(&model, cache.inner(), first_cache.inner(), s);
    })?;
    state.replace(model)?;
    Ok(NavResult::ok(st))
}

#[tauri::command]
pub fn next_image(
    state: State<'_, AppState>,
    cache: State<'_, DecodeCache>,
    first_cache: State<'_, FolderFirstCache>,
    settings: State<'_, SettingsState>,
) -> Result<NavResult, String> {
    let r = navigate(
        state.inner(),
        cache.inner(),
        first_cache.inner(),
        settings.inner(),
        |m| m.next(),
    );
    r
}

#[tauri::command]
pub fn prev_image(
    state: State<'_, AppState>,
    cache: State<'_, DecodeCache>,
    first_cache: State<'_, FolderFirstCache>,
    settings: State<'_, SettingsState>,
) -> Result<NavResult, String> {
    navigate(
        state.inner(),
        cache.inner(),
        first_cache.inner(),
        settings.inner(),
        |m| m.prev(),
    )
}

/// 文件夹级跳转：target ∈ "first" | "prev" | "next" | "last"
/// 专业版功能：免费版返回 pro-required（前端弹解锁引导）
#[tauri::command]
pub fn jump_folder(
    state: State<'_, AppState>,
    cache: State<'_, DecodeCache>,
    first_cache: State<'_, FolderFirstCache>,
    settings: State<'_, SettingsState>,
    license: State<'_, crate::license::LicenseManager>,
    target: String,
) -> Result<NavResult, String> {
    require_pro(&license, Ok(NavResult::pro_required()), || {
        let t = match target.as_str() {
            "first" => FolderTarget::First,
            "prev" => FolderTarget::Prev,
            "next" => FolderTarget::Next,
            "last" => FolderTarget::Last,
            _ => {
                log::warn!("browse jump failed: unknown target");
                return Err(format!("未知跳转目标: {target}"));
            }
        };
        let r = navigate(
            state.inner(),
            cache.inner(),
            first_cache.inner(),
            settings.inner(),
            |m| m.jump_folder(t),
        )?;
        if let Some(st) = r.state.as_ref() {
            // 文件夹级跳转成功：低频关键定位变更，记 info
            log::info!("browse jump folder {target} ok {}", st.folder_name);
        }
        Ok(r)
    })
}

/// 启动时查询初始状态（命令行参数已由 main.rs 注入模型）
/// 快照 Arc 后释放锁再读状态（state 可能等待后台填充，不能占着 AppState 锁等）
#[tauri::command]
pub fn get_initial_state(state: State<'_, AppState>) -> Option<BrowseState> {
    state
        .with_model(|| None, |m| Some(m.state()))
        .ok()
        .flatten()
}

/// 图片加载通道分发：常见格式 → asset（前端直读）；RAW/静态 → JPEG 字节；
/// 动画 → 帧序列（按需拆帧）。命中缓存直接返回（≤50ms 目标）；
/// spawn_blocking 避免 CPU 密集解码占用 runtime 线程。
/// 缓存关闭（enabled=false / 免费版）时跳过缓存读写（不缓存）。
/// 返回值是二进制信封（pack_envelope）：前端 invoke 收到 ArrayBuffer 解析。
/// `full=false`：RAW 优先返回内嵌预览（毫秒级）；预览**不写缓存**，仅全量写缓存。
#[tauri::command]
pub async fn load_image(
    path: String,
    full: bool,
    cache: State<'_, DecodeCache>,
    settings: State<'_, SettingsState>,
    license: State<'_, crate::license::LicenseManager>,
) -> Result<tauri::ipc::Response, String> {
    crate::z_log::set_last_viewed(&path);
    let enabled = settings.read(|s| s.is_enabled())?;
    let caching = enabled && require_pro(&license, false, || true);
    if caching {
        // 命中：信封已打包一次缓存于条目内，这里只做一次缓冲拷贝进 IPC 响应
        if let Some(hit) = cache.get_entry(&path) {
            let envelope = hit.envelope();
            return Ok(tauri::ipc::Response::new((*envelope).clone()));
        }
    }
    // 预取可能正在解码同一路径 —— 不等待：预取是低优先级后台优化，
    // 实时加载（翻页/幻灯片）永远优先，直接自己解码（代价是重复解码一次，
    // 但绝不让实时路径被预取拖住或排队死锁）。
    let path2 = path.clone();
    let base = basename_of(&path);
    let result = tauri::async_runtime::spawn_blocking(move || {
        crate::decode::load_image(&path2, full).map(Arc::new)
    })
    .await
    .map_err(|e| {
        let msg = format!("解码任务失败: {e}");
        log::warn!("decode failed {base}: {}", first_line_of(&msg));
        msg
    })?;
    let result = match result {
        Ok(r) => r,
        Err(e) => {
            // 解码失败：warn，只记 basename + 原因首行，不记全路径
            log::warn!("decode failed {base}: {}", first_line_of(&e));
            return Err(e);
        }
    };
    if caching {
        // asset 空结果（单帧 gif/png/webp）与内嵌预览不占缓存槽位
        if result.mode != "asset" && !result.is_preview {
            cache.put(path.clone(), Arc::clone(&result));
        }
        // 实时解码完成：清除该路径的预取登记（预取任务 put 前 peek 会跳过）
        cache.end_prefetch(&path);
    }
    Ok(tauri::ipc::Response::new(crate::decode::pack_envelope(
        &result,
    )))
}

/// 轻量判定文件是否多帧动画（不拆帧）：WebP 扫 RIFF ANMF 块、PNG 扫 acTL 块、
/// GIF 看魔数即视为动画。前端在 asset 通道显示后据此决定是否亮出帧控制浮条。
#[tauri::command]
pub fn check_animation(path: String) -> bool {
    crate::decode::is_animated(&path)
}

/// 会话统计计数点（埋点）：前端在图片**显示成功**后调用一次（唯一的"用户看到"事件）。
/// asset 快速通道（jpg/bmp 等浏览器原生解码，不经 load_image）与 IPC 通道
/// （RAW/动画）都在 `showImage` 显示成功后各调一次；解码失败/被抢占的显示不调。
/// 仅聚合统计路径，不上报路径本身（analytics.rs 只输出数量与格式分布）。
#[tauri::command]
pub fn record_view(path: String, stats: State<'_, crate::analytics::SessionStats>) {
    stats.record(&path);
}

/// 设置缓存等级：0=关闭 1=开启（前后各1） 2=高（前1后3）
/// 关闭清空双队列；开启/高等级恢复容量
/// 专业版功能：免费版拒绝（前端引导解锁）
#[tauri::command]
pub fn set_cache_level(
    cache: State<'_, DecodeCache>,
    first_cache: State<'_, FolderFirstCache>,
    settings: State<'_, SettingsState>,
    license: State<'_, crate::license::LicenseManager>,
    level: usize,
) -> Result<AppSettings, String> {
    require_pro(
        &license,
        Err("预取缓存是专业版功能，请解锁后使用".to_string()),
        || {
            let level = level.min(2);
            settings.update(|g| {
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
                // 缓存等级变更：核心配置变迁，记 info
                log::info!("cache level set {level}");
                *g
            })
        },
    )
}

/// 查询当前设置（免费版强制返回 cache_level=0，防前端本地存储恢复付费等级）
#[tauri::command]
pub fn get_settings(
    settings: State<'_, SettingsState>,
    license: State<'_, crate::license::LicenseManager>,
) -> AppSettings {
    let mut s = *settings.0.lock().unwrap_or_else(|e| e.into_inner());
    let level = s.cache_level;
    s.cache_level = require_pro(&license, 0, || level);
    s
}

/// RAW 扩展名列表（前端 needsIpc 的唯一真源：启动时拉取替换前端内置兜底集，
/// 避免后端 RAW_EXTS 与前端 IPC_EXTS 两份清单人肉同步 —— 漏同步的新格式会被
/// 前端误判为 asset 通道而无法显示）
#[tauri::command]
pub fn get_raw_extensions() -> Vec<&'static str> {
    crate::decode::RAW_EXTS.to_vec()
}

/// 查询当前图片上下文路径（按缓存强度），供前端预解码池预热
/// 相邻文件夹首图的目录枚举移入 spawn_blocking，不在持有 AppState 锁时阻塞命令线程
/// 专业版功能：免费版返回空（无预取）
#[tauri::command]
pub async fn get_context(
    state: State<'_, AppState>,
    settings: State<'_, SettingsState>,
    license: State<'_, crate::license::LicenseManager>,
) -> Result<Vec<String>, String> {
    if !require_pro(&license, false, || true) {
        return Ok(Vec::new());
    }
    // 快照模型后释放 AppState 锁（context_paths/neighbor_folders 走模型内部锁，均快速）
    let Some(model) = state.snapshot()? else {
        return Ok(Vec::new());
    };
    // 单次锁读取设置（预取窗口 + 首图队列深度）
    let (prev_n, next_n, folder_depth, enabled) = settings.read(|s| {
        let (p, n) = s.neighbor_window();
        (p, n, s.folder_first_depth, s.is_enabled())
    })?;
    let mut paths: Vec<String> = model
        .context_paths(prev_n, next_n)
        .into_iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    // 缓存关闭（cache_level=0）时不返回任何首图路径，前端不预热
    let neighbors = if folder_depth > 0 && enabled {
        model.neighbor_folders(1)
    } else {
        Vec::new()
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
