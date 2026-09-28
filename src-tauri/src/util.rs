//! 通用小工具：日志/时间辅助的单一实现，避免各模块复制同一逻辑

use std::path::Path;
use std::time::UNIX_EPOCH;

/// 日志用 basename：只记文件名，不记全路径（用户名/目录结构不出日志）。
/// 无文件名（如以 `..` 结尾）或空名回退 `"unknown"`。
pub fn basename_of<P: AsRef<Path>>(path: P) -> String {
    path.as_ref()
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// 日志用原因摘要：取首行 + 字符截断封顶（200 字符），
/// 不记大段文本/服务端原文全文。
pub fn first_line_of(s: &str) -> String {
    const MAX_CHARS: usize = 200;
    let line = s.lines().next().unwrap_or("").trim();
    if line.chars().count() > MAX_CHARS {
        line.chars().take(MAX_CHARS).collect()
    } else {
        line.to_string()
    }
}

/// 当前 Unix 秒；系统时间早于 UNIX_EPOCH（异常时钟）时回退 0。
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
