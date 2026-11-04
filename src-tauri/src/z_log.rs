//! 统一日志能力（Rust 侧）：复用 tauri-plugin-log v2，不造轮子。
//!
//! - dev：Debug + Stdout + LogDir + Webview（终端与前端控制台可见）
//! - release：Info + 仅 LogDir（零 Stdout/Webview 回显），统一写入以应用名称命名的日志文件（如 ZViewer.log）
//! - 日志只进 LogDir 文件，不碰业务 DB（无 sqlite）
//! - 本地运行为主，不自动上报

use std::path::{Path, PathBuf};

use log::LevelFilter;

use crate::util::{basename_of, first_line_of};
use tauri_plugin_log::{RotationStrategy, Target, TargetKind, TimezoneStrategy, WEBVIEW_TARGET};

const MAX_FILE_SIZE: u128 = 5 * 1024 * 1024; // 5MB
const ROTATION_KEEP: usize = 5; // KeepSome(5)：最多保留 5 个轮转文件
pub(crate) const PRUNE_KEEP_DAYS: u64 = 14;
pub(crate) const PRUNE_MAX_BYTES: u64 = 25 * 1024 * 1024; // viewer 本地盘小：25MB 上限

/// 日志门槛解析：`ZVIEWER_LOG` 优先，`RUST_LOG` 兜底，均未设置/非法时用默认。
/// 取值（大小写不敏感）：trace/debug/info/warn(error)/off；非法值穿透为 None。
fn parse_level(s: &str) -> Option<LevelFilter> {
    match s.trim().to_ascii_lowercase().as_str() {
        "trace" => Some(LevelFilter::Trace),
        "debug" => Some(LevelFilter::Debug),
        "info" => Some(LevelFilter::Info),
        "warn" | "warning" => Some(LevelFilter::Warn),
        "error" => Some(LevelFilter::Error),
        "off" => Some(LevelFilter::Off),
        _ => None,
    }
}

/// 双分支门槛：先 `ZVIEWER_LOG` 后 `RUST_LOG`，均缺席/非法时回退到调用方默认
/// （dev 默认 Debug，release 默认 Info）。
pub(crate) fn resolve_level(default: LevelFilter) -> LevelFilter {
    for key in ["ZVIEWER_LOG", "RUST_LOG"] {
        if let Ok(v) = std::env::var(key) {
            if let Some(l) = parse_level(&v) {
                return l;
            }
        }
    }
    default
}
/// 编译期配置解析缓存：tauri.conf.json 在进程内只解析一次（OnceLock）。
/// app_name/identifier 在日志 init、panic hook、修剪、导出等多处调用，
/// 每次重新 include_str + serde 解析纯属浪费（Top2 收口）。
fn parsed_tauri_config() -> &'static serde_json::Value {
    static CACHE: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| {
        serde_json::from_str(include_str!("../tauri.conf.json")).unwrap_or(serde_json::Value::Null)
    })
}

/// 从配置文件（tauri.conf.json）读取字符串字段：解析结果进程内缓存，
/// 取 trim 后非空值，字段缺失/为空时回退 fallback。
fn config_str_field(key: &str, fallback: &str) -> String {
    if let Some(v) = parsed_tauri_config().get(key).and_then(|v| v.as_str()) {
        let v = v.trim();
        if !v.is_empty() {
            return v.to_string();
        }
    }
    fallback.to_string()
}

/// 从配置文件（tauri.conf.json）获取应用名称（productName，兜底为 Cargo 包名）
pub fn app_name_from_config() -> String {
    config_str_field("productName", env!("CARGO_PKG_NAME"))
}

/// 从配置文件（tauri.conf.json）获取应用标识（identifier）
pub fn app_identifier_from_config() -> String {
    config_str_field("identifier", "com.zviewer.app")
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
            .level(resolve_level(LevelFilter::Debug))
            .level_for("hyper", LevelFilter::Warn)
            .level_for("reqwest", LevelFilter::Warn)
            .level_for("wry", LevelFilter::Warn)
            .level_for("tao", LevelFilter::Warn)
            .timezone_strategy(TimezoneStrategy::UseLocal)
            .format(fmt)
            .max_file_size(MAX_FILE_SIZE)
            .rotation_strategy(RotationStrategy::KeepSome(ROTATION_KEEP))
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
            .level(resolve_level(LevelFilter::Info))
            .level_for("hyper", LevelFilter::Warn)
            .level_for("reqwest", LevelFilter::Warn)
            .level_for("wry", LevelFilter::Warn)
            .level_for("tao", LevelFilter::Warn)
            .timezone_strategy(TimezoneStrategy::UseLocal)
            .format(fmt)
            .max_file_size(MAX_FILE_SIZE)
            .rotation_strategy(RotationStrategy::KeepSome(ROTATION_KEEP))
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
        if cur.contains('@') && cur.chars().any(|c| c.is_alphanumeric()) && cur.len() >= 5 {
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
                while j < s.len()
                    && !bytes[j].is_ascii_whitespace()
                    && bytes[j] != b'"'
                    && bytes[j] != b'\''
                    && bytes[j] != b','
                    && bytes[j] != b';'
                {
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

/// 判定路径是否为日志或崩溃文件（仅看文件名/扩展名，不做 is_file 检查）。
fn is_log_or_crash_file(p: &Path) -> bool {
    p.extension().is_some_and(|x| x == "log")
        || (p
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("crash-"))
            && p.extension().is_some_and(|x| x == "txt"))
}

/// 修剪日志目录：删 14 天前文件，总量超 25MB 时按 mtime 从旧到新删到预算内。
/// 绝不向外抛 panic，失败只记 log。
pub fn prune(dir: &Path, max_age_days: u64, max_total_bytes: u64) {
    let entries: Vec<(PathBuf, u64, std::time::SystemTime)> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| is_log_or_crash_file(p))
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
            log::debug!("zlog prune: removed expired {}", basename_of(&p));
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
            log::debug!("zlog prune: over budget removed {}", basename_of(&p));
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

/// crash 报告的 `last_viewed` 字段：只留 basename。
/// 全路径等于泄漏「用户崩溃时在看什么」（redact 只折叠家目录，非家目录路径拦不住）。
fn crash_last_viewed(raw: Option<String>) -> String {
    raw.map(|p| basename_of(Path::new(&p)))
        .unwrap_or_else(|| "<none>".to_string())
}

/// crash 报告的 `payload` 字段：只取首行并截断。panic 文案是自由文本，可能内嵌
/// 路径或大段错误原文，整段落盘绕过「路径只许 basename」「不记全文」红线。
/// （backtrace 只含构建机源码路径且 release `panic = "abort"` 不产 crash 文件，
/// 见 logging.md 已知限制，故维持全量保留诊断价值。）
fn crash_payload(raw: String) -> String {
    first_line_of(&raw)
}

fn write_crash_report(info: &std::panic::PanicHookInfo) {
    let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = info.payload().downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    };
    let payload = crash_payload(payload);
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "<unknown>".to_string());
    let bt = std::backtrace::Backtrace::force_capture();
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S%.3f");
    let app_name = app_name_from_config();
    let identifier = app_identifier_from_config();
    let last_image = crash_last_viewed(get_last_viewed());
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static SEQ: AtomicU32 = AtomicU32::new(0);

    fn temp_dir() -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("z-viewer-test-log-{}-{n}", std::process::id()));
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
    fn test_crash_last_viewed_basename_only() {
        // crash 报告红线：last_viewed 只落 basename，不落全路径
        assert_eq!(crash_last_viewed(None), "<none>");
        assert_eq!(
            crash_last_viewed(Some("/home/u/photos/a.jpg".into())),
            "a.jpg"
        );
        // 无文件名/空路径走 basename_of 的 unknown 回退
        assert_eq!(crash_last_viewed(Some("".into())), "unknown");
    }

    #[test]
    fn test_crash_payload_first_line() {
        // crash 报告红线：payload 只落首行并截断（panic 文案可能内嵌路径/大段原文）
        assert_eq!(
            crash_payload("first line\nsecond line".into()),
            "first line"
        );
        let long = "x".repeat(500);
        assert_eq!(crash_payload(long).chars().count(), 200);
    }

    #[test]
    fn test_converged_waterline() {
        // 收敛水位锁定：单文件 5MB / 保留 5 个轮转 / 14 天过期 / 总量 25MB
        assert_eq!(MAX_FILE_SIZE, 5 * 1024 * 1024);
        assert_eq!(ROTATION_KEEP, 5);
        assert_eq!(PRUNE_KEEP_DAYS, 14);
        assert_eq!(PRUNE_MAX_BYTES, 25 * 1024 * 1024);
    }

    #[test]
    fn test_format_path_masks() {
        // format 写盘前经 redact 二次脱敏：路径类载荷只留 basename 形态，
        // 家目录/email/token/key 均被掩码（回归：format 逻辑不动，只锁掩码行为）
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_default();
        if !home.is_empty() {
            let input = format!("open {home}/photos/a.jpg done");
            assert_eq!(redact(&input), "open ~/photos/a.jpg done");
        }
        assert_eq!(
            redact("browse open failed secret-photo by adm@example.com"),
            "browse open failed secret-photo by [redacted-email]"
        );
        assert_eq!(
            redact("decode failed a.cr2: Bearer abcdef12345"),
            "decode failed a.cr2: Bearer [redacted]"
        );
        assert_eq!(
            redact("license ZV-PRO-ABCD-1234-EFGH checked"),
            "license [redacted-key] checked"
        );
        // 全路径即使误入消息也不得原样落盘：家目录必被折叠
        if !home.is_empty() {
            let leaked = format!("cache miss {home}/x/b.jpg");
            assert!(!redact(&leaked).contains(&home));
        }
    }

    #[test]
    fn test_resolve_level_dual_branch() {
        // ZVIEWER_LOG / RUST_LOG 双分支：串行化 env 操作防并行测试串扰
        static GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let old_z = std::env::var("ZVIEWER_LOG").ok();
        let old_r = std::env::var("RUST_LOG").ok();
        let restore = |zv: &Option<String>, rl: &Option<String>| {
            match zv {
                Some(v) => std::env::set_var("ZVIEWER_LOG", v),
                None => std::env::remove_var("ZVIEWER_LOG"),
            }
            match rl {
                Some(v) => std::env::set_var("RUST_LOG", v),
                None => std::env::remove_var("RUST_LOG"),
            }
        };
        let set = |zv: Option<&str>, rl: Option<&str>| {
            match zv {
                Some(v) => std::env::set_var("ZVIEWER_LOG", v),
                None => std::env::remove_var("ZVIEWER_LOG"),
            }
            match rl {
                Some(v) => std::env::set_var("RUST_LOG", v),
                None => std::env::remove_var("RUST_LOG"),
            }
        };
        // 双缺席 → 默认穿透
        set(None, None);
        assert_eq!(resolve_level(LevelFilter::Debug), LevelFilter::Debug);
        assert_eq!(resolve_level(LevelFilter::Info), LevelFilter::Info);
        // ZVIEWER_LOG 优先于 RUST_LOG
        set(Some("warn"), Some("error"));
        assert_eq!(resolve_level(LevelFilter::Debug), LevelFilter::Warn);
        // 仅 RUST_LOG 生效
        set(None, Some("debug"));
        assert_eq!(resolve_level(LevelFilter::Info), LevelFilter::Debug);
        // 大小写不敏感
        set(Some("WARN"), None);
        assert_eq!(resolve_level(LevelFilter::Debug), LevelFilter::Warn);
        // ZVIEWER 非法时穿透到 RUST_LOG
        set(Some("verbose"), Some("error"));
        assert_eq!(resolve_level(LevelFilter::Debug), LevelFilter::Error);
        // 双非法 → 默认
        set(Some("verbose"), Some("nope"));
        assert_eq!(resolve_level(LevelFilter::Debug), LevelFilter::Debug);
        restore(&old_z, &old_r);
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

}
