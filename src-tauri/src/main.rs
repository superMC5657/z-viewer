// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod browse;
mod commands;

use std::path::Path;
use std::sync::Mutex;

use browse::BrowseModel;
use commands::AppState;
use tauri::Manager;

/// 从命令行参数中找出第一个文件/目录路径（跳过以 `-` 开头的选项与可执行文件自身）
fn startup_image_path() -> Option<String> {
    let mut args = std::env::args();
    args.next(); // 可执行文件自身
    for arg in args {
        if arg.starts_with('-') {
            continue;
        }
        let p = Path::new(&arg);
        if p.is_file() || p.is_dir() {
            return Some(arg);
        }
    }
    None
}

fn main() {
    tauri::Builder::default()
        .manage(AppState(Mutex::new(None)))
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
