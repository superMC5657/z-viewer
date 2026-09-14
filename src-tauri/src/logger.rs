//! 开发/统一日志：dev_log! 保留调用点，内部转接到 log::debug!（经 tauri-plugin-log 分发）。
//!
//! 约定：
//! - 后端日志统一前缀 [BE]（前端日志前缀 [FE]，便于区分）
//! - 时间戳/落盘/分级由 src-tauri/src/z_log.rs 的插件统一处理（dev=Debug+Stdout+LogDir+Webview，release=Info+仅LogDir）
//! - release 下 debug 被 Info 门槛过滤，零开销近似原宏的 cfg 消除语义

/// 关键步骤日志：保留原有调用点，内部转 log::debug!（[BE] 前缀保留）。
#[macro_export]
macro_rules! dev_log {
    ($($arg:tt)*) => {
        log::debug!("[BE] {}", format!($($arg)*));
    };
}
