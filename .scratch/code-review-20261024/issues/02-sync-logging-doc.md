# 02. 修订 logging.md 失真条目

Status: resolved
Source: code review 2026-10-24, Standards 轴硬违规 #3
Severity: P2

## 问题

`docs/logging.md` 引用的 `src/window-state.ts` 已不存在(功能并入 `src/presence.ts`);「相关文件(行号索引)」的行号全部漂移(如 license.rs:238→实际 254,701/707/709→691/697/699)。

## 修改建议

1. `window-state.ts` 引用改为 `presence.ts`。
2. 行号索引整体换成**符号名索引**(如 `license.rs#LicenseState::status`),行号天生易腐,符号可被 grep 定位;若坚持行号,重新生成一遍并在文档头注明「行号以某 commit 为准」。
3. 同步 01 号 ticket 修复后的 prune/crash 行为到字段表与示例。

## 验收

- 文中引用的文件路径全部存在;索引抽查 3 处可定位到真实符号/行。

## Comments

- 2026-10-24 已实施：行号索引整体改为符号索引（license.rs/analytics.rs 按函数名定位），`window-state.ts` 死引用改为 `presence.ts`；新增维护原则「新增打点补符号不补行号」。crash 字段表已随 01 号票同步。
