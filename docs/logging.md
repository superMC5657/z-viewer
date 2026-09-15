# 统一日志（logging）

> 复用 `tauri-plugin-log` v2，不自动上报。日志只进 LogDir 文件，**禁止写入业务 SQLite**。
> 实现：`src-tauri/src/z_log.rs`（插件/脱敏/prune/panic/导出命令）、`src/lib/z-log.ts`（前端转发）。

## 日志目录（各 OS，`identifier = com.zviewer.app`）

目录由 `app.path().app_log_dir()` 解析（`zlog_get_dir` 会先 `create_dir_all` + `prune`）：

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

单文件 1MB 轮转（`max_file_size`），策略 `KeepAll`（靠 prune 封顶）；
`prune`：删 7 天前文件，总量超 20MB 时按 mtime 从旧到新删到预算内，
只处理 `*.log` 与 `crash-*.txt`，失败只记 `log` 不抛 panic。

## dev / release 矩阵

| | dev（`debug_assertions` / `vite dev`） | release |
| --- | --- | --- |
| Rust 门槛 | `Debug`（hyper/reqwest/wry/tao 压到 `Warn`） | `Info`（同上压到 `Warn`，`debug` 不落盘） |
| Stdout | 有 | 无（`clear_targets` 后只留 LogDir） |
| Webview 回显 | 有，但**只回显 Rust 日志**（前端日志不二次广播，无回声循环） | 无 |
| 前端回显 | `attachLogger` + `echoGuard` 把 Rust 日志打印到 devtools，回显输出不再二次转发 | 不 attach |
| 时区/格式 | 本地时区，`[HH:mm:ss.SSS][级别][target] <脱敏后消息>` | 同左 |

## 脱敏（`z_log::redact`，写盘前 format 内执行，无正则零依赖，绝不 panic）

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
- 后端直接用 `log::*`，无私有宏封装，调用点即日志点（文件行号准确）。
- 摘要辅助（`commands/mod.rs`，纯函数无日志）：`basename_of`（`Path::file_name`，
  无文件名时回退 `unknown`）、`first_line_of`（首行 + 200 字符截断，`chars()` 字符边界安全）；
  `commands/prefetch.rs` 复用同一实现。

## 级别语义

- `info`：生命周期（打开成功、激活/注销成功）
- `warn`：可恢复（打开失败、解码失败、激活失败、离线降级）
- `error`：失败（浏览模型打不开：`open_path` 直接返回前端错误）
- `debug`：细节（导航边界、预取失败、续验成功/推迟、埋点上报失败）

## 消息风格与红线

- 一律英文小写前缀模块词（`browse open` / `decode failed` / `license revoked` …），只记摘要。
- 路径只许 basename（`Path::file_name`）；原因取首行或截断封顶（`first_line_of` 200 字符）。
- 禁止：完整 args/全路径/激活码-email-JWT-deviceId 三件套值/服务端原文/全文 dump；
  `redact` 只是兜底，“不该记的值”不许进消息。
- 禁止 `println!/eprint!/dbg!`；前端业务不加打点（框架转发 + 全局错误已够）。

## 浏览（`main.rs` 启动路径 + `commands/mod.rs`）

重要才记：打开结果 + 打开失败 + 边界（debug）。

- `info` browse open ok {basename}（启动传参/双击打开成功，`main.rs`；`open_path` 成功同文案，
  两处为不同入口各记一次，不存在双记）
- `warn` browse open failed {basename}（启动路径打不开：非图片/空目录）
- `error` browse open failed {basename}（`open_path` 模型建立失败，直接返回前端错误）
- `debug` browse boundary {first-image|last-image|first-folder|last-folder}
  （静态枚举映射，高频且用户可见 toast，不 info 化；`navigate` 三导航命令共用。
  `pro-required` 是免费版 `jump_folder` 早返（`commands/mod.rs:228-230`）给前端的解锁引导，
  不经 `navigate`，无日志）
- `warn` browse jump failed: unknown target（静态文案，不记 target 原文；
  `Err` 回前端含 target，但不进日志）
- `debug` browse open skipped invalid startup arg（启动参数非法/不存在，`None` 静默跳过；
  静态文案，不记参数原文；`main.rs:62`；无参数正常启动不记）

## 解码（`commands/mod.rs` → `decode/*.rs`）

- `warn` decode failed {basename}: {原因首行}（`load_image` 实时路径：RAW/TIFF/动画全通道收敛于此；
  预取后台调用走 `cache prefetch failed` 的 debug，见下）
- 解码器内部（`raw.rs`/`tiff.rs`/`animation.rs`）零 log：底层错误（io/ImageError/rawler `Display`）
  均不含路径，只在命令层记一次。

## 缓存（`commands/prefetch.rs`）

- `debug` cache prefetch failed {basename}: {原因首行}（队列 B 上下文预取 + 队列 A 首图预取；
  低价值后台优化，release 不可见）
- LRU 本体（`cache/*.rs`）热路径零 log：锁失败/未命中是常态，不记。

## 授权（`license.rs`，只记事件不记值）

`activate/deactivate/verify_online` 的 `Err(String)` 载荷含服务端自由文本/reqwest 错误
（含 URL），一律不进消息；有疑问只记静态文案。激活码 / email / JWT / deviceId 一律不进日志。

- `info` license activated（激活成功）
- `warn` license activate failed（激活失败，静态文案）
- `warn` license activate token verify failed（本地验签不过；与自有静态文案精确相等才分流，
  服务端自由文本不参与分流）
- `debug` license verify ok（在线续验成功，release 不可见）
- `warn` license verify offline, using grace period（网络失败走离线宽限期）
- `warn` license revoked, credentials removed（吊销删凭证，3 处：HTTP 非 2xx 吊销码/body 错误码/valid=false）
- `debug` license verify deferred（其他服务端错误/响应解析异常，保留凭证等重验）
- `info` license deactivated（注销成功）/ `warn` license deactivate failed（注销失败，静态文案）
- `debug` license load invalid fallback to free（启动 `load()` 文件存在但解析/验签失败回退免费版；
  静态文案，不记路径不记内容；`license.rs:238`；文件不存在正常回退不记）

## 埋点（`analytics.rs`）

- `debug` analytics report failed ×3（client 构造失败/请求构造失败（非法 URL）/发送失败；
  静态文案，不记负载内容；未配置 apiBase/analyticsPath 时静默不上报，不记）
- `record_view` 高频计数点零 log；上报负载只含聚合统计（见 `analytics.rs` 隐私边界）。

## 导出诊断包（供 UI 按钮接线）

- 后端命令（已在 `main.rs` 注册）：`zlog_get_dir`（返回日志目录，先 prune）、
  `zlog_export_bundle`（返回 zip 路径；zip 内 `*.log` + `crash-*.txt` + `version.txt`，文本二次脱敏）。
- 前端封装（`src/lib/z-log.ts`）：`getZLogDir(): Promise<string>` /
  `exportZLogBundle(): Promise<string>`。
- 所需 capability：`log:default`（`src-tauri/capabilities/default.json` 已配）。

## 已知限制

- `Cargo.toml [profile.release] panic = "abort"`：release 下 panic 直接 abort，
  panic hook（`crash-*.txt`）仅 dev/unwind 构建有效。如需 release 崩溃报告，
  需另议 panic 策略（不在本文范围内，不动 release profile）。

## 相关文件（行号索引）

- `src-tauri/src/z_log.rs`（插件构建、prune、redact、panic hook、导出命令；不动）
- `src-tauri/src/main.rs`（启动参数跳过 debug：62；启动打开打点：128/135）
- `src-tauri/src/commands/mod.rs`（摘要辅助 + 打开/边界/跳转/解码打点：
  `basename_of`/`first_line_of` 定义、边界 134、打开失败 169/成功 172、
  未知跳转目标 237、解码失败 291/298）
- `src-tauri/src/commands/prefetch.rs`（预取失败 debug：40/116）
- `src-tauri/src/license.rs`（授权事件打点：启动加载回退 238、离线降级 402、吊销 416/433/439、
  推迟 421/425、续验成功 458、激活 701/707/709、注销 721/725）
- `src-tauri/src/analytics.rs`（上报失败 debug：138/145/148）
- `src-tauri/src/browse/*.rs`、`src-tauri/src/decode/*.rs`、`src-tauri/src/cache/*.rs`
  （零 log：错误上浮到命令层记一次，热路径不记）
- `src/lib/z-log.ts`、`src/main.ts`（前端接入；业务不加打点）
- `src-tauri/capabilities/default.json`（`log:default`）
