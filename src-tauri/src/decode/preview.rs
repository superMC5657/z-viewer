//! EXIF 元数据辅助：RAW 内嵌 JPEG 预览提取 + 图片方向（Orientation 0x0112）回正
//!
//! - extract_preview：best-effort 遍历 TIFF 结构取内嵌预览（CR2 JpgFromRaw /
//!   DNG PreviewImage / NEF·ARW 等 JPEGInterchangeFormat），FFD8 校验失败返回 None，
//!   调用方回退全量解码。rawler 无预览 API，各品牌 tag/偏移有差异，必须容错。
//! - read_orientation：kamadak-exif 读取（TIFF/JPEG/WebP 容器；GIF 无 EXIF 返回 None）
//! - apply_orientation：按 EXIF 1-8 旋转/翻转 DynamicImage
//!   （rawler develop 只解析方向不应用；拆帧重编码会丢方向标记，均须在此回正）

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};

use image::{imageops, DynamicImage};

/// 内嵌预览：JPEG 字节 + 尺寸
pub(super) struct Preview {
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

// 预览候选 tag（EXIF/TIFF 编号）
const TAG_JPG_OFFSET: u16 = 0x0201; // JPEGInterchangeFormat（NEF/ARW 等）
const TAG_JPG_LENGTH: u16 = 0x0202;
const TAG_JPG_FROM_RAW: u16 = 0x002E; // CR2
const TAG_PREVIEW_IMAGE: u16 = 0x0111; // DNG

/// 提取内嵌 JPEG 预览（best-effort；任何异常/校验失败返回 None）
pub(super) fn extract_preview(path: &str) -> Option<Preview> {
    let mut f = File::open(path).ok()?;
    let mut hdr = [0u8; 8];
    f.read_exact(&mut hdr).ok()?;
    let little = match &hdr[..4] {
        b"II*\x00" => true,
        b"MM\x00*" => false,
        _ => return None, // 非 TIFF 结构（如 RAF 定制头）——回退全量解码
    };
    let file_len = f.metadata().ok()?.len();
    let mut ifd_off = rd_u32(&hdr[4..8], little);
    let mut candidates: Vec<(u32, u32)> = Vec::new(); // (offset, length)
    let mut pending_offset: Option<u32> = None;
    for _ in 0..8 {
        if ifd_off == 0 || (ifd_off as u64) + 2 > file_len {
            break;
        }
        f.seek(SeekFrom::Start(ifd_off as u64)).ok()?;
        let mut cnt = [0u8; 2];
        f.read_exact(&mut cnt).ok()?;
        let n = rd_u16(&cnt, little) as usize;
        if n == 0 || n > 128 {
            break; // 防御：真实 IFD 条目数远小于此
        }
        let mut entries = vec![0u8; n * 12];
        f.read_exact(&mut entries).ok()?;
        for e in entries.chunks_exact(12) {
            let tag = rd_u16(&e[0..2], little);
            let count = rd_u32(&e[4..8], little);
            let val = rd_u32(&e[8..12], little);
            match tag {
                // 这两个 tag 的值字段即偏移、count 即字节长（大预览必然越界存储）
                TAG_JPG_FROM_RAW | TAG_PREVIEW_IMAGE => candidates.push((val, count)),
                TAG_JPG_OFFSET => pending_offset = Some(val),
                TAG_JPG_LENGTH => {
                    if let Some(off) = pending_offset.take() {
                        candidates.push((off, val));
                    }
                }
                _ => {}
            }
        }
        // 下一 IFD 指针
        let mut next = [0u8; 4];
        f.read_exact(&mut next).ok()?;
        ifd_off = rd_u32(&next, little);
    }
    for (off, len) in candidates {
        if off == 0 || len == 0 || len > 128 * 1024 * 1024 {
            continue;
        }
        let start = off as u64;
        if start >= file_len {
            continue;
        }
        let len = (len as u64).min(file_len - start) as usize;
        if len < 2 {
            continue;
        }
        f.seek(SeekFrom::Start(start)).ok()?;
        let mut soi = [0u8; 2];
        if f.read_exact(&mut soi).is_err() || soi != [0xFF, 0xD8] {
            continue; // 偏移基准不同/损坏 → 尝试下一候选
        }
        // SOI 已读出：剩余 len-2 字节（JPEG 总长含 SOI）
        let mut buf = vec![0u8; len];
        buf[0] = 0xFF;
        buf[1] = 0xD8;
        if f.read_exact(&mut buf[2..]).is_err() {
            continue;
        }
        if let Some((w, h)) = jpeg_dimensions(&buf) {
            return Some(Preview {
                jpeg: buf,
                width: w,
                height: h,
            });
        }
    }
    None
}

/// 读取 EXIF Orientation（1-8）；无 EXIF/解析失败返回 None。
/// 接收已打开的文件句柄：调用方（TIFF/动画解码）复用同一句柄，省一次 open。
pub(super) fn read_orientation(file: &std::fs::File) -> Option<u8> {
    let exif = exif::Reader::new()
        .read_from_container(&mut BufReader::new(file))
        .ok()?;
    let field = exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)?;
    match field.value {
        exif::Value::Short(ref v) => v.first().copied().map(|x| x as u8),
        _ => None,
    }
}

/// 按 EXIF Orientation（1-8）旋转/翻转回正；1/0/>8 原样返回
/// 旋转语义（EXIF 规范）：2=水平镜像 3=180° 4=垂直镜像 5=主对角线镜像
/// 6=顺时针90° 7=副对角线镜像 8=逆时针90°（image crate rotate90 为顺时针）
pub(super) fn apply_orientation(img: DynamicImage, o: u8) -> DynamicImage {
    use imageops::{flip_horizontal, rotate180, rotate270, rotate90};
    if !(2..=8).contains(&o) {
        return img;
    }
    let rgba = img.to_rgba8();
    let out = match o {
        2 => flip_horizontal(&rgba),
        3 => rotate180(&rgba),
        4 => imageops::flip_vertical(&rgba),
        5 => rotate270(&flip_horizontal(&rgba)),
        6 => rotate90(&rgba),
        7 => rotate90(&flip_horizontal(&rgba)),
        8 => rotate270(&rgba),
        _ => rgba,
    };
    DynamicImage::ImageRgba8(out)
}

/// 解析 JPEG 帧头（SOF 段）取宽高（高在前）；非 JPEG/解析失败返回 None
pub(super) fn jpeg_dimensions(buf: &[u8]) -> Option<(u32, u32)> {
    if buf.len() < 4 || buf[0] != 0xFF || buf[1] != 0xD8 {
        return None;
    }
    let mut i = 2;
    while i + 4 <= buf.len() {
        if buf[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = buf[i + 1];
        // 无长度字段的标记：SOI/D0-D7(表)/01
        if marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            i += 2;
            continue;
        }
        if i + 4 > buf.len() {
            return None;
        }
        let seg_len = ((buf[i + 2] as usize) << 8) | buf[i + 3] as usize;
        if seg_len < 2 {
            return None;
        }
        // SOF 标记：C0-C3、C5-C7、C9-CB、CD-CF（排除 DHT/DAC）
        if matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
            if i + 9 > buf.len() {
                return None;
            }
            let h = ((buf[i + 5] as u32) << 8) | buf[i + 6] as u32;
            let w = ((buf[i + 7] as u32) << 8) | buf[i + 8] as u32;
            return (w > 0 && h > 0).then_some((w, h));
        }
        i += 2 + seg_len;
    }
    None
}

fn rd_u16(b: &[u8], little: bool) -> u16 {
    if little {
        u16::from_le_bytes([b[0], b[1]])
    } else {
        u16::from_be_bytes([b[0], b[1]])
    }
}

fn rd_u32(b: &[u8], little: bool) -> u32 {
    if little {
        u32::from_le_bytes([b[0], b[1], b[2], b[3]])
    } else {
        u32::from_be_bytes([b[0], b[1], b[2], b[3]])
    }
}
