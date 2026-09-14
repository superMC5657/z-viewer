# 统一日志（logging）

> 复用 `tauri-plugin-log` v2，不自造轮子，不引入 tracing/sentry，不自动上报。
> 日志只进 LogDir 文件，**禁止写入业务 SQLite**。
> 实现位置：`src-tauri/src/z_log.rs`（插件/脱敏/prune/panic/导出命令）、
> `src-tauri/src/logger.rs`（`dev_log!`）、`src/lib/z-log.ts`（前端转发）、
> `src/logger.ts`（`feLog`，仅 DEV 控制台输出）。

## 日志目录（各 OS，`identifier = com.zviewer.app`）

目录由 Tauri `app.path().app_log_dir()` 解析（`zlog_get_dir` 会先 `create_dir_all` + `prune`）：

| OS | 路径 |
| --- | --- |
| Windows | `%LOCALAPPDATA%\com.zviewer.app\logs` |
| macOS | `~/Library/Logs/com.zviewer.app`（注意：无末尾 `logs`） |
| Linux | `~/.local/share/com.zviewer.app/logs`（`XDG_DATA_HOME` 存在时以它为基） |

panic hook 拿不到 `AppHandle`，用 `resolve_log_dir_fallback()` 按同样规则兜底
（全失败时退到系统临时目录 `com.zviewer.app-logs`）。

## 目录里的文件

| 文件 | 来源 | 说明 |
| --- | --- | --- |
| `rust.log`（+轮转兄弟 `rust_*.log`） | Rust 侧（`target` 非 `webview` 开头） | 后端业务/框架日志 |
| `webview.log`（+轮转兄弟） | 前端经 `plugin:log\|log`（`target = webview…`） | 前端 `console.*` 转发、`window.onerror`、`unhandledrejection` |
| `crash-<ts>.txt` | panic hook | 版本/identifier/时间/线程/位置/payload/Backtrace，写盘前已脱敏 |
| `zviewer-logs-<ts>.zip` | `zlog_export_bundle` | 诊断包（见下），下次导出时不被重复打包（只收 `*.log` + `crash-*.txt`） |

单文件 1MB 轮转（`max_file_size`），策略 `KeepAll`（全部保留，靠 prune 封顶）；
`prune`：删 7 天前文件，总量超 20MB 时按 mtime 从旧到新删到预算内，
只处理 `*.log` 与 `crash-*.txt`，失败只记 `log` 不抛 panic。

## dev / release 矩阵

| | dev（`debug_assertions` / `vite dev`） | release |
| --- | --- | --- |
| Rust 门槛 | `Debug`（hyper/reqwest/wry/tao 压到 `Warn`） | `Info`（同上压到 `Warn`） |
| Stdout | 有 | 无（`clear_targets` 后只留 LogDir） |
| Webview 回显 | 有，但**只回显 Rust 日志**（`!starts_with(WEBVIEW_TARGET)`，前端日志不二次广播，无回声循环） | 无 |
| 前端回显 | `attachLogger` + `echoGuard` 把 Rust 日志打印到 devtools，回显输出不再二次转发 | 不 attach |
| 时区/格式 | 本地时区，`[HH:mm:ss.SSS][级别][target] <脱敏后消息>` | 同左 |

## 脱敏（`z_log::redact`，写盘/打包两处执行，无正则零依赖，绝不 panic）

1. 家目录 → `~`（`USERPROFILE` / `HOME`）。
2. 含 `@` 的连续 token → `[redacted-email]`。
3. `bearer <值>`（大小写不敏感，后跟空白才算）→ `bearer [redacted]`；
   `*token[=:] <值>`（含 `api_token`/`access_token`/`analyticsToken` 等前后缀，
   支持 JSON `"xxxToken": "值"` 形态）→ `[redacted]`。
4. 连续 `[A-Z0-9-]` 且长度 ≥16（含 `-` 或数字）→ `[redacted-key]`。

## 前端用法

```ts
// src/main.ts：启动第一行，失败不阻塞启动
import { initZLog } from "./lib/z-log";
void initZLog().catch(() => undefined);
```

- `initZLog()`（幂等）：劫持 `console[log/info/warn/error]` 转发为 `[FE] …`
  （`orig` 先输出，原生 console 行为不变；转发失败静默丢弃）；
  注册 `window.onerror` + `unhandledrejection`；仅 `import.meta.env.DEV` 时回显后端日志。
- `detachZLog()`：取消回显（测试清理用）。
- `feLog()`（`src/logger.ts`）：仅 DEV 控制台输出，不落盘；要落盘走 `console.*`。
- 后端 `dev_log!(…)`（`src-tauri/src/logger.rs`）：`macro_rules` 在调用点展开，
  内部转 `log::debug!("[BE] …")`，保留调用点文件行号；release 下被 `Info` 门槛过滤。

## 导出诊断包（供 UI 按钮接线）

- 后端命令（已在 `main.rs` 注册）：`zlog_get_dir`（返回日志目录，先 prune）、
  `zlog_export_bundle`（返回 zip 路径；zip 内 `*.log` + `crash-*.txt` + `version.txt`，文本二次脱敏）。
- 前端封装（`src/lib/z-log.ts`）：`getZLogDir(): Promise<string>` /
  `exportZLogBundle(): Promise<string>`。
- 所需 capability：`log:default`（`src-tauri/capabilities/default.json` 已配）。

## 已知限制

- `Cargo.toml [profile.release] panic = "abort"`：release 下 panic 直接 abort，
  panic hook（`crash-*.txt`）仅 dev/ unwind 构建有效。如需 release 崩溃报告，
  需另议 panic 策略（不在本文范围内，不动 release profile）。
