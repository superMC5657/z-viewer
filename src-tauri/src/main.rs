// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod browse;
mod cache;
mod commands;
mod decode;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use browse::BrowseModel;
use cache::DecodeCache;
use commands::AppState;
use tauri::Manager;

/// 解析命令行传入的路径：相对路径优先按项目根解析
/// （`pnpm tauri dev -- <path>` 时应用 cwd 为 src-tauri/，相对路径会失效）
fn resolve_path_arg(arg: &str) -> PathBuf {
    let p = Path::new(arg);
    if p.exists() || p.is_absolute() {
        return p.to_path_buf();
    }
    // exe 位于 <项目根>/src-tauri/target/<profile>/，上溯 4 级到项目根
    if let Some(exe) = std::env::current_exe().ok() {
        let root = exe
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .and_then(|p| p.parent());
        if let Some(root) = root {
            let cand = root.join(p);
            if cand.exists() {
                return cand;
            }
        }
    }
    p.to_path_buf()
}

/// 从命令行参数中找出第一个文件/目录路径（跳过以 `-` 开头的选项与可执行文件自身）
fn startup_image_path() -> Option<String> {
    let mut args = std::env::args();
    args.next(); // 可执行文件自身
    for arg in args {
        if arg.starts_with('-') {
            continue;
        }
        let resolved = resolve_path_arg(&arg);
        if resolved.is_file() || resolved.is_dir() {
            return Some(resolved.to_string_lossy().to_string());
        }
    }
    None
}

fn main() {
    tauri::Builder::default()
        .manage(AppState(Mutex::new(None)))
        .manage(DecodeCache::new(8))
        .setup(|app| {
            // 双击图片打开 / 命令行传参（文件或目录）：初始化浏览模型
            if let Some(path) = startup_image_path() {
                let p = Path::new(&path);
                let model = if p.is_dir() {
                    BrowseModel::open_first_in_dir(p)
                } else {
                    BrowseModel::open(p)
                };
                if let Some(model) = model {
                    let state = app.state::<AppState>();
                    if let Ok(mut guard) = state.0.lock() {
                        *guard = Some(model);
                    };
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::open_path,
            commands::next_image,
            commands::prev_image,
            commands::jump_folder,
            commands::get_initial_state,
            commands::load_image,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
