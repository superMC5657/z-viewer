//! 图片格式统一分类器与扩展名白名单（单一事实来源）
//!
//! 所有模块（decode、browse、analytics、prefetch、commands）对图片扩展名、
//! 格式分类、动图判定的判断统一收口于此。新增格式只需在此文件扩展。

use std::path::Path;

/// 相机 RAW 扩展名（rawler 覆盖主流机型）
pub const RAW_EXTS: &[&str] = &[
    "cr2", "cr3", "nef", "arw", "dng", "orf", "rw2", "pef", "srw", "raf", "raw", "x3f", "erf",
    "3fr", "kdc", "dcr", "mrw", "mef", "mos", "iiq", "fff", "ari",
];

/// TIFF 扩展名（image crate 解码的静态通道）
pub const TIFF_EXTS: &[&str] = &["tif", "tiff"];

/// 潜在动画格式（默认原生 <img> 播放；帧控制按需拆帧）
pub const ANIM_EXTS: &[&str] = &["gif", "png", "webp"];

/// 浏览器原生解码的 asset 扩展名（前端 PrefetchPool 预热，Rust 预取跳过）。
/// 含动画格式：默认由 <img> 原生播放，帧控制按需拆帧（不进 Rust 预取）。
pub const ASSET_EXTS: &[&str] = &[
    "jpg", "jpeg", "bmp", "ico", "svg", "avif", "gif", "png", "webp",
];

/// 图片格式分类
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormatCategory {
    /// 浏览器原生解码通道（img 标签/asset 协议直读，含单帧 gif/png/webp 等）
    Asset,
    /// 相机 RAW 格式（全量通过 rawler 解码，或提取嵌入预览）
    Raw,
    /// 静态高位深格式（TIFF，通过 image crate 解码）
    Tiff,
    /// 未知 / 非受支持格式
    Unknown,
}

impl ImageFormatCategory {
    /// 是否为支持的图片格式
    #[inline]
    pub fn is_image(&self) -> bool {
        !matches!(self, Self::Unknown)
    }
}

/// 小写扩展名（Path::extension 语义：无扩展名文件返回空串，
/// 不像 `rsplit('.')` 会把整个文件名当扩展名）
pub fn ext_of<P: AsRef<Path>>(path: P) -> String {
    path.as_ref()
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// 按扩展名分类格式
pub fn classify_ext(ext: &str) -> ImageFormatCategory {
    let ext_lower = ext.to_ascii_lowercase();
    let ext_str = ext_lower.as_str();
    if ASSET_EXTS.contains(&ext_str) {
        ImageFormatCategory::Asset
    } else if TIFF_EXTS.contains(&ext_str) {
        ImageFormatCategory::Tiff
    } else if is_raw_ext(ext) {
        ImageFormatCategory::Raw
    } else {
        ImageFormatCategory::Unknown
    }
}

/// 按路径分类格式
pub fn classify<P: AsRef<Path>>(path: P) -> ImageFormatCategory {
    classify_ext(&ext_of(path))
}

/// 判定扩展名是否属于相机 RAW 格式
pub fn is_raw_ext(ext: &str) -> bool {
    RAW_EXTS.iter().any(|r| r.eq_ignore_ascii_case(ext))
}

/// 判定路径是否为浏览器原生解码通道格式（含动图格式）
pub fn is_asset<P: AsRef<Path>>(path: P) -> bool {
    matches!(classify(path), ImageFormatCategory::Asset)
}

/// 兼容既有签名：判定路径是否为 asset 格式
pub fn is_asset_ext(path: &str) -> bool {
    is_asset(path)
}

/// 判定路径是否为支持的常规图片文件
pub fn is_image_file<P: AsRef<Path>>(path: P) -> bool {
    classify(path).is_image()
}

/// 判定扩展名是否为潜在动画格式（GIF/WebP/PNG）
pub fn is_potential_anim(ext: &str) -> bool {
    ANIM_EXTS.iter().any(|a| a.eq_ignore_ascii_case(ext))
}

/// 轻量判定是否多帧动画（不拆帧解码）：
/// GIF 按魔数、WebP 扫 RIFF ANMF 块、PNG 扫 acTL 块；非动画格式/读取失败返回 false
pub fn is_animated(path: &str) -> bool {
    use std::io::{Read, Seek};
    let ext = ext_of(path);
    if !is_potential_anim(&ext) {
        return false;
    }
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    match ext.as_str() {
        "gif" => {
            let mut magic = [0u8; 6];
            f.read_exact(&mut magic).is_ok() && (&magic == b"GIF87a" || &magic == b"GIF89a")
        }
        "webp" => {
            let mut hdr = [0u8; 12];
            if f.read_exact(&mut hdr).is_err() || &hdr[0..4] != b"RIFF" || &hdr[8..12] != b"WEBP" {
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
                if f.seek(std::io::SeekFrom::Current((size + (size & 1)) as i64))
                    .is_err()
                {
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
                if f.seek(std::io::SeekFrom::Current((len + 4) as i64))
                    .is_err()
                {
                    break;
                }
            }
            false
        }
        _ => false,
    }
}
