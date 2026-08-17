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

mod animation;
mod preview;
mod raw;
mod tiff;
#[cfg(test)]
mod tests;

/// 相机 RAW 扩展名（rawler 覆盖主流机型；供 browse.rs 浏览枚举复用）
pub const RAW_EXTS: &[&str] = &[
    "cr2", "cr3", "nef", "arw", "dng", "orf", "rw2", "pef", "srw", "raf", "raw", "x3f", "erf",
    "3fr", "kdc", "dcr", "mrw", "mef", "mos", "iiq", "fff", "ari",
];

/// TIFF 扩展名（image crate 解码的静态通道）
pub const TIFF_EXTS: &[&str] = &["tif", "tiff"];

/// 潜在动画格式（默认原生 <img> 播放；帧控制按需拆帧）
const ANIM_EXTS: &[&str] = &["gif", "png", "webp"];

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
pub fn load_image(path: &str, full: bool) -> Result<LoadResult, String> {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();

    if is_raw_ext(&ext) {
        return raw::decode_raw(path, full);
    }

    if TIFF_EXTS.contains(&ext.as_str()) {
        return tiff::decode_static(path);
    }

    if ANIM_EXTS.contains(&ext.as_str()) {
        if let Some(frames) = animation::decode_animation(path)? {
            return Ok(frames);
        }
        // 单帧：降级为静态，走 asset 协议直读
    }

    Ok(LoadResult::asset())
}

pub fn is_raw_ext(ext: &str) -> bool {
    RAW_EXTS.iter().any(|r| r.eq_ignore_ascii_case(ext))
}

/// 浏览器原生解码的 asset 扩展名（前端 PrefetchPool 预热，Rust 预取跳过）。
/// 含动画格式：默认由 <img> 原生播放，帧控制按需拆帧（不进 Rust 预取）。
pub fn is_asset_ext(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "bmp" | "ico" | "svg" | "avif" | "gif" | "png" | "webp"
    )
}

/// 轻量判定是否多帧动画（不拆帧解码）：
/// GIF 按魔数、WebP 扫 RIFF ANMF 块、PNG 扫 acTL 块；非动画格式/读取失败返回 false
pub fn is_animated(path: &str) -> bool {
    use std::io::{Read, Seek};
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    match ext.as_str() {
        "gif" => {
            let mut magic = [0u8; 6];
            f.read_exact(&mut magic).is_ok()
                && (&magic == b"GIF87a" || &magic == b"GIF89a")
        }
        "webp" => {
            let mut hdr = [0u8; 12];
            if f.read_exact(&mut hdr).is_err()
                || &hdr[0..4] != b"RIFF"
                || &hdr[8..12] != b"WEBP"
            {
                return false;
            }
            let mut buf = [0u8; 8];
            for _ in 0..64 {
                if f.read_exact(&mut buf).is_err() {
                    break;
                }
                if &buf[0..4] == b"ANMF" {
                    return true;
                }
                let size = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]) as u64;
                // 块数据 + 奇数长度对齐字节
                if f.seek(std::io::SeekFrom::Current((size + (size & 1)) as i64)).is_err() {
                    break;
                }
            }
            false
        }
        "png" => {
            let mut sig = [0u8; 8];
            if f.read_exact(&mut sig).is_err()
                || sig != [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
            {
                return false;
            }
            let mut buf = [0u8; 8];
            for _ in 0..128 {
                if f.read_exact(&mut buf).is_err() {
                    break;
                }
                let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as u64;
                match &buf[4..8] {
                    b"acTL" => return true,
                    b"IEND" | b"IDAT" => break, // IDAT 之后不会再出现 acTL
                    _ => {}
                }
                if f.seek(std::io::SeekFrom::Current((len + 4) as i64)).is_err() {
                    break;
                }
            }
            false
        }
        _ => false,
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

/// 长边降采样上限（屏幕 2 倍尺寸足够，控制传输体积；raw/tiff 共用）
pub(super) const MAX_DIM: u32 = 2560;

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
