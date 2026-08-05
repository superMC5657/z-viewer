//! Tauri IPC commands：前端浏览逻辑的桥梁

use std::path::Path;
use std::sync::Mutex;

use tauri::State;

use crate::browse::{Boundary, BrowseModel, BrowseState, FolderTarget};

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

/// 打开指定路径（拖拽/命令行）：文件定位到浏览模型，目录取其首张图片
#[tauri::command]
pub fn open_path(state: State<'_, AppState>, path: String) -> Result<NavResult, String> {
    let p = Path::new(&path);
    let model = if p.is_dir() {
        BrowseModel::open_first_in_dir(p)
    } else {
        BrowseModel::open(p)
    };
    let model = model.ok_or_else(|| "无法打开：不是支持的图片格式".to_string())?;
    let st = model.state();
    *state.0.lock().map_err(|e| e.to_string())? = Some(model);
    Ok(NavResult::ok(st))
}

#[tauri::command]
pub fn next_image(state: State<'_, AppState>) -> Result<NavResult, String> {
    let mut guard = state.0.lock().map_err(|e| e.to_string())?;
    match guard.as_mut() {
        Some(m) => {
            let nav = m.next();
            Ok(nav_ok_or_none(m, nav))
        }
        None => Ok(NavResult::none()),
    }
}

#[tauri::command]
pub fn prev_image(state: State<'_, AppState>) -> Result<NavResult, String> {
    let mut guard = state.0.lock().map_err(|e| e.to_string())?;
    match guard.as_mut() {
        Some(m) => {
            let nav = m.prev();
            Ok(nav_ok_or_none(m, nav))
        }
        None => Ok(NavResult::none()),
    }
}

/// 文件夹级跳转：target ∈ "first" | "prev" | "next" | "last"
#[tauri::command]
pub fn jump_folder(state: State<'_, AppState>, target: String) -> Result<NavResult, String> {
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
            Ok(nav_ok_or_none(m, nav))
        }
        None => Ok(NavResult::none()),
    }
}

/// 启动时查询初始状态（命令行参数已由 main.rs 注入模型）
#[tauri::command]
pub fn get_initial_state(state: State<'_, AppState>) -> Option<BrowseState> {
    state.0.lock().ok()?.as_ref().map(|m| m.state())
}
