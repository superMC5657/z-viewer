/**
 * 应用更新服务模块（tauri-plugin-updater）
 * 检查新版本、下载并展示进度 Toast、安装与重启
 */

import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { feLog } from "../logger";
import type { UI } from "../ui";

export async function checkForUpdate(ui: UI): Promise<void> {
  ui.showToast("正在检查更新…");
  let update;
  try {
    update = await check();
  } catch (err) {
    ui.showToast(`检查更新失败：${String(err)}`);
    feLog(`检查更新失败: ${String(err)}`);
    return;
  }
  if (!update) {
    ui.showToast("已是最新版本");
    feLog("检查更新: 已是最新版本");
    return;
  }
  feLog(`发现新版本: v${update.currentVersion} → v${update.version}`);
  ui.showToast(`发现新版本 v${update.version}，开始下载…`);
  try {
    let total = 0;
    let downloaded = 0;
    let lastPct = 0;
    await update.download((event) => {
      if (event.event === "Started") {
        total = event.data.contentLength ?? 0;
        downloaded = 0;
        lastPct = 0;
        feLog(`下载开始: 共 ${total} 字节`);
      } else if (event.event === "Progress") {
        downloaded += event.data.chunkLength;
        // 进度 Toast（每 20% 刷新一次，避免高频刷屏）
        if (total > 0) {
          const pct = Math.floor((downloaded / total) * 100);
          if (pct - lastPct >= 20) {
            lastPct = pct;
            ui.showToast(`正在下载更新 ${pct}%…`);
          }
        }
      } else if (event.event === "Finished") {
        ui.showToast("下载完成，正在安装…");
      }
    });
    await update.install();
  } catch (err) {
    ui.showToast(`更新失败：${String(err)}`);
    feLog(`更新失败: ${String(err)}`);
    return;
  }
  feLog("更新安装完成，重启应用");
  ui.showToast("更新完成，正在重启…");
  try {
    await relaunch();
  } catch (err) {
    ui.showToast(`重启失败，请手动重启应用：${String(err)}`);
    feLog(`重启失败: ${String(err)}`);
  }
}
