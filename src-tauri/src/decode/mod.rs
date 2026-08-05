//! 解码服务：RAW 解码（rawler）+ 动画拆帧（image crate）
//!
//! 图片加载通道（《需求报告与技术方案.md》8.3）：
//! - 常见格式 → 前端 asset 协议直读（零拷贝）
//! - RAW → Rust 解码 → 降采样 → JPEG 字节 → 前端 Blob URL
//! - 动画（GIF/APNG/动态 WebP）→ Rust 拆帧 → 帧 PNG + 每帧延迟 → 前端 canvas 逐帧控制
//!
//! 文件组织（可维护性拆分）：
//! - mod.rs：类型定义 + 统一入口 load_image + 扩展名判断
//! - raw.rs：RAW 解码通道（rawler）
//! - animation.rs：动画拆帧通道（GIF/APNG/WebP）
//! - tests.rs：测试（#[cfg(test)] 独立文件，不混入产品代码）

mod animation;
mod raw;
#[cfg(test)]
mod tests;

use base64::Engine;
use serde::Serialize;

/// 相机 RAW 扩展名（rawler 覆盖主流机型；供 browse.rs 浏览枚举复用）
pub const RAW_EXTS: &[&str] = &[
    "cr2", "cr3", "nef", "arw", "dng", "orf", "rw2", "pef", "srw", "raf", "raw", "x3f", "erf",
    "3fr", "kdc", "dcr", "mrw", "mef", "mos", "iiq", "fff", "ari",
];

/// 潜在动画格式（按帧数判定是否真的多帧）
const ANIM_EXTS: &[&str] = &["gif", "png", "webp"];

#[derive(Serialize, Clone)]
pub struct FrameData {
    /// base64 编码的 PNG 帧
    pub png: String,
    pub delay_ms: u32,
}

#[derive(Serialize, Clone)]
pub struct LoadResult {
    /// "asset"（前端 asset 协议直读）| "raw"（解码 JPEG）| "animated"（帧序列）
    pub mode: String,
    /// raw 模式：base64 编码的 JPEG 字节
    pub data: Option<String>,
    /// animated 模式：帧序列
    pub frames: Option<Vec<FrameData>>,
}

/// 统一入口：按扩展名与内容分发到三条加载通道
pub fn load_image(path: &str) -> Result<LoadResult, String> {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();

    if is_raw_ext(&ext) {
        return raw::decode_raw(path);
    }

    if ANIM_EXTS.contains(&ext.as_str()) {
        if let Some(frames) = animation::decode_animation(path)? {
            return Ok(LoadResult {
                mode: "animated".into(),
                data: None,
                frames: Some(frames),
            });
        }
        // 单帧：降级为静态，走 asset 协议直读
    }

    Ok(LoadResult {
        mode: "asset".into(),
        data: None,
        frames: None,
    })
}

pub fn is_raw_ext(ext: &str) -> bool {
    RAW_EXTS.iter().any(|r| r.eq_ignore_ascii_case(ext))
}

/// 浏览器原生解码的 asset 扩展名（前端 PrefetchPool 预热，Rust 预取跳过）
pub fn is_asset_ext(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "jpg" | "jpeg" | "bmp" | "ico" | "svg")
}

/// base64 编码（raw/动画通道共用）
pub(super) fn b64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}
