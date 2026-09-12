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

/// 提取内嵌 JPEG 预览（best-effort；支持 TIFF/CR2/NEF/ARW/DNG、富士 RAF 及佳能 CR3/ISOBMFF）
pub(super) fn extract_preview(path: &str) -> Option<Preview> {
    let mut f = File::open(path).ok()?;
    let mut hdr = [0u8; 16];
    let n = f.read(&mut hdr).ok()?;
    if n < 8 {
        return None;
    }

    // 1. 富士 RAF 格式（前 16 字节为 "FUJIFILMCCD-RAW "）
    if n >= 16 && &hdr[..16] == b"FUJIFILMCCD-RAW " {
        return extract_raf_preview(&mut f);
    }

    // 2. 佳能 CR3 / ISOBMFF 格式（[size][ftyp][crx ]...）
    if n >= 12 && &hdr[4..8] == b"ftyp" {
        return extract_isobmff_preview(&mut f);
    }

    // 3. 标准 TIFF 结构（CR2/NEF/ARW/DNG 等）
    extract_tiff_preview(&mut f, &hdr[..8])
}

/// 富士 RAF 内嵌 JPEG 提取（头偏移 84 处存偏移，88 处存长度，微秒级读取）
fn extract_raf_preview(f: &mut File) -> Option<Preview> {
    let file_len = f.metadata().ok()?.len();
    if file_len < 120 {
        return None;
    }
    f.seek(SeekFrom::Start(84)).ok()?;
    let mut info = [0u8; 8];
    f.read_exact(&mut info).ok()?;
    let off = u32::from_be_bytes([info[0], info[1], info[2], info[3]]) as u64;
    let len = u32::from_be_bytes([info[4], info[5], info[6], info[7]]) as usize;
    if off == 0 || len < 4 || off + (len as u64) > file_len {
        return None;
    }
    f.seek(SeekFrom::Start(off)).ok()?;
    let mut buf = vec![0u8; len];
    f.read_exact(&mut buf).ok()?;
    if buf.len() >= 2 && buf[0] == 0xFF && buf[1] == 0xD8 {
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

/// 佳能 CR3 (ISOBMFF) 内嵌 JPEG 提取（解析 box 并提取 PRVW/uuid 中的预览）
fn extract_isobmff_preview(f: &mut File) -> Option<Preview> {
    let file_len = f.metadata().ok()?.len();
    let mut pos = 0u64;
    while pos + 8 <= file_len && pos < 32 * 1024 * 1024 {
        f.seek(SeekFrom::Start(pos)).ok()?;
        let mut hdr = [0u8; 8];
        if f.read_exact(&mut hdr).is_err() {
            break;
        }
        let size = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as u64;
        let box_type = &hdr[4..8];
        let box_size = if size == 1 {
            let mut ext = [0u8; 8];
            if f.read_exact(&mut ext).is_err() {
                break;
            }
            u64::from_be_bytes(ext)
        } else if size == 0 {
            file_len - pos
        } else {
            size
        };

        if box_size < 8 {
            break;
        }

        if box_type == b"uuid" || box_type == b"moov" || box_type == b"PRVW" {
            let read_len = (box_size.min(16 * 1024 * 1024)) as usize;
            let mut buf = vec![0u8; read_len];
            f.seek(SeekFrom::Start(pos)).ok()?;
            if f.read_exact(&mut buf).is_ok() {
                if let Some(soi) = find_jpeg_soi(&buf) {
                    let sub = &buf[soi..];
                    if let Some((w, h)) = jpeg_dimensions(sub) {
                        return Some(Preview {
                            jpeg: sub.to_vec(),
                            width: w,
                            height: h,
                        });
                    }
                }
            }
        }

        pos = pos.saturating_add(box_size);
    }
    None
}

fn find_jpeg_soi(buf: &[u8]) -> Option<usize> {
    if buf.len() < 3 {
        return None;
    }
    (0..buf.len() - 2).find(|&i| buf[i] == 0xFF && buf[i + 1] == 0xD8 && buf[i + 2] == 0xFF)
}

/// 标准 TIFF IFD 结构提取内嵌 JPEG
fn extract_tiff_preview(f: &mut File, hdr: &[u8]) -> Option<Preview> {
    let little = match &hdr[..4] {
        b"II*\x00" => true,
        b"MM\x00*" => false,
        _ => return None,
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
        for e in entries.as_chunks::<12>().0 {
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
