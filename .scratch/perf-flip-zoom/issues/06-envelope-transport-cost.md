# 06. 决策：RGBA 信封传输链降载

Status: ready-for-human
Type: grilling
Priority: P2
Source: spec `.scratch/perf-flip-zoom/spec.md` 症状① R4

## 决策点

IPC 通道静态图交付为原始 RGBA（3840×2160×4 ≈ 33MB/张）。从 Rust 到上屏要过
5 次大内存拷贝：`pack_envelope` 拼 payload → 缓存命中 `(*envelope).clone()` →
WebView2 IPC 传输 → `new ImageData` → `createImageBitmap` + `drawImage`。
票 02 显示命中路径（含一次 33MB clone + IPC）也只要 10–13ms——说明此链当前
**不是**主要矛盾；但 RGBA 相比 JPEG 交付的内存占用（LRU 8 条 ≈ 264MB）与
弱机传输峰值值得复核。

## 选项

- **A. 维持 RGBA**：现值 10–13ms 命中、无损、免二次编解码；不动。
- **B. 改 JPEG 交付**：33MB → 3–8MB，LRU 内存占用降一个量级；代价是编码
  （~30–80ms，加在冷翻）+ 有损 + 解码回读。当年从 JPEG 切 RGBA 正是为免
  二次有损（`decode/mod.rs` 注释），回切需产品侧确认画质立场。
- **C. 半尺寸交付**：显示帽本来就 3840，对 ≤2K 显示设备可再降（如 2560），
  深放大路径已有真像素升级兜底；省内存省传输，代价是高分屏 fit 态锐度。

## 建议

维持 A，但把「issue 03 分阶段计时 + LRU 内存实测」作为复核输入：
若弱机上 IPC/位图转换段占比 >15% 或内存成为投诉点再复议 B/C。

## 验收

- 裁决写入 `## Answer`；若选 B/C 另立实施票 + ADR（画质立场变更需留档）。

## Comments

- 2026-10-26 建档。
