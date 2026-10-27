# 01. 修复深放大真像素升级被丢弃（放大糊的主根因）

Status: resolved
Type: task
Priority: P0
Source: spec `.scratch/perf-flip-zoom/spec.md` 症状②；票 07（pm-followup）实现缺陷

## 问题

`src/presenter.ts:359-362` `maybeFetchNative`：

```ts
if (!header.capped) {
  this.nativeLoaded = true; // 原图本就不超过显示帽，无需升级
  return;
}
```

`original=true` 请求走 `load_image_original`（cap = ZOOM_MAX_DIM = 8192），
`capped=false` 意为「原图 ≤ 8192」。该分支前置条件是 `currentCapped=true`
（显示位图已按 3840 帽降采样），所以早退只在 **3840 < 原图长边 ≤ 8192** 可达
——24MP 6000×4000 等主流素材全部命中：后端递来的全分辨率位图被丢弃，
放大永远停在 3840 Bilinear 插值上。这正是用户「放大比 JpegView 糊」的主因。

第二处：`src/presenter.ts:345` 阈值 `if (this.viewer.currentScale <= 1) return;`
为严格大于——actual 模式（1:1）恰好 currentScale=1，永不触发深放大取数，
「实际大小」显示的是 3840 插值图的 1:1。

## 方案

1. **丢弃分支改为升级**：`original=true` 的返回就是 ≤8192 的最高保真版本，
   且该路径仅在 `currentCapped=true`（源 >3840）时可达——直接
   `showUpgrade(header, payload)`，删掉 `!header.capped` 早退；防御性保留
   「返回宽度 ≤ 当前显示宽度则跳过」的兜底（理论上不可达）。同步修正注释。
2. **阈值 `> 1` 改 `>= 1`**：actual 模式（恰好 1:1）也触发取数；fit 态对
   >3840 源 currentScale=baseScale<1 天然不触发，无误升级风险。
3. **补 TS 测试基建 + 回归测试**：仓库当前无前端测试（CI `pnpm run --if-present
   test` 空转）。引入 vitest（与 vite 同生态，CI 已预留 test/test:coverage 挂点）；
   把「是否升级」判定抽为纯函数（如 `shouldUpgradeToNative(header, currentCapped,
   currentScale)`）后单测覆盖三区间：≤3840（currentCapped=false 不触发）、
   3841–8192（必须升级）、>8192（升级到 8192 帽）。回归测试先红后绿。
4. 真实手验：>3840 源（24MP JXL/TIFF/RAW）放大至 1:1 以上，与 asset 通道大图
   对比细节；actual 模式同验。

## 验收

- vitest 基建落地，`pnpm test` 进 CI 真实执行；
- 新增单测锁住三区间判定（修复前红、修复后绿）；
- `tsc`、`cargo test` 全绿；`maybeFetchNative` 无行为级重排（代次守卫、
  nativePending/nativeFailed 语义不变）；
- 修正 `presenter.ts:360` 与 spec 引用的过时注释。

## Comments

- 2026-10-26 建档。诊断依据：代码路径追踪 + `decode/tests.rs:437`
  `original_decode_bypasses_display_cap`（后端契约恰好证明被丢弃的是真像素）+
  票 07 手验项正是本场景。
- 2026-10-27 实施完成（agent）。
  - **改动**：判定收口纯函数 `src/native-upgrade.ts shouldUpgradeToNative(header,
    currentCapped, currentScale, currentWidth)`，三区间语义单点维护；presenter
    预检传 `header=null`，取回后传完整 header 复核。BUG-A：删除 `!header.capped`
    丢弃分支——original=true 的返回即 ≤8192 帽的最高保真版本，仅保留「返回宽度
    ≤ 当前显示宽度」防御兜底（记 nativeLoaded，理论不可达）。BUG-B：触发阈值
    `>1` 改 `>=1`（`currentScale < 1` 才跳过），actual 恰好 1:1 也取数。presenter
    的 isPreview 早退保留在纯函数调用之前（失败可重试语义不变）。
  - **测试基建**：vitest 5.0.5（devDep）+ `pnpm test`（CI `--if-present` 挂点
    自动生效）；`src/native-upgrade.test.ts` 8 例：三区间 + 阈值边界 + isPreview/
    无增益防御。
  - **先红后绿证据**：纯函数按旧（错位）语义抽取并接线后跑测试 → `2 failed |
    6 passed`（正是 BUG-A 3841–8192 区间与 BUG-B currentScale=1 两例）；修复后
    `8 passed`；`tsc` 0 error；`pnpm build` 通过（测试文件不进产物）。
  - **Rust 侧验收**：`cargo test`（默认 feature）因本机缺 cmake 不可用（jpegxl-src
    build script panic，复确认）；以 `cargo test --no-default-features` 全绿
    （95 passed）代验——本票为纯前端改动，Rust 未触碰。默认构建待 cmake 可用后
    CI 复核。
  - **遗留**：真实手验（>3840 源 JXL/TIFF/RAW 放大至 1:1 与 asset 通道大图对比
    细节、actual 模式同验）留发版手验，与票 07 场景合并执行。
