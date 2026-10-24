# 07. 深放大真像素:放大越过位图分辨率时按需解码原图

Status: resolved
Source: 产品决策(用户拍板「放大时要拿到真实的数据」);票 02/06 的 3840 显示帽残余代价
Type: task

## 问题

Rust 通道(TIFF/JXL/RAW)显示位图封顶 3840——放大越过位图 1:1 后是插值,
看不到原始像素;asset 通道(JPEG 等)本就持有原文件不受影响。

## 设计决策

- **触发**:显示倍率越过当前位图 1:1(currentScale > 1)且后端 envelope 标记
  `capped=true`(交付位图确实由更大原图降采样);粘性升级——升级后不回退,
  切图/重开即释放,避免 3840↔原图抖动换手。
- **上限 8192**(ZOOM_MAX_DIM):GPU 纹理安全线;超过的全景图仍插值显示,
  单图 tile 化属后续立项。
- **免费/专业版均可用**:这是解码能力而非缓存,符合 ADR 0002/0003 分层。
- **原图不写 LRU**:单张可达数百 MB,182MB×8 不可接受;每次按需现解。
- **视图稳定**:Viewer.showUpgrade 按新旧尺寸比回正总倍率(userScale 反比补偿,
  baseScale 重算),升级前后屏幕视图逐像素不变、仅更锐;RAW 预览→全量的既有
  升级路径同样受益(缩放中升级不再跳变)。
- **RAW 代价**:深放大触发完整 demosaic 重解(秒级),复用「解码中」指示器。

## 实现

- 后端:decode 各通道 cap 参数贯穿(TIFF/RAW finalize、JXL cap_rgba_frame 双引擎);
  `load_image_original`(ZOOM_MAX_DIM)+ `load_image` 命令 `original` 参数;
  envelope 新增 `capped` 标记;原图路径跳过缓存读写。
- 前端:presenter `maybeFetchNative`(onStateChange 驱动,代次守卫 + 在飞去重 +
  静默失败);viewer `showUpgrade` 倍率回正。

## 验收

- 后端单测:常规解码 capped 至 3840 且带标记,original 解码保持原始尺寸;
- cargo test 全绿、clippy --all-targets 干净、pnpm build(tsc)通过;
- 真实手验(安装包放大 TIFF/JXL/RAW 至 1:1 以上看细节)留人,记入发版手验清单。

## Comments

- 2026-10-24 建档并认领。
- 2026-10-24 完成：后端 cap 贯穿（TIFF/RAW finalize、JXL 双引擎 cap_rgba_frame）、
  `load_image_original` + 命令 `original` 参数、envelope `capped` 标记、原图路径
  跳过缓存；前端 `maybeFetchNative`（scale > 1 触发）+ `showUpgrade` 倍率回正
  （RAW 预览→全量升级顺带修复了缩放中的视图跳变）。单测
  `original_decode_bypasses_display_cap` 锁行为；tsc/clippy/test 全绿。
  真实手验记入发版手验清单 J 项。

## Answer

按「实现」节落地，全部设计决策维持。遗留：>8192 长边全景图的 tile 化
（单图分块按需解码）属后续立项；升级为粘性（不回退），切图释放。
