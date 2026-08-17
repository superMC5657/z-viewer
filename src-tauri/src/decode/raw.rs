//! RAW 解码通道：内嵌预览快显（毫秒级）→（后台）rawler develop 全量 → JPEG 字节
//!
//! rawler 0.7 的 develop 只解析 EXIF 方向（RawImage.orientation）但不应用，
//! 全量输出前按方向回正（preview::apply_orientation）。

use std::io::Cursor;

use rawler::Orientation;

use super::{cap_dimensions, LoadResult};

pub(super) fn decode_raw(path: &str, full: bool) -> Result<LoadResult, String> {
    // 非全量拍：优先返回内嵌 JPEG 预览；提取失败回退全量 develop
    if !full {
        if let Some(p) = super::preview::extract_preview(path) {
            return Ok(LoadResult::jpeg("raw", p.jpeg, p.width, p.height, true));
        }
    }

    let rawimage = rawler::decode_file(path).map_err(|e| format!("RAW 解码失败: {e}"))?;
    let orientation = orient_to_u8(rawimage.orientation);
    let develop = rawler::imgop::develop::RawDevelop::default();
    let intermediate = develop
        .develop_intermediate(&rawimage)
        .map_err(|e| format!("RAW 处理失败: {e}"))?;
    let img = intermediate
        .to_dynamic_image()
        .ok_or_else(|| "RAW 图像转换失败".to_string())?;

    let img = cap_dimensions(super::preview::apply_orientation(img, orientation));
    let (w, h) = (img.width(), img.height());
    let img = img.to_rgb8();

    let mut buf: Vec<u8> = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Jpeg)
        .map_err(|e| format!("JPEG 编码失败: {e}"))?;

    Ok(LoadResult::jpeg("raw", buf, w, h, false))
}

/// rawler Orientation → EXIF 1-8（Unknown 按 1 正常处理）
fn orient_to_u8(o: Orientation) -> u8 {
    use Orientation::*;
    match o {
        Normal => 1,
        HorizontalFlip => 2,
        Rotate180 => 3,
        VerticalFlip => 4,
        Transpose => 5,
        Rotate90 => 6,
        Transverse => 7,
        Rotate270 => 8,
        Unknown => 1,
    }
}
