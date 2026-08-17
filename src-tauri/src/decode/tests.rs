//! decode 模块测试（#[cfg(test)] 独立文件，不混入产品代码）

use super::*;

/// 测试图库相对路径（从 src-tauri 工作目录运行）
fn test_img(rel: &str) -> String {
    std::env::current_dir()
        .unwrap()
        .join("..")
        .join(rel)
        .to_string_lossy()
        .to_string()
}

#[test]
fn gif_animation_frames() {
    let result = load_image(&test_img("test-images/B/img_2.gif"), false).unwrap();
    assert_eq!(result.mode, "animated", "两帧 GIF 应识别为动画");
    assert!(
        result.frame_sizes.len() >= 2,
        "GIF 至少 2 帧，实际 {}",
        result.frame_sizes.len()
    );
    assert_eq!(
        result.frame_delays.len(),
        result.frame_sizes.len(),
        "延迟与帧一一对应"
    );
    let total: u32 = result.frame_sizes.iter().sum();
    assert_eq!(total, result.bytes.len() as u32, "帧长总和应等于 payload 长度");
    for d in &result.frame_delays {
        assert!(*d > 0, "帧延迟应 > 0，实际 {d}");
    }
    // 测试生成的 GIF 每帧 400ms
    assert_eq!(result.frame_delays[0], 400);
    // 每帧切片应是合法 PNG 字节
    let mut off = 0usize;
    for size in &result.frame_sizes {
        let slice = &result.bytes[off..off + *size as usize];
        assert!(
            slice.starts_with(&[0x89, b'P', b'N', b'G']),
            "帧应为 PNG 字节"
        );
        off += *size as usize;
    }
}

#[test]
fn static_png_goes_asset() {
    let result = load_image(&test_img("test-images/A/1.png"), false).unwrap();
    assert_eq!(result.mode, "asset");
    assert!(result.bytes.is_empty());
    assert!(result.frame_sizes.is_empty());
}

#[test]
fn raw_ext_detection() {
    assert!(is_raw_ext("CR2"));
    assert!(is_raw_ext("nef"));
    assert!(is_raw_ext("DNG"));
    assert!(!is_raw_ext("jpg"));
    assert!(!is_raw_ext("png"));
}

#[test]
fn sub_millisecond_delay_clamped_to_1ms() {
    // 5/10ms 整数除法截断为 0：应兜底为 1ms（否则前端 setTimeout(0) 全速疯转）
    let buffer = image::RgbaImage::new(1, 1);
    let delay = image::Delay::from_numer_denom_ms(5, 10);
    let frame = image::Frame::from_parts(buffer, 0, 0, delay);
    let (_, delay_ms) = animation::encode_frame(frame, 1).unwrap();
    assert_eq!(delay_ms, 1);
}

#[test]
fn envelope_roundtrip() {
    let result = LoadResult::animated(vec![vec![1, 2, 3], vec![4, 5]], vec![100, 200]);
    let buf = pack_envelope(&result);
    assert_eq!(&buf[..4], ENVELOPE_MAGIC, "信封魔数");
    let hlen = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
    let header: serde_json::Value = serde_json::from_slice(&buf[8..8 + hlen]).unwrap();
    assert_eq!(header["mode"], "animated");
    assert_eq!(header["frame_sizes"], serde_json::json!([3, 2]));
    assert_eq!(&buf[8 + hlen..], &result.bytes[..], "payload 原样透传");
}

#[test]
fn jpeg_sof_dimensions_parse() {
    // 构造最小合法 JPEG：SOI + SOF0(高2宽3) + EOI
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x02, 0x00, 0x03];
    jpeg.extend_from_slice(&[0x03, 0x01, 0x11, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01]);
    jpeg.push(0xFF);
    jpeg.push(0xD9);
    let (w, h) = preview::jpeg_dimensions(&jpeg).expect("应解析出尺寸");
    assert_eq!((w, h), (3, 2));
}

#[test]
fn preview_tiff_walk_finds_embedded_jpeg() {
    // 合成小 TIFF（little-endian）：IFD0 含 JpgFromRaw(0x002E) → 内嵌 JPEG
    // 结构：header(8) + IFD0（1 条目 + next 指针）+ JPEG 数据
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x04, 0x00, 0x05];
    jpeg.extend_from_slice(&[0x03, 0x01, 0x11, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01]);
    jpeg.push(0xFF);
    jpeg.push(0xD9);
    let ifd_off = 8u32;
    let jpeg_off = ifd_off + 2 + 12 * 1 + 4;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"II*\x00");
    bytes.extend_from_slice(&ifd_off.to_le_bytes());
    // IFD0：1 条目（JpgFromRaw：tag=0x002E, type=LONG, count=len, value=offset）
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&0x002Eu16.to_le_bytes());
    bytes.extend_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(&(jpeg.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&jpeg_off.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes()); // next IFD = 0
    bytes.extend_from_slice(&jpeg);
    let dir = std::env::temp_dir().join("iv_preview_test.tif");
    std::fs::write(&dir, &bytes).unwrap();
    let p = preview::extract_preview(&dir.to_string_lossy()).expect("应提取到内嵌 JPEG");
    assert_eq!((p.width, p.height), (5, 4));
    assert!(p.jpeg.starts_with(&[0xFF, 0xD8]));
    std::fs::remove_file(&dir).ok();
}
