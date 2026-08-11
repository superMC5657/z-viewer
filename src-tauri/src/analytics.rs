//! 会话统计埋点：记录本次运行的读图行为，正常退出时上报
//!
//! 上报时机与边界（用户确认）：
//! - 仅应用正常退出（点窗口关闭 / Alt+F4 / 菜单退出）时上报，挂在
//!   `tauri::RunEvent::ExitRequested` 回调，用 `async_runtime::block_on` 阻塞
//!   发送（3 秒超时），保证请求发出后才退出进程。
//! - `kill` / 任务管理器强杀进程无任何代码执行机会，无法上报（已知限制）。
//!
//! 上报地址：`plugins.store.apiBase` + `analyticsPath`（tauri.conf.json 编译期配置，
//! 与激活/在线验证同源）。
//!
//! 隐私边界：**只上报聚合统计，绝不上报文件路径/文件名/文件夹名**。
//! 路径仅存内存用于去重计数，序列化时只输出数量。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::Manager;

/// 会话统计（Tauri managed state；record 在 load_image 命令中调用）
pub struct SessionStats(Mutex<SessionStatsInner>);

#[derive(Default)]
struct SessionStatsInner {
    /// 应用启动时刻（Unix 秒，setup 时初始化）
    session_start: i64,
    /// 读图次数（含重复浏览）
    images_viewed: u64,
    /// 去重图片（绝对路径仅存内存，不上报）
    unique_images: HashSet<String>,
    /// 图片格式分布：扩展名（小写，不带点）→ 次数
    formats: HashMap<String, u64>,
    /// 本次会话浏览过的文件夹（绝对路径仅存内存去重，不上报，只输出数量）
    folders: HashSet<String>,
}

impl SessionStats {
    pub fn new() -> Self {
        Self(Mutex::new(SessionStatsInner {
            session_start: now(),
            ..Default::default()
        }))
    }

    /// 记录一次成功读图（前端 record_view 命令调用；asset/IPC 两通道显示成功都会调用）
    pub fn record(&self, path: &str) {
        if let Ok(mut g) = self.0.lock() {
            g.images_viewed += 1;
            g.unique_images.insert(normalize_key(path));
            if let Some(ext) = Path::new(path)
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
            {
                *g.formats.entry(ext).or_default() += 1;
            }
            if let Some(dir) = Path::new(path).parent().filter(|d| !d.as_os_str().is_empty())
            {
                g.folders.insert(normalize_key(&dir.to_string_lossy()));
            }
        }
    }

    /// 组装上报负载（聚合统计，无路径信息）
    fn snapshot(&self, app: &tauri::AppHandle) -> serde_json::Value {
        let g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        // 授权状态（license.rs 的 status：debug 恒 pro，release 按许可证）
        let license_status = app
            .try_state::<crate::license::LicenseManager>()
            .map(|l| l.status().status)
            .unwrap_or_else(|| "unknown".into());
        // 缓存等级（0=关闭 1=开启 2=高；免费版恒 0）
        let cache_level = app
            .try_state::<crate::commands::SettingsState>()
            .map(|s| s.0.lock().ok().map(|g| g.cache_level).unwrap_or(0))
            .unwrap_or(0);

        build_payload(&g, &license_status, cache_level)
    }
}

/// 负载组装（与 Tauri state 解耦，便于单元测试）
fn build_payload(
    g: &SessionStatsInner,
    license_status: &str,
    cache_level: usize,
) -> serde_json::Value {
    serde_json::json!({
            "deviceId": crate::license::device_id(),
            "licenseStatus": license_status,
            "version": env!("CARGO_PKG_VERSION"),
            "os": os_version(),
            "arch": std::env::consts::ARCH,
            "sessionStart": g.session_start,
            "sessionEnd": now(),
            "imagesViewed": g.images_viewed,
            "uniqueImages": g.unique_images.len(),
            "formats": g.formats,
            "foldersViewed": g.folders.len(),
            "cacheLevel": cache_level,
    })
}

impl Default for SessionStats {
    fn default() -> Self {
        Self::new()
    }
}

/// 正常退出上报：组装负载 → block_on 发送（3 秒超时，不阻塞退出太久）
/// apiBase 未配置 / 网络失败均静默（埋点不打扰用户，也不拖慢退出）
pub fn report_exit(app: &tauri::AppHandle) {
    let Some(stats) = app.try_state::<SessionStats>() else {
        return;
    };
    let store = crate::license::StoreConfig::from_config(app.config());
    if store.api_base.is_empty() || store.analytics_path.is_empty() {
        return;
    }
    let payload = stats.snapshot(app);
    let url = format!("{}{}", store.api_base, store.analytics_path);
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
    {
        Ok(c) => c,
        Err(_) => return,
    };
    let _ = tauri::async_runtime::block_on(async move {
        let _ = client.post(&url).json(&payload).send().await;
    });
}

/// 去重键规范化：统一分隔符为 `/`；Windows 路径大小写不敏感，统一转小写，
/// 保证同一文件/文件夹无论以 `C:\Photos\a.JPG` 还是 `c:/photos/a.jpg` 形态出现都只计一次
/// （命令行传入与浏览模型产出的路径形态可能不同）
fn normalize_key(path: &str) -> String {
    let unified = path.replace('\\', "/");
    #[cfg(windows)]
    {
        unified.to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        unified
    }
}

/// 当前 Unix 秒
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 操作系统版本描述（Windows 读注册表 ProductName + CurrentBuild，其他平台给系统名）
fn os_version() -> String {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_LOCAL_MACHINE;
        use winreg::RegKey;
        if let Ok(k) = RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
        {
            let name: String = k.get_value("ProductName").unwrap_or_default();
            let build: String = k.get_value("CurrentBuild").unwrap_or_default();
            if !name.is_empty() {
                return if build.is_empty() {
                    name
                } else {
                    format!("{name} (build {build})")
                };
            }
        }
        "windows".into()
    }
    #[cfg(not(windows))]
    {
        std::env::consts::OS.to_string()
    }
}

// ---------- 测试 ----------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_aggregates_without_paths_in_snapshot() {
        let s = SessionStats::new();
        s.record("C:/photos/a.JPG");
        s.record("C:/photos/a.JPG"); // 重复浏览
        s.record("C:/photos/b.png");
        s.record("D:/raw/c.cr2");

        let g = s.0.lock().unwrap();
        assert_eq!(g.images_viewed, 4);
        assert_eq!(g.unique_images.len(), 3);
        assert_eq!(g.folders.len(), 2);
        assert_eq!(*g.formats.get("jpg").unwrap(), 2);
        assert_eq!(*g.formats.get("png").unwrap(), 1);
        assert_eq!(*g.formats.get("cr2").unwrap(), 1);
    }

    #[test]
    fn snapshot_shape_and_privacy() {
        let s = SessionStats::new();
        s.record("C:/private/secret.png");
        let inner = s.0.lock().unwrap();
        let v = build_payload(&inner, "free", 0);
        // 必填字段齐全
        assert!(v["imagesViewed"].as_u64().is_some());
        assert!(v["uniqueImages"].as_u64().is_some());
        assert!(v["sessionStart"].as_i64().is_some());
        assert!(v["sessionEnd"].as_i64().is_some());
        assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(v["licenseStatus"], "free");
        assert_eq!(v["cacheLevel"], 0);
        // 隐私：序列化结果中不得出现任何路径
        let text = serde_json::to_string(&v).unwrap();
        assert!(!text.contains("private"), "上报负载不得包含路径信息");
        assert!(!text.contains("secret"));
    }

    #[test]
    fn record_dedupes_by_normalized_key() {
        let s = SessionStats::new();
        // 分隔符不同的同一文件只计一次（replace('\\', "/") 全平台生效）
        s.record("C:/photos/a.jpg");
        s.record("C:\\photos\\a.jpg");
        let g = s.0.lock().unwrap();
        assert_eq!(g.images_viewed, 2);
        assert_eq!(g.unique_images.len(), 1);
        #[cfg(windows)]
        assert_eq!(g.folders.len(), 1); // 同一文件夹的两种形态
    }

    #[cfg(windows)]
    #[test]
    fn record_dedupes_case_insensitive_on_windows() {
        let s = SessionStats::new();
        // 同一文件的大小写/分隔符变体只计一次
        s.record("C:/Photos/IMG_0001.JPG");
        s.record("c:\\photos\\img_0001.jpg");
        s.record("D:/RAW/raw1.cr2");
        s.record("d:\\raw\\raw2.CR2");
        let g = s.0.lock().unwrap();
        assert_eq!(g.images_viewed, 4);
        assert_eq!(g.unique_images.len(), 3); // IMG_0001(去重) + raw1 + raw2
        assert_eq!(g.folders.len(), 2); // photos + raw（各含大小写/分隔符变体）
        assert_eq!(*g.formats.get("jpg").unwrap(), 2);
        assert_eq!(*g.formats.get("cr2").unwrap(), 2);
    }
}
