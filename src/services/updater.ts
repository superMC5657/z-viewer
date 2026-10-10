/**
 * 应用更新服务模块（tauri-plugin-updater）
 * 检查新版本、下载并展示进度 Toast、安装与重启
 */

import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export async function checkForUpdate(notify: (msg: string) => void): Promise<void> {
  notify("正在检查更新…");
  console.info("[updater] checking for updates...");
  let update;
  try {
    update = await check();
  } catch (err) {
    console.warn(`[updater] check failed: ${String(err)}`);
    notify(`检查更新失败：${String(err)}`);
    return;
  }
  if (!update) {
    console.info("[updater] check complete: already up to date");
    notify("已是最新版本");
    return;
  }
  console.info(`[updater] update available: v${update.version}`);
  notify(`发现新版本 v${update.version}，开始下载…`);
  console.info(`[updater] downloading update v${update.version}...`);
  try {
    let total = 0;
    let downloaded = 0;
    let lastPct = 0;
    await update.download((event) => {
      if (event.event === "Started") {
        total = event.data.contentLength ?? 0;
        downloaded = 0;
        lastPct = 0;
      } else if (event.event === "Progress") {
        downloaded += event.data.chunkLength;
        // 进度 Toast（每 20% 刷新一次，避免高频刷屏）
        if (total > 0) {
          const pct = Math.floor((downloaded / total) * 100);
          if (pct - lastPct >= 20) {
            lastPct = pct;
            notify(`正在下载更新 ${pct}%…`);
          }
        }
      } else if (event.event === "Finished") {
        notify("下载完成，正在安装…");
      }
    });
  } catch (err) {
    console.error(`[updater] download failed: ${String(err)}`);
    notify(`更新失败：${String(err)}`);
    return;
  }
  console.info(`[updater] download complete v${update.version}, installing...`);
  try {
    await update.install();
  } catch (err) {
    console.error(`[updater] install failed: ${String(err)}`);
    notify(`更新失败：${String(err)}`);
    return;
  }
  console.info("[updater] update installed, restarting...");
  notify("更新完成，正在重启…");
  try {
    await relaunch();
  } catch (err) {
    console.error(`[updater] relaunch failed: ${String(err)}`);
    notify(`重启失败，请手动重启应用：${String(err)}`);
  }
}
