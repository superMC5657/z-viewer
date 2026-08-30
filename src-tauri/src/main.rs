// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod analytics;
mod browse;
mod cache;
mod commands;
mod decode;
mod license;
mod logger;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use browse::BrowseModel;
use cache::{DecodeCache, FolderFirstCache};
use commands::{AppSettings, AppState, SettingsState};
use tauri::{Emitter, Manager};

/// 解析命令行传入的路径：相对路径优先按项目根解析
/// （`pnpm tauri dev -- <path>` 时应用 cwd 为 src-tauri/，相对路径会失效）
fn resolve_path_arg(arg: &str) -> PathBuf {
    let p = Path::new(arg);
    if p.exists() || p.is_absolute() {
        return p.to_path_buf();
    }
    // exe 位于 <项目根>/src-tauri/target/<profile>/，上溯 4 级到项目根
    // （仅开发构建需要；release 安装后 cwd 语义不变，上溯反而可能误匹配无关同名文件）
    #[cfg(debug_assertions)]
    if let Ok(exe) = std::env::current_exe() {
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
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState(Mutex::new(None)))
        .manage(analytics::SessionStats::new())
        .manage(SettingsState(Mutex::new(AppSettings::default())))
        .manage(DecodeCache::new(4))
        .manage(FolderFirstCache::new(4))
        .setup(|app| {
            // 授权状态：从磁盘加载许可证；后台在线验证（吊销生效，网络失败不阻止）
            // 商店/授权配置：编译期固化在 tauri.conf.json → plugins.store
            let store = license::StoreConfig::from_config(app.config());
            let license_path = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::env::temp_dir())
                .join(&store.license_file_name);
            let license = license::LicenseManager::load(license_path, store);
            {
                let app_handle = app.handle().clone();
                let license = license.clone();
                tauri::async_runtime::spawn(async move {
                    if let Some(status) = license.verify_online().await {
                        let _ = app_handle.emit(license::LICENSE_STATUS_CHANGED, status);
                    }
                });
            }
            app.manage(license.clone());
            let pro = license.is_pro();
            if !pro {
                // 免费版：缓存容量归零（队列清空），设置锁定为关闭
                let cache = app.state::<DecodeCache>();
                cache.set_capacity(0);
                let first_cache = app.state::<FolderFirstCache>();
                first_cache.set_capacity(0);
                if let Ok(mut s) = app.state::<SettingsState>().0.lock() {
                    s.cache_level = 0;
                }
            }

            // 双击图片打开 / 命令行传参（文件或目录）：初始化浏览模型
            if let Some(path) = startup_image_path() {
                let p = Path::new(&path);
                let on_ready = Some(commands::on_ready_callback(app.handle().clone()));
                let model = if p.is_dir() {
                    BrowseModel::open_first_in_dir_gated(p, on_ready, pro)
                } else {
                    BrowseModel::open_gated(p, on_ready, pro)
                };
                if let Some(model) = model {
                    let state = app.state::<AppState>();
                    if let Ok(mut guard) = state.0.lock() {
                        *guard = Some(std::sync::Arc::new(model));
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
            commands::check_animation,
            commands::record_view,
            commands::set_cache_level,
            commands::get_settings,
            commands::get_raw_extensions,
            commands::get_context,
            license::activate_license,
            license::deactivate_license,
            license::get_license_status,
            license::get_store_info,
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|app, event| {
            // 正常退出（点关闭/Alt+F4/菜单退出）时上报会话统计；kill 强杀无执行机会
            if let tauri::RunEvent::ExitRequested { .. } = event {
                analytics::report_exit(app);
            }
        });
}
