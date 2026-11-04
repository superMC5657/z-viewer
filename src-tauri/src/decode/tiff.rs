//! TIFF 静态解码通道：image crate 解码 → 方向回正 → 降采样 → 原始 RGBA8 像素
//! （Chromium 不解码 TIFF，走 Rust 通道原始像素直通，免二次 JPEG 有损压缩与编解码开销）

use std::io::{Cursor, Read};

use super::stage::name;
use super::{finalize_rgba, load_mode, stage, LoadResult};

pub(super) fn decode_static(path: &str, cap: u32) -> Result<LoadResult, String> {
    // EXIF 头在文件前部：open_with_orientation 读方向后已 seek 回开头（省一次 open）。
    // 正文整体读入内存归入 read 段：解码与读盘分段各自独立，流式交错会让
    // 36MB 级正文 IO 全落进 engine 段，分段数据把解码占比压到失真。代价是
    // 峰值内存多出一份文件字节（无压缩 TIFF 字节量 ≈ 像素数据量，可接受），
    // 且 engine 从内存解码免去逐块 syscall。
    let (mut file, orientation) = stage::timed(name::READ, || {
        super::preview::open_with_orientation(path).map_err(|e| format!("TIFF 打开失败: {e}"))
    })?;
    let bytes = stage::timed(name::READ, || -> Result<Vec<u8>, String> {
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)
            .map_err(|e| format!("TIFF 读取失败: {e}"))?;
        Ok(buf)
    })?;
    let img = stage::timed(name::ENGINE, || {
        image::ImageReader::new(Cursor::new(&bytes))
            .with_guessed_format()
            .map_err(|e| format!("TIFF 打开失败: {e}"))?
            .decode()
            .map_err(|e| format!("TIFF 解码失败: {e}"))
    })?;

    Ok(finalize_rgba(img, orientation, load_mode::STATIC, cap))
}
