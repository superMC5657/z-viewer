# 04. 澄清 require_pro 恒真闭包语义

Status: resolved
Source: code review 2026-10-24, Standards 轴 #5(Mysterious Name)
Severity: P3

## 问题

`src-tauri/src/commands/mod.rs:176,294,420` 三处 `require_pro(license.is_pro(), false, || true)` 恒等于 `license.is_pro()`,闭包形式遮蔽了「这只是个布尔判断」的语义。

## 修改建议

- 阅读 `require_pro` 签名确认闭包参数的用途(推测是「pro 时才执行的取值逻辑」)。对只需要布尔的调用点:新增 `fn is_pro_available(license: &License) -> bool` 或直接内联 `license.is_pro()`,删除恒真闭包调用。
- 保留至少一个真正使用闭包的调用点作为样例;若无任何调用点真正用到闭包,考虑简化 `require_pro` 签名本身。

## 验收

- `grep -n "|| true" src-tauri/src/commands/mod.rs` 无 require_pro 相关命中。
- `cargo check` / `cargo test` 全绿。

## Comments

- 2026-10-24 已实施：三处恒真闭包（open_path / load_image / get_context）内联为 `license.is_pro()`；jump_folder 与 set_cache_level 的真闭包调用保留，`require_pro` 门控收口语义不变。cargo test 全绿。
