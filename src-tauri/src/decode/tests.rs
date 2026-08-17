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
    let (_, delay_ms) = animation::encode_frame(frame).unwrap();
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
