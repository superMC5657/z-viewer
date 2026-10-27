//! 基准素材生成（#[cfg(test)]）：vendored libjxl 编码器现场产出 JXL 码流。
//! 测试与基准共用同一实现，避免编码样板复制。

#[cfg(feature = "jxl-libjxl")]
use super::jxl::libjxl_ffi as ffi;

/// RGBA8 → JXL 码流（effort 3 + distance 1.0：基准只关心解码速度，编码够快即可）。
#[cfg(feature = "jxl-libjxl")]
pub(crate) fn encode_jxl_fixture(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    unsafe {
        let enc = ffi::JxlEncoderCreate(std::ptr::null());
        assert!(!enc.is_null());
        let mut info = ffi::JxlBasicInfo::zeroed();
        info.xsize = w;
        info.ysize = h;
        info.bits_per_sample = 8;
        info.exponent_bits_per_sample = 0;
        info.orientation = ffi::JXL_ORIENT_IDENTITY;
        info.num_color_channels = 3;
        // 素材为 4 通道 RGBA，BasicInfo 须同步声明 alpha 通道
        info.num_extra_channels = 1;
        info.alpha_bits = 8;
        info.alpha_exponent_bits = 0;
        info.alpha_premultiplied = 0;
        info.uses_original_profile = 0;
        assert_eq!(
            ffi::JxlEncoderSetBasicInfo(enc, &info),
            ffi::JXL_ENC_SUCCESS
        );
        let color = ffi::srgb_color_encoding();
        assert_eq!(
            ffi::JxlEncoderSetColorEncoding(enc, &color),
            ffi::JXL_ENC_SUCCESS
        );
        let fs = ffi::JxlEncoderFrameSettingsCreate(enc, std::ptr::null());
        assert!(!fs.is_null());
        assert_eq!(
            ffi::JxlEncoderFrameSettingsSetOption(fs, ffi::JXL_ENC_FRAME_SETTING_EFFORT, 3),
            ffi::JXL_ENC_SUCCESS
        );
        assert_eq!(
            ffi::JxlEncoderSetFrameDistance(fs, 1.0),
            ffi::JXL_ENC_SUCCESS
        );
        let fmt = ffi::JxlPixelFormat {
            num_channels: 4,
            data_type: ffi::JXL_TYPE_UINT8,
            endianness: ffi::JXL_NATIVE_ENDIAN,
            align: 0,
        };
        assert_eq!(
            ffi::JxlEncoderAddImageFrame(fs, &fmt, rgba.as_ptr(), rgba.len()),
            ffi::JXL_ENC_SUCCESS
        );
        ffi::JxlEncoderCloseInput(enc);

        let mut out = vec![0u8; 1 << 20];
        let mut next = out.as_mut_ptr();
        let mut avail = out.len();
        loop {
            match ffi::JxlEncoderProcessOutput(enc, &mut next, &mut avail) {
                ffi::JXL_ENC_SUCCESS => break,
                ffi::JXL_ENC_NEED_MORE_OUTPUT => {
                    let used = out.len() - avail;
                    out.resize(out.len() * 2, 0);
                    next = out.as_mut_ptr().add(used);
                    avail = out.len() - used;
                }
                st => panic!("JXL 编码失败 status={st}"),
            }
        }
        ffi::JxlEncoderDestroy(enc);
        out.truncate(out.len() - avail);
        out
    }
}
