# 06. ensureAnimation 清理路径去重

Status: resolved
Source: code review 2026-10-24, Standards 轴 #7(Duplicated Code)
Severity: P3
Blocked by: 03(同文件冲突规避)

## 问题

`src/presenter.ts:433-522` `ensureAnimation` 约 90 行内三处重复「关位图+关解码器+return」清理块(467/482/495)。

## 修改建议

提取局部闭包(如 `const abortWith = (reason) => { /* 关位图 + 关解码器 */ }`),三处早退改为调用它;或重排为单一 guard 链使清理只出现一次。不改对外行为。

## 验收

- 清理逻辑单一路径;动画→静态→动画切换手验正常;动画播完后帧条显隐正常。
- tsc / `pnpm build` 全绿。

## Comments

- 2026-10-24 已实施：ensureAnimation 抽取 `dispose()` 单一清理路径，四处退出（frameCount≤1 / 循环中代次过期 / 循环后代次过期 / catch 降级）共用；成功路径仍只关解码器（位图所有权移交 Viewer）。tsc 全绿。
