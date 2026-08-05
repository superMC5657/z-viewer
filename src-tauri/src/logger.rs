//! 开发日志（仅 debug 构建打印，release 完全消除）
//!
//! 约定：
//! - 后端日志统一前缀 [BE]（黄色 ANSI 33）；前端日志前缀 [FE]（青色），便于区分
//! - 每条日志带本地时间戳 `HH:mm:ss.SSS`（chrono，已在依赖树中，零额外编译成本）
//! - `cargo build`（debug）打印；`cargo build --release` 时宏体被 cfg 移除，零开销

/// 关键步骤日志：仅 debug 构建输出，黄色 [BE] 前缀 + 本地时间戳
#[macro_export]
macro_rules! dev_log {
    ($($arg:tt)*) => {
        #[cfg(debug_assertions)]
        {
            use std::fmt::Write as _;
            use chrono::Timelike;
            let now = chrono::Local::now();
            let mut ts = String::with_capacity(16);
            let _ = write!(ts, "{:02}:{:02}:{:02}.{:03}",
                now.hour(), now.minute(), now.second(), now.timestamp_subsec_millis());
            println!("\x1b[33m[BE]\x1b[0m {} {}", ts, format!($($arg)*));
        }
    };
}
