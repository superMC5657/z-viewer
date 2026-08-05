//! RAW 解码通道：rawler 解码 → develop（demosaic + 白平衡 + 色彩校准 + sRGB）
//! → 降采样 → JPEG 编码 → base64

use std::io::Cursor;

use super::{b64, LoadResult};

/// RAW 解码后降采样最长边限制（屏幕 2 倍尺寸足够，控制传输体积）
const MAX_DIM: u32 = 2560;

pub(super) fn decode_raw(path: &str) -> Result<LoadResult, String> {
    let rawimage = rawler::decode_file(path).map_err(|e| format!("RAW 解码失败: {e}"))?;
    let develop = rawler::imgop::develop::RawDevelop::default();
    let intermediate = develop
        .develop_intermediate(&rawimage)
        .map_err(|e| format!("RAW 处理失败: {e}"))?;
    let img = intermediate
        .to_dynamic_image()
        .ok_or_else(|| "RAW 图像转换失败".to_string())?;

    let (w, h) = (img.width(), img.height());
    let img = if w.max(h) > MAX_DIM {
        let scale = MAX_DIM as f32 / w.max(h) as f32;
        img.resize(
            (w as f32 * scale).round() as u32,
            (h as f32 * scale).round() as u32,
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    let img = img.to_rgb8();

    let mut buf: Vec<u8> = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Jpeg)
        .map_err(|e| format!("JPEG 编码失败: {e}"))?;

    Ok(LoadResult {
        mode: "raw".into(),
        data: Some(b64(&buf)),
        frames: None,
    })
}
