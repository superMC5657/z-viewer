//! 解码服务：RAW 解码（rawler）+ 静态解码（image crate，TIFF）+ 动画拆帧
//!
//! 图片加载通道（《需求报告与技术方案.md》8.3）：
//! - 常见格式 → 前端 asset 协议直读（零拷贝，含浏览器原生播放的动画格式）
//! - RAW → 内嵌预览快显（毫秒级）→（后台）demosaic 全量 → JPEG 字节 → 前端 Blob URL
//! - TIFF → image crate 解码 → JPEG 字节 → 前端 Blob URL
//! - 动画（GIF/APNG/动态 WebP）→ 默认原生 <img> 播放；帧控制按需拆帧 → 帧 PNG 字节
//!
//! 所有 Rust 解码通道经 IPC 返回**原始字节**（二进制信封 pack_envelope，
//! 前端 parseLoadEnvelope 解析）—— 不再 base64，免 33% 体积膨胀与双端编解码。
//!
//! 文件组织（可维护性拆分）：
//! - mod.rs：类型定义 + 统一入口 load_image + 信封打包 + 扩展名判断
//! - raw.rs：RAW 解码通道（rawler + 内嵌预览快显 + 方向回正）
//! - preview.rs：EXIF 辅助（内嵌预览提取 / 方向读取 / 方向回正）
//! - tiff.rs：TIFF 静态解码通道（image crate）
//! - animation.rs：动画拆帧通道（GIF/APNG/WebP）
//! - tests.rs：测试（#[cfg(test)] 独立文件，不混入产品代码）

pub mod format;
mod animation;
mod preview;
mod raw;
#[cfg(test)]
mod tests;
mod tiff;

pub use format::*;
use crate::util::{basename_of, first_line_of};

/// IPC 二进制信封魔数（与前端 parseLoadEnvelope 对应）
pub const ENVELOPE_MAGIC: &[u8; 4] = b"IMGV";

/// 解码结果：所有 Rust 通道的产物，payload 为原始字节
#[derive(Clone)]
pub struct LoadResult {
    /// "asset"（前端 asset 协议直读）| "raw" | "static"(TIFF) | "animated"（帧序列）
    pub mode: String,
    /// payload 的 MIME（raw/static: image/jpeg；animated: image/png）
    pub mime: Option<String>,
    /// raw 通道内嵌预览位：true 表示 bytes 是内嵌预览，全量仍在后台解码
    pub is_preview: bool,
    /// 图像尺寸（raw/static 通道提供）
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// animated：每帧延迟（ms）
    pub frame_delays: Vec<u32>,
    /// animated：每帧 PNG 字节长（payload 按此切分）
    pub frame_sizes: Vec<u32>,
    /// payload：raw/static = 单张 JPEG；animated = 帧 PNG 拼接
    pub bytes: Vec<u8>,
}

impl LoadResult {
    /// asset 通道占位（单帧动画降级 / 无 Rust 解码需求）
    pub fn asset() -> Self {
        Self {
            mode: "asset".into(),
            mime: None,
            is_preview: false,
            width: None,
            height: None,
            frame_delays: Vec::new(),
            frame_sizes: Vec::new(),
            bytes: Vec::new(),
        }
    }

    /// 单张 JPEG 通道（raw/static）
    pub fn jpeg(mode: &str, bytes: Vec<u8>, w: u32, h: u32, is_preview: bool) -> Self {
        Self {
            mode: mode.into(),
            mime: Some("image/jpeg".into()),
            is_preview,
            width: Some(w),
            height: Some(h),
            frame_delays: Vec::new(),
            frame_sizes: Vec::new(),
            bytes,
        }
    }

    /// 原始 RGBA8 像素通道（TIFF/全量 RAW 极速直显，免二次 JPEG 有损压缩与解码损耗）
    pub fn rgba(mode: &str, bytes: Vec<u8>, w: u32, h: u32, is_preview: bool) -> Self {
        Self {
            mode: mode.into(),
            mime: Some("image/x-rgba".into()),
            is_preview,
            width: Some(w),
            height: Some(h),
            frame_delays: Vec::new(),
            frame_sizes: Vec::new(),
            bytes,
        }
    }

    /// 动画帧序列：帧 PNG 拼接为单段 payload，frame_sizes 供前端切分
    pub fn animated(frames: Vec<Vec<u8>>, delays: Vec<u32>) -> Self {
        let sizes = frames.iter().map(|f| f.len() as u32).collect();
        let mut bytes = Vec::new();
        for f in &frames {
            bytes.extend_from_slice(f);
        }
        Self {
            mode: "animated".into(),
            mime: Some("image/png".into()),
            is_preview: false,
            width: None,
            height: None,
            frame_delays: delays,
            frame_sizes: sizes,
            bytes,
        }
    }
}

/// 统一入口：按扩展名与内容分发到各通道
/// `full=false`：RAW 优先返回内嵌预览（快），全量由后续 full=true 请求完成
///
/// 通道失败在此记一次 warn（basename + 原因首行 + 通道后缀）：命令层 `load_image`
/// 的通用 `decode failed` 与此按通道后缀去重（`[raw]`/`[tiff]`/`[anim]`），只看
/// 通道维度时无需翻命令层日志。
pub fn load_image(path: &str, full: bool) -> Result<LoadResult, String> {
    let ext = ext_of(path);

    match classify_ext(&ext) {
        ImageFormatCategory::Raw => raw::decode_raw(path, full).inspect_err(|e| {
            log::warn!(
                "decode failed {} [raw]: {}",
                basename_of(path),
                first_line_of(e)
            );
        }),
        ImageFormatCategory::Tiff => tiff::decode_static(path).inspect_err(|e| {
            log::warn!(
                "decode failed {} [tiff]: {}",
                basename_of(path),
                first_line_of(e)
            );
        }),
        ImageFormatCategory::Asset => {
            if is_potential_anim(&ext) {
                let frames = animation::decode_animation(path).inspect_err(|e| {
                    log::warn!(
                        "decode failed {} [anim]: {}",
                        basename_of(path),
                        first_line_of(e)
                    );
                })?;
                if let Some(frames) = frames {
                    return Ok(frames);
                }
                // 单帧：降级为静态，走 asset 协议直读
            }
            Ok(LoadResult::asset())
        }
        ImageFormatCategory::Unknown => Ok(LoadResult::asset()),
    }
}

/// 打包为 IPC 二进制信封：[魔数][header_len LE][header JSON][payload]
/// 前端 parseLoadEnvelope 解析（src/types.ts）
pub fn pack_envelope(result: &LoadResult) -> Vec<u8> {
    let header = serde_json::to_vec(&serde_json::json!({
        "mode": result.mode,
        "mime": result.mime,
        "is_preview": result.is_preview,
        "width": result.width,
        "height": result.height,
        "frame_delays": result.frame_delays,
        "frame_sizes": result.frame_sizes,
    }))
    .unwrap_or_default();
    let mut buf = Vec::with_capacity(8 + header.len() + result.bytes.len());
    buf.extend_from_slice(ENVELOPE_MAGIC);
    buf.extend_from_slice(&(header.len() as u32).to_le_bytes());
    buf.extend_from_slice(&header);
    buf.extend_from_slice(&result.bytes);
    buf
}

/// 长边降采样上限（覆盖 4K 超清 3840×2160，保证高分屏 1:1 清晰呈现同时严格防爆显存）
pub(super) const MAX_DIM: u32 = 3840;

/// 解码通道收尾：先按 EXIF 方向回正，再超限降采样，最后输出原始 RGBA8
/// （raw/static 共用；mode 由调用方指定：raw="raw"、tiff="static"）
pub(super) fn finalize_rgba(img: image::DynamicImage, orientation: u8, mode: &str) -> LoadResult {
    let img = cap_dimensions(preview::apply_orientation(img, orientation));
    let (w, h) = (img.width(), img.height());
    let rgba = img.to_rgba8().into_raw();
    LoadResult::rgba(mode, rgba, w, h, false)
}

/// 超限降采样：长边 > MAX_DIM 时按比例缩到上限（Triangle 滤波）
pub(super) fn cap_dimensions(mut img: image::DynamicImage) -> image::DynamicImage {
    let (w, h) = (img.width(), img.height());
    if w.max(h) > MAX_DIM {
        let scale = MAX_DIM as f32 / w.max(h) as f32;
        img = img.resize(
            (w as f32 * scale).round() as u32,
            (h as f32 * scale).round() as u32,
            image::imageops::FilterType::Triangle,
        );
    }
    img
}
