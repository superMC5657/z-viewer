# 08. 代码卫生批(四小项)

Status: resolved
Source: code review 2026-10-24, Standards 轴 #9(次要杂项)
Severity: P3

## 问题与建议(逐项)

1. **魔法串** — `LoadResult.mode` / `NavResult.boundary` 为字符串魔法值(比对点:`src-tauri/src/decode/decode_cache.rs:150`、`src-tauri/src/cache_policy.rs:22`、`src/presenter.ts:30`)。注意:提交 f912515 已做过 mode 联合类型,动手前先核对现状,只补缺口(倾向:TS 侧联合类型 + Rust 侧 `&'static str` 常量,两测对齐)。
2. **重复 matches!** — `src-tauri/src/license.rs:398,417` 吊销错误码判断重复两份 → 提取 `fn is_revocation_code(code: &str) -> bool`。
3. **绕过封装** — `src-tauri/src/commands/mod.rs:394` `get_settings` 直抓锁 → 改走 `SettingsState::read`。
4. **注释失真** — `src-tauri/src/license.rs:731` `status()` 注释称「单快照」实为两次加锁 → 倾向改注释;若该函数确需一致快照,改为单次加锁一次读。

## 验收

- 逐项 grep 确认;`cargo test` + tsc 全绿;mode/boundary 的 TS 联合类型与 Rust 常量取值一致(加注释互指)。

## Comments

- 2026-10-24 已实施：① TS 侧新增 `BoundaryKind`/`LoadMode` 联合类型，BOUNDARY_TEXT 收紧为 `Record<BoundaryKind, string>`；Rust 侧新增 `decode::load_mode` 常量模块，cache_policy.rs / decode_cache.rs 比对点改引常量（boundary 的 Rust 侧本就是 enum + as_str 单点产出，无需另加常量）。② license.rs 新增 `is_revocation_error()` 消除两处重复 matches!。③ get_settings 改走 `SettingsState::read`（锁中毒回退 Default，查询命令不值得 panic）。④ status() 注释改为如实描述「相邻两次快照」及不为此加锁的理由。cargo test + tsc 全绿。
