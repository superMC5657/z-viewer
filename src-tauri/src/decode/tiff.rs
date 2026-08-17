//! TIFF 静态解码通道：image crate 解码 → 方向回正 → 降采样 → JPEG 字节
//! （Chromium 不解码 TIFF，走 Rust 通道复用「静态 JPEG」展示路径）

use std::io::Cursor;

use super::{cap_dimensions, LoadResult};

pub(super) fn decode_static(path: &str) -> Result<LoadResult, String> {
    let img = image::ImageReader::open(path)
        .map_err(|e| format!("TIFF 打开失败: {e}"))?
        .decode()
        .map_err(|e| format!("TIFF 解码失败: {e}"))?;
    let orientation = super::preview::read_orientation(path).unwrap_or(1);
    let img = cap_dimensions(super::preview::apply_orientation(img, orientation));
    let (w, h) = (img.width(), img.height());
    let img = img.to_rgb8(); // 16-bit/CMYK 统一归一为 8-bit RGB

    let mut buf: Vec<u8> = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Jpeg)
        .map_err(|e| format!("JPEG 编码失败: {e}"))?;

    Ok(LoadResult::jpeg("static", buf, w, h, false))
}
