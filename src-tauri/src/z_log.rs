//! 统一日志能力（Rust 侧）：复用 tauri-plugin-log v2，不造轮子。
//!
//! - dev：Debug + Stdout + LogDir + Webview（终端与前端控制台可见）
//! - release：Info + 仅 LogDir（零 Stdout/Webview 回显），统一写入以应用名称命名的日志文件（如 ZViewer.log）
//! - 日志只进 LogDir 文件，不碰业务 DB（无 sqlite）
//! - 本地运行为主，不自动上报，只提供导出诊断包命令供按钮接线

use std::io::Write as _;
use std::path::{Path, PathBuf};

use log::LevelFilter;
use tauri_plugin_log::{RotationStrategy, Target, TargetKind, TimezoneStrategy, WEBVIEW_TARGET};

const MAX_FILE_SIZE: u128 = 1024 * 1024; // 1MB
const PRUNE_KEEP_DAYS: u64 = 7;
const PRUNE_MAX_BYTES: u64 = 20 * 1024 * 1024; // viewer 本地盘小：20MB 上限

/// 从配置文件（tauri.conf.json）获取应用名称（productName，兜底为 Cargo 包名）
pub fn app_name_from_config() -> String {
    let conf_str = include_str!("../tauri.conf.json");
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(conf_str) {
        if let Some(name) = val.get("productName").and_then(|v| v.as_str()) {
            let name = name.trim();
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    env!("CARGO_PKG_NAME").to_string()
}

/// 从配置文件（tauri.conf.json）获取应用标识（identifier）
pub fn app_identifier_from_config() -> String {
    let conf_str = include_str!("../tauri.conf.json");
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(conf_str) {
        if let Some(id) = val.get("identifier").and_then(|v| v.as_str()) {
            let id = id.trim();
            if !id.is_empty() {
                return id.to_string();
            }
        }
    }
    "com.zviewer.app".to_string()
}

/// 日志插件工厂：dev=Debug+Stdout+LogDir+Webview，release=Info+仅LogDir。
pub fn init() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    // 全局 format：级别 + target + 本地 HH:mm:ss.SSS，写盘前二次脱敏
    let fmt = |out: tauri_plugin_log::fern::FormatCallback,
               message: &std::fmt::Arguments,
               record: &log::Record| {
        out.finish(format_args!(
            "[{}][{}][{}] {}",
            chrono::Local::now().format("%H:%M:%S%.3f"),
            record.level(),
            record.target(),
            redact(&message.to_string())
        ))
    };

    // 统一日志文件：rust 后端与 webview 前端日志写入以应用名命名的日志文件（从配置文件获取），统一因果时间轴
    let app_name = app_name_from_config();
    let app_file = Target::new(TargetKind::LogDir {
        file_name: Some(app_name),
    });

    #[cfg(debug_assertions)]
    {
        tauri_plugin_log::Builder::new()
            .level(LevelFilter::Debug)
            .level_for("hyper", LevelFilter::Warn)
            .level_for("reqwest", LevelFilter::Warn)
            .level_for("wry", LevelFilter::Warn)
            .level_for("tao", LevelFilter::Warn)
            .timezone_strategy(TimezoneStrategy::UseLocal)
            .format(fmt)
            .max_file_size(MAX_FILE_SIZE)
            .rotation_strategy(RotationStrategy::KeepAll)
            .targets([
                Target::new(TargetKind::Stdout),
                app_file,
                // 仅把 Rust 日志回显到 devtools：前端日志本地已有 console 输出，
                // 全量回显会形成 前端->后端->前端 的回声循环
                Target::new(TargetKind::Webview)
                    .filter(|md| !md.target().starts_with(WEBVIEW_TARGET)),
            ])
            .build()
    }

    #[cfg(not(debug_assertions))]
    {
        // release：clear_targets 后只留 LogDir，零 Stdout/Webview 回显
        tauri_plugin_log::Builder::new()
            .level(LevelFilter::Info)
            .level_for("hyper", LevelFilter::Warn)
            .level_for("reqwest", LevelFilter::Warn)
            .level_for("wry", LevelFilter::Warn)
            .level_for("tao", LevelFilter::Warn)
            .timezone_strategy(TimezoneStrategy::UseLocal)
            .format(fmt)
            .max_file_size(MAX_FILE_SIZE)
            .rotation_strategy(RotationStrategy::KeepAll)
            .clear_targets()
            .targets([app_file])
            .build()
    }
}

/// 脱敏：homeDir->~、email、bearer/token、license key（[A-Z0-9-]{16,}）。
/// 无正则依赖，纯手写扫描，避免新增编译负担；绝不 panic。
pub fn redact(input: &str) -> String {
    let mut out = input.to_string();
    out = mask_home_dir(&out);
    out = mask_emails(&out);
    out = mask_bearer_token(&out);
    out = mask_license_keys(&out);
    out
}

fn mask_home_dir(s: &str) -> String {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    if home.is_empty() {
        return s.to_string();
    }
    s.replace(&home, "~")
}

/// 粗粒度 email 脱敏：含 @ 的连续 token 整体替换（保留首尾可读性低，直接整块替换）。
fn mask_emails(s: &str) -> String {
    let mut res = String::with_capacity(s.len());
    let mut cur = String::new();
    let flush = |cur: &mut String, res: &mut String| {
        if cur.contains('@')
            && cur.chars().any(|c| c.is_alphanumeric())
            && cur.len() >= 5
        {
            res.push_str("[redacted-email]");
        } else {
            res.push_str(cur);
        }
        cur.clear();
    };
    for ch in s.chars() {
        if ch.is_whitespace() || matches!(ch, '"' | '\'' | '<' | '>' | ',' | ';' | '(' | ')') {
            flush(&mut cur, &mut res);
            res.push(ch);
        } else {
            cur.push(ch);
        }
    }
    flush(&mut cur, &mut res);
    res
}

/// bearer/token 脱敏（大小写不敏感）：`bearer xxx` -> `bearer [redacted]`，
/// `token=xxx` / `token: xxx` -> `token=[redacted]`。
fn mask_bearer_token(s: &str) -> String {
    let v = mask_token_keyword(s, "bearer");
    mask_token_kv(&v)
}

fn mask_token_keyword(s: &str, keyword: &str) -> String {
    let lower = s.to_lowercase();
    let kw = keyword.to_lowercase();
    let mut res = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < s.len() {
        if lower[i..].starts_with(kw.as_str()) {
            let after = i + kw.len();
            // bearer 后必须跟空白才算（避免误伤 bearing 等词）
            if after < s.len() && bytes[after].is_ascii_whitespace() {
                let mut j = after;
                while j < s.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                let val_start = j;
                while j < s.len() && !bytes[j].is_ascii_whitespace() && bytes[j] != b'"' && bytes[j] != b'\'' && bytes[j] != b',' && bytes[j] != b';' {
                    j += 1;
                }
                if val_start < j {
                    res.push_str(&s[i..after]);
                    // 保留原始空白
                    res.push_str(&s[after..val_start]);
                    res.push_str("[redacted]");
                    i = j;
                    continue;
                }
            }
        }
        // 按 char 推进（避免 UTF-8 切半）
        let ch = s[i..].chars().next().unwrap_or('\0');
        if ch == '\0' {
            break;
        }
        res.push(ch);
        i += ch.len_utf8();
    }
    res
}

fn mask_token_kv(s: &str) -> String {
    // 处理 token / api_token / access_token / analyticsToken 等 `xxx_token[=:] value` 形态
    let lower = s.to_lowercase();
    let mut res = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < s.len() {
        if let Some(pos) = lower[i..].find("token") {
            let k = i + pos;
            let mut j = k + "token".len();
            while j < s.len() && (bytes[j] == b'_' || bytes[j].is_ascii_alphanumeric()) {
                j += 1; // token 后缀如 s（tokens）直接跳过到分隔符
            }
            // 允许 token 前缀词（如 analyticsToken）：只要以 token 结尾即可
            let mut sep = j;
            // JSON 形态 `"xxxToken": "value"`：token 名与分隔符之间允许一个引号
            if sep < s.len() && (bytes[sep] == b'"' || bytes[sep] == b'\'') {
                sep += 1;
            }
            while sep < s.len() && bytes[sep].is_ascii_whitespace() {
                sep += 1;
            }
            if sep < s.len() && (bytes[sep] == b'=' || bytes[sep] == b':') {
                let mut v = sep + 1;
                while v < s.len() && bytes[v].is_ascii_whitespace() {
                    v += 1;
                }
                let quote_start = v;
                // 可选引号
                let quote = if v < s.len() && (bytes[v] == b'"' || bytes[v] == b'\'') {
                    v += 1;
                    Some(bytes[v - 1])
                } else {
                    None
                };
                let val_start = v;
                while v < s.len()
                    && !bytes[v].is_ascii_whitespace()
                    && bytes[v] != b'"'
                    && bytes[v] != b'\''
                    && bytes[v] != b','
                    && bytes[v] != b';'
                {
                    v += 1;
                }
                if val_start < v {
                    res.push_str(&s[i..=sep]);
                    // 保留分隔符后到引号/值之间的空白
                    res.push_str(&s[sep + 1..quote_start]);
                    if let Some(q) = quote {
                        res.push(q as char);
                    }
                    res.push_str("[redacted]");
                    if let Some(q) = quote {
                        res.push(q as char);
                    }
                    i = v + usize::from(quote.is_some() && v < s.len());
                    continue;
                }
            }
            // 不是 kv 形态：原样推进到 j，避免死循环
            res.push_str(&s[i..j]);
            i = j;
            continue;
        }
        let ch = s[i..].chars().next().unwrap_or('\0');
        if ch == '\0' {
            break;
        }
        res.push(ch);
        i += ch.len_utf8();
    }
    res
}

/// license key 脱敏：连续 [A-Z0-9-] 且长度>=16（含至少一个 `-` 或数字）-> [redacted-key]。
fn mask_license_keys(s: &str) -> String {
    let mut res = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < s.len() {
        let b = bytes[i];
        if b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-' {
            let mut j = i;
            while j < s.len()
                && (bytes[j].is_ascii_uppercase() || bytes[j].is_ascii_digit() || bytes[j] == b'-')
            {
                j += 1;
            }
            let run = &s[i..j];
            let has_dash = run.contains('-');
            let has_digit = run.chars().any(|c| c.is_ascii_digit());
            if run.len() >= 16 && (has_dash || has_digit) {
                res.push_str("[redacted-key]");
            } else {
                res.push_str(run);
            }
            i = j;
            continue;
        }
        let ch = s[i..].chars().next().unwrap_or('\0');
        if ch == '\0' {
            break;
        }
        res.push(ch);
        i += ch.len_utf8();
    }
    res
}

/// 修剪日志目录：删 7 天前文件，总量超 20MB 时按 mtime 从旧到新删到预算内。
/// 绝不向外抛 panic，失败只记 log。
pub fn prune(dir: &Path, max_age_days: u64, max_total_bytes: u64) {
    let entries: Vec<(PathBuf, u64, std::time::SystemTime)> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.extension().is_some_and(|x| x == "log")
                    || p.file_name().is_some_and(|n| {
                        n.to_string_lossy().starts_with("crash-")
                            && p.extension().is_some_and(|x| x == "txt")
                    })
            })
            .filter_map(|p| {
                let md = std::fs::metadata(&p).ok()?;
                Some((p, md.len(), md.modified().ok()?))
            })
            .collect(),
        Err(_) => return,
    };
    let now = std::time::SystemTime::now();
    let max_age = std::time::Duration::from_secs(max_age_days * 24 * 3600);
    let mut kept: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    for (p, len, mtime) in entries {
        let age = now.duration_since(mtime).unwrap_or_default();
        if age > max_age {
            let _ = std::fs::remove_file(&p);
            log::info!("zlog prune: 删除过期日志 {}", p.display());
        } else {
            kept.push((p, len, mtime));
        }
    }
    let mut total: u64 = kept.iter().map(|(_, l, _)| *l).sum();
    if total <= max_total_bytes {
        return;
    }
    kept.sort_by_key(|(_, _, t)| *t); // 最旧在前
    for (p, len, _) in kept {
        if total <= max_total_bytes {
            break;
        }
        if std::fs::remove_file(&p).is_ok() {
            total = total.saturating_sub(len);
            log::info!("zlog prune: 超预算删除 {}", p.display());
        }
    }
}

/// 解析 tauri 推荐日志目录（无 AppHandle 时的兜底，供 panic hook 使用）。
fn resolve_log_dir_fallback() -> PathBuf {
    let identifier = app_identifier_from_config();
    #[cfg(target_os = "windows")]
    {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            return PathBuf::from(local).join(&identifier).join("logs");
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home)
                .join("Library")
                .join("Logs")
                .join(&identifier);
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let base = std::env::var("XDG_DATA_HOME").unwrap_or_default();
        if !base.is_empty() {
            return PathBuf::from(base).join(&identifier).join("logs");
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home)
                .join(".local")
                .join("share")
                .join(&identifier)
                .join("logs");
        }
    }
    std::env::temp_dir().join(format!("{identifier}-logs"))
}

static LAST_VIEWED: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// 记录当前正在查看的文件路径（供崩溃报告溯源使用）
pub fn set_last_viewed(path: &str) {
    if let Ok(mut g) = LAST_VIEWED.lock() {
        *g = Some(path.to_string());
    }
}

pub fn get_last_viewed() -> Option<String> {
    LAST_VIEWED.lock().ok().and_then(|g| g.clone())
}

/// 安装 panic hook：take_hook + 写 crash-{ts}.txt + Backtrace + 链回默认，绝不二次 panic。
pub fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // 绝不二次 panic：全程 catch_unwind + 忽略错误，写完链回默认 hook
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| write_crash_report(info)));
        prev(info);
    }));
}

fn write_crash_report(info: &std::panic::PanicHookInfo) {
    let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = info.payload().downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    };
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "<unknown>".to_string());
    let bt = std::backtrace::Backtrace::force_capture();
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S%.3f");
    let app_name = app_name_from_config();
    let identifier = app_identifier_from_config();
    let last_image = get_last_viewed().unwrap_or_else(|| "<none>".to_string());
    let body = format!(
        "product: {}\nversion: {}\nidentifier: {}\ntime: {}\nthread: {:?}\nlocation: {}\nlast_viewed: {}\npayload: {}\nbacktrace:\n{}\n",
        app_name,
        env!("CARGO_PKG_VERSION"),
        identifier,
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f %:z"),
        std::thread::current().name().unwrap_or("<unnamed>").to_string(),
        location,
        last_image,
        payload,
        bt
    );
    let body = redact(&body);
    let dir = resolve_log_dir_fallback();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("crash-{ts}.txt"));
    let _ = std::fs::write(&path, body.as_bytes());
}

/// 取日志目录（供前端“打开日志目录 / 导出诊断包”按钮接线）。
#[tauri::command]
pub fn zlog_get_dir(app: tauri::AppHandle) -> Result<String, String> {
    use tauri::Manager as _;
    let dir = app.path().app_log_dir().map_err(|e| e.to_string())?;
    let _ = std::fs::create_dir_all(&dir);
    prune(&dir, PRUNE_KEEP_DAYS, PRUNE_MAX_BYTES);
    Ok(dir.to_string_lossy().to_string())
}

/// 导出诊断包：zip（logs 中 *.log + crash-*.txt + version.txt），打包时二次脱敏。
#[tauri::command]
pub async fn zlog_export_bundle(app: tauri::AppHandle) -> Result<String, String> {
    use tauri::Manager as _;
    let dir = app.path().app_log_dir().map_err(|e| e.to_string())?;
    let _ = std::fs::create_dir_all(&dir);
    prune(&dir, PRUNE_KEEP_DAYS, PRUNE_MAX_BYTES);

    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let app_name = app_name_from_config();
    let out_path = dir.join(format!("{}-logs-{ts}.zip", app_name.to_lowercase()));
    export_bundle_inner(&dir, &out_path)?;
    Ok(out_path.to_string_lossy().to_string())
}

fn export_bundle_inner(dir: &Path, out_path: &Path) -> Result<(), String> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && (p.extension().is_some_and(|x| x == "log")
                    || (p.file_name().is_some_and(|n| {
                        n.to_string_lossy().starts_with("crash-")
                    }) && p.extension().is_some_and(|x| x == "txt")))
        })
        .collect();
    files.sort();

    let f = std::fs::File::create(out_path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(f);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    for src in &files {
        let name = src
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown.log".to_string());
        let raw = std::fs::read(src).map_err(|e| e.to_string())?;
        // 二次脱敏：按 UTF-8 lossy 转文本再 redact 后写入
        let text = String::from_utf8_lossy(&raw);
        let clean = redact(&text);
        zip.start_file(name, opts).map_err(|e| e.to_string())?;
        zip.write_all(clean.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    let app_name = app_name_from_config();
    let identifier = app_identifier_from_config();
    let version_txt = format!(
        "product: {}\nversion: {}\nidentifier: {}\nexported_at: {}\n",
        app_name,
        env!("CARGO_PKG_VERSION"),
        identifier,
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f %:z")
    );
    zip.start_file("version.txt", opts)
        .map_err(|e| e.to_string())?;
    zip.write_all(version_txt.as_bytes())
        .map_err(|e| e.to_string())?;
    zip.finish().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static SEQ: AtomicU32 = AtomicU32::new(0);

    fn temp_dir() -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("z-viewer-test-log-{}-{n}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn test_redact_email() {
        let input = "User supermc@example.com logged in";
        assert_eq!(redact(input), "User [redacted-email] logged in");

        let input_bracket = "Contact: <support@zviewer.app>;";
        assert_eq!(redact(input_bracket), "Contact: <[redacted-email]>;");
    }

    #[test]
    fn test_redact_bearer_and_tokens() {
        let input1 = "Authorization: Bearer secret-token-12345";
        assert_eq!(redact(input1), "Authorization: Bearer [redacted]");

        let input2 = "token=abc123456789; status=ok";
        assert_eq!(redact(input2), "token=[redacted]; status=ok");

        let input3 = r#"{"analyticsToken": "super-secret-jwt-token"}"#;
        assert_eq!(redact(input3), r#"{"analyticsToken": "[redacted]"}"#);
    }

    #[test]
    fn test_redact_license_keys() {
        let input = "License key is ZV-PRO-ABCD-1234-EFGH";
        assert_eq!(redact(input), "License key is [redacted-key]");
    }

    #[test]
    fn test_prune_size_budget() {
        let dir = temp_dir();
        let f1 = dir.join("app_old.log");
        let f2 = dir.join("app_new.log");

        // 写入测试文件
        std::fs::write(&f1, vec![b'a'; 1000]).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&f2, vec![b'b'; 1000]).unwrap();

        // 限制总量 1500 字节，应删掉最旧的 f1
        prune(&dir, 7, 1500);
        assert!(!f1.exists(), "old file should be pruned");
        assert!(f2.exists(), "newer file should be kept");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_app_name_and_identifier_from_config() {
        let name = app_name_from_config();
        assert_eq!(name, "ZViewer");

        let id = app_identifier_from_config();
        assert_eq!(id, "com.zviewer.app");
    }

    #[test]
    fn test_export_bundle_inner() {
        let dir = temp_dir();
        let app_name = app_name_from_config();
        let log_file_name = format!("{app_name}.log");
        let log_file = dir.join(&log_file_name);
        let crash_file = dir.join("crash-20260916.txt");
        let other_file = dir.join("ignore.json");

        std::fs::write(&log_file, "Hello Bearer secret123").unwrap();
        std::fs::write(&crash_file, "Panic at test@example.com").unwrap();
        std::fs::write(&other_file, "Ignored").unwrap();

        let zip_path = dir.join("bundle.zip");
        export_bundle_inner(&dir, &zip_path).expect("export should succeed");

        assert!(zip_path.exists());

        // 校验 zip 内容
        let file = std::fs::File::open(&zip_path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();

        let mut names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        names.sort();

        let mut expected = vec![log_file_name.as_str(), "crash-20260916.txt", "version.txt"];
        expected.sort();
        assert_eq!(names, expected);

        // 验证 zip 内二次脱敏生效
        use std::io::Read as _;
        let mut app_content = String::new();
        archive.by_name(&log_file_name).unwrap().read_to_string(&mut app_content).unwrap();
        assert_eq!(app_content, "Hello Bearer [redacted]");

        let mut crash_content = String::new();
        archive.by_name("crash-20260916.txt").unwrap().read_to_string(&mut crash_content).unwrap();
        assert_eq!(crash_content, "Panic at [redacted-email]");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
