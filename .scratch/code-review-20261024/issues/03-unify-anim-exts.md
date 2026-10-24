# 03. 动画扩展名表统一为单一真源

Status: resolved
Source: code review 2026-10-24, Standards 轴 #4(Duplicated Code)
Severity: P2

## 问题

`src/presenter.ts:39` `ANIM_EXTS = new Set(["gif","png","webp"])` 与 `src-tauri/src/decode/format.rs:21` `ANIM_EXTS: &[&str]` 人肉双份。仓库已有先例:RAW 扩展名由后端 `get_raw_extensions`(`src-tauri/src/commands/mod.rs:400`)作为真源下发。后端新增动画格式时,前端帧条将静默失效。

## 修改建议

- 首选:仿照 `get_raw_extensions`,通过现有 IPC 通道把动画扩展名表下发给前端(可并入返回 settings/context 的命令,避免新增命令);前端启动时拉取并缓存,删除 `presenter.ts:39` 的本地常量。
- 次选(若不想动 IPC):两处各加互指注释,并在 Rust 侧测试中断言前端文件文本包含同一张表(防漂移的最小闸门)。

## 验收

- 前端无硬编码动画集合(或存在同步测试闸门)。
- 在 format.rs 假想新增一种动画格式时,前端行为无需改代码即正确(推演确认)。
- `cargo test` + `pnpm build` 全绿。

## Comments

- 2026-10-24 已实施：后端新增 `get_anim_extensions` 命令（真源 decode::format::ANIM_EXTS，main.rs 已注册）；前端 types.ts 新增 `setAnimExts`/`isPotentialAnim`（镜像 setRawExts 模式，保留启动兜底集），presenter.ts 删除本地 ANIM_EXTS 常量。cargo test + pnpm build 全绿。
