/**
 * 深放大真像素升级判定（票 01 抽取的纯函数）：是否用 original=true 请求取回的
 * 位图换掉当前显示位图。无 IO/DOM 依赖，单测锁三区间语义（src/native-upgrade.test.ts）。
 *
 * 三区间（以原图长边计，显示帽 3840 / 取数帽 8192）：
 * - ≤3840：currentCapped=false，显示位图已是最高保真，不触发；
 * - 3841–8192：original 返回 capped=false 的全分辨率真像素，必须升级；
 * - >8192：original 返回 capped=true 的 8192 帽降采样，升级到取数帽。
 */

import type { LoadEnvelope } from "./types";

/** 判定所需的最小信封切片；取数前 header 传 null（仅做预检） */
export type NativeUpgradeProbe = Pick<LoadEnvelope, "isPreview" | "capped" | "width">;

export function shouldUpgradeToNative(
  header: NativeUpgradeProbe | null,
  currentCapped: boolean,
  currentScale: number,
  currentWidth: number,
): boolean {
  if (!currentCapped) return false; // 源 ≤3840：显示位图已是最高保真
  if (currentScale < 1) return false; // 未达 1:1（fit 态对 >3840 源 baseScale<1 天然不触发）
  if (!header) return true; // 预检通过：发起 original=true 取数
  if (header.isPreview) return false; // 防御：异常路径仍返回预览
  // original=true 的返回即 ≤8192 帽的最高保真版本：capped=false 表示原图本就
  // ≤8192，返回的是全分辨率真像素（勿与显示帽 3840 的 capped 语义混淆），
  // 与 capped=true 的 8192 帽降采样同样升级。
  if (header.width !== null && header.width <= currentWidth) return false; // 防御：返回位图无增益
  return true;
}
