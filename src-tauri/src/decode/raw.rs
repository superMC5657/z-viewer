//! RAW 解码通道：rawler 解码 → develop（demosaic + 白平衡 + 色彩校准 + sRGB）
//! → 降采样 → JPEG 字节 → 前端 Blob URL（经 IPC 二进制信封）

use std::io::Cursor;

use super::{cap_dimensions, LoadResult};

pub(super) fn decode_raw(path: &str) -> Result<LoadResult, String> {
    let rawimage = rawler::decode_file(path).map_err(|e| format!("RAW 解码失败: {e}"))?;
    let develop = rawler::imgop::develop::RawDevelop::default();
    let intermediate = develop
        .develop_intermediate(&rawimage)
        .map_err(|e| format!("RAW 处理失败: {e}"))?;
    let img = intermediate
        .to_dynamic_image()
        .ok_or_else(|| "RAW 图像转换失败".to_string())?;

    let img = cap_dimensions(img);
    let (w, h) = (img.width(), img.height());
    let img = img.to_rgb8();

    let mut buf: Vec<u8> = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Jpeg)
        .map_err(|e| format!("JPEG 编码失败: {e}"))?;

    Ok(LoadResult::jpeg("raw", buf, w, h, false))
}
