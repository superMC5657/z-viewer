# 05. viewer.ts 加载簿记收敛为 LoadContext

Status: resolved
Source: code review 2026-10-24, Standards 轴 #6(Duplicated Code + Data Clump)
Severity: P3
Blocked by: 03(先落 03 避免 presenter.ts/viewer.ts 冲突)

## 问题

`src/viewer.ts:157-165` 与 `203-219` 重复 seq/prevOwned/revokePrev/restorePrev 簿记;seq/prevOwned/revokePrev/restorePrev 4 个参数一路穿入 `showEnvelopeInternal`(:235)。

## 修改建议

1. 抽一个小类型(如 `LoadSlot { seq, prevOwned, revokePrev, restorePrev }`),两处簿记收敛到一个构造/更新函数。
2. `showEnvelopeInternal` 改收单个 `LoadSlot` 参数。
3. 纯重构不改行为;重点回归:快速连续翻页时旧位图被正确 revoke(内存不涨)、动画↔静态切换、seq 竞态丢弃旧帧。

## 验收

- 重复簿记块消失,tsc/`pnpm build` 全绿。
- 手验:连续快翻 50 张内存平稳;动画与静态来回切换正常;拖动缩放不闪旧帧。

## Comments

- 2026-10-24 已实施：新增 `LoadSlot` 类型与 `beginLoad()` 单一开局簿记（stopAnimation + settlePending + seq + 归属权），show/showEnvelope 的重复簿记消除；`showEnvelopeInternal` 的 4 参数收敛为 1 个 slot（体内解构，行为零变化）。tsc + vite build 全绿；建议按本节手验清单回归一次。
