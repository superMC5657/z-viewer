/**
 * 前端统一日志（复用 @tauri-apps/plugin-log v2，不造轮子）
 *
 * - dev：attachLogger 把 Rust 日志回显到 devtools（echoGuard 防回声）；console 照常输出
 * - release：只写 LogDir（<productName>.log），零 Stdout/Webview 回显由后端保证
 * - viewer 量小：直接转发 console[log/info/warn/error] + window.onerror/unhandledrejection
 * - 本地运行为主，不自动上报
 */

import { attachLogger, debug, error, info, LogLevel, warn } from "@tauri-apps/plugin-log";

const GLOBAL_INIT_KEY = "__zlog_inited__";
/** 回声抑制：attachConsole 触发的 console 调用不再二次转发，避免 前端->后端->前端 循环 */
let echoGuard = false;

function fmtArgs(args: readonly unknown[]): string {
  return args
    .map((a) => {
      if (typeof a === "string") return a;
      try {
        return JSON.stringify(a) ?? String(a);
      } catch {
        return String(a);
      }
    })
    .join(" ");
}

function forward(
  method: "log" | "info" | "warn" | "error",
  send: (msg: string) => Promise<void>,
): void {
  const orig = console[method].bind(console);
  console[method] = (...args: unknown[]): void => {
    orig(...args);
    if (echoGuard) return;
    try {
      void send(`[FE] ${fmtArgs(args)}`).catch(() => undefined);
    } catch {
      /* 转发绝不影响业务 */
    }
  };
}

/** 初始化前端日志：在前端入口调用一次（幂等，防 Vite HMR 重复挂载）。 */
export async function initZLog(): Promise<void> {
  const g = window as unknown as Record<string, boolean | undefined>;
  if (g[GLOBAL_INIT_KEY]) return;
  g[GLOBAL_INIT_KEY] = true;

  forward("log", debug);
  forward("info", info);
  forward("warn", warn);
  forward("error", (msg: string) => error(msg));

  if (import.meta.env.DEV) {
    void debug(
      `[app] frontend initialized (DPI: ${window.devicePixelRatio}, Viewport: ${window.innerWidth}x${window.innerHeight})`,
    ).catch(() => undefined);
  }

  window.addEventListener("error", (e) => {
    try {
      void error(`[FE] window.onerror: ${e.message} @${e.filename}:${e.lineno}:${e.colno}`).catch(
        () => undefined,
      );
    } catch {
      /* 忽略 */
    }
  });
  window.addEventListener("unhandledrejection", (e) => {
    let reason: string;
    try {
      reason = e.reason instanceof Error ? (e.reason.stack ?? e.reason.message) : fmtArgs([e.reason]);
    } catch {
      reason = "<unserializable rejection>";
    }
    try {
      void error(`[FE] unhandledrejection: ${reason}`).catch(() => undefined);
    } catch {
      /* 忽略 */
    }
  });

  // DEV 才回显 Rust 日志到 devtools（release 无 Webview target，attach 无意义）。
  // 用 attachLogger 自行输出并全程 echoGuard：attachConsole 内部的 console 调用
  // 会命中上面的 forward 补丁，把 Rust 日志再转发回后端落盘（造成日志文件重复与回声污染）。
  if (import.meta.env.DEV) {
    try {
      const off = await attachLogger(({ level, message }) => {
        echoGuard = true;
        try {
          if (level === LogLevel.Trace) console.log(message);
          else if (level === LogLevel.Debug) console.debug(message);
          else if (level === LogLevel.Info) console.info(message);
          else if (level === LogLevel.Warn) console.warn(message);
          else console.error(message);
        } finally {
          echoGuard = false;
        }
      });
      void off;
    } catch {
      /* 后端日志插件不可用时静默降级 */
    }
  }
}

