# 07. 移除生产环境调试全局 __lastAdvance

Status: resolved
Source: code review 2026-10-24, Standards 轴 #8(调试残留)
Severity: P2

## 问题

`src/main.ts:314` `(window as unknown as Record<string, unknown>).__lastAdvance = result;` 幻灯片每拍写全局,供 CDP 读取,随生产发布。

## 修改建议

直接删除;若 CDP 调试仍需要,用 `if (import.meta.env.DEV)` gate 住并加一行注释说明用途。

## 验收

- `grep -n "__lastAdvance" src/` 无命中(或仅 DEV gate 分支内)。
- 幻灯片播放回归正常。

## Comments

- 2026-10-24 已实施：删除 main.ts 的 `(window).__lastAdvance` 调试探针及其注释（无其他引用点）。tsc 全绿。
