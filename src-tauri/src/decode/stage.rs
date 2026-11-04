//! 解码分阶段计时：机制单点实现，各通道仅以 `timed(..)` 标注阶段。
//!
//! 收集器线程局部（解码在 spawn_blocking 工作线程内同步完成，同线程
//! begin→timed→emit 天然成对）；会话由 `load_image_inner` 控制：仅当日志
//! 门槛为 debug（或基准测试显式 force）才收集，release 生产路径除每处标记
//! 一次 `log_enabled!` 判断外零开销、不落盘（现状维持）。
//!
//! 阶段语义（单张解码的分段构成）：
//! - `read`：文件打开与正文读取（TIFF 通道 open_with_orientation + 正文整体读入
//!   内存，engine 段只剩纯解码；JXL/RAW 引擎自带文件 IO，读取不可单独分拆归入
//!   engine 段）；
//! - `engine`：像素解码；
//! - `orient` / `resize` / `rgba`：finalize_rgba 内方向回正 / cap 降采样 /
//!   RGBA 输出（cap_dimensions 是全通道降采样收口；JXL 通道产物封顶亦经此，
//!   但在 engine 计时段外执行）；
//! - `pack`：信封打包在命令层且换线程执行，不进 decode 段行——生产由命令层
//!   `decode pack` 行单独计时（与 stages 行按 basename+时序关联），bench 侧
//!   计入分段报表。
//!
//! 可加性：各通道分段严格可加（JXL 逐帧 cap 移出 engine 计时段后无重复计入）。
//! `decode stages` 行仅 debug 可见，解读分段数据时以此为准（bench 报表另注）。

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// 阶段名常量（打点与报表两侧唯一真源）：字面量散落各调用点时，typo 会静默
/// 分裂统计桶，一律引用常量。bench 报表（crate 根）也读 PACK，故 pub(crate)。
pub(crate) mod name {
    pub(crate) const READ: &str = "read";
    pub(crate) const ENGINE: &str = "engine";
    pub(crate) const ORIENT: &str = "orient";
    pub(crate) const RESIZE: &str = "resize";
    pub(crate) const RGBA: &str = "rgba";
    // pack 段不进 decode 会话（见模块注释），唯一消费者是 #[cfg(test)] 的
    // 翻页延迟基准报表——非测试构建下按 dead_code 静默
    #[allow(dead_code)]
    pub(crate) const PACK: &str = "pack";
}

thread_local! {
    static STAGES: RefCell<Vec<(&'static str, Duration)>> = const { RefCell::new(Vec::new()) };
}

/// 同名阶段累计（find-or-insert）：decode 会话线程局部与 bench 聚合（f64 ms）
/// 共用同一形状，单一实现
pub(crate) fn accumulate<T: Copy + std::ops::AddAssign>(
    v: &mut Vec<(&'static str, T)>,
    name: &'static str,
    d: T,
) {
    match v.iter_mut().find(|(n, _)| *n == name) {
        Some(e) => e.1 += d,
        None => v.push((name, d)),
    }
}

/// 基准/测试显式开启开关（生产由日志门槛动态判定，见 `begin`）
static FORCE: AtomicBool = AtomicBool::new(false);

/// 基准测试显式开启收集（无日志后端的测试/基准环境；生产由日志门槛动态判定）。
/// 仅测试/基准代码引用（bench 与 decode 测试均为 #[cfg(test)]），随构建剔除。
#[cfg(test)]
pub(crate) fn force(on: bool) {
    FORCE.store(on, Ordering::Relaxed);
}

fn active() -> bool {
    FORCE.load(Ordering::Relaxed) || log::log_enabled!(log::Level::Debug)
}

/// 包裹一段同步工作并按名累计耗时；未激活时直通（release 生产零开销）
pub(super) fn timed<T>(stage: &'static str, f: impl FnOnce() -> T) -> T {
    if !active() {
        return f();
    }
    let t0 = Instant::now();
    let out = f();
    let dt = t0.elapsed();
    STAGES.with(|s| accumulate(&mut s.borrow_mut(), stage, dt));
    out
}

/// 会话开始（激活时清空累计）；返回是否激活，调用方据此决定是否 emit
pub(super) fn begin() -> bool {
    if !active() {
        return false;
    }
    STAGES.with(|s| s.borrow_mut().clear());
    true
}

/// 会话结束：debug 单行分段明细（`decode stages {basename} read=.. engine=.. total=..`）
pub(super) fn emit(basename: &str, total_ms: u128) {
    if !active() {
        return;
    }
    STAGES.with(|s| {
        let v = s.borrow();
        if v.is_empty() {
            return;
        }
        let mut line = String::new();
        for (name, dt) in v.iter() {
            line.push_str(&format!(" {name}={}", dt.as_millis()));
        }
        log::debug!("decode stages {basename}{line} total={total_ms}");
    });
}

/// 基准直读当前线程累计（ms，不清空；须在 load_image 返回后同线程调用）。
/// 仅测试/基准代码引用，随构建剔除（同 `force`）。
#[cfg(test)]
pub(crate) fn snapshot() -> Vec<(&'static str, f64)> {
    STAGES.with(|s| {
        s.borrow()
            .iter()
            .map(|(n, d)| (*n, d.as_secs_f64() * 1000.0))
            .collect()
    })
}
