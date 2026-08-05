/**
 * 幻灯片播放器（《需求报告与技术方案.md》2.4 + 《UI设计草图.md》3.6）
 *
 * - 从当前图片开始自动轮播，遵循跨文件夹无缝衔接（next_image）
 * - 间隔可调：2s / 5s / 10s
 * - 播放到全局最后一张自动停止并回调 onEnd（前端弹「已播放完所有图片」Toast）
 * - 手动翻页不中断播放，但重置计时（从当前图重新计间隔）
 *
 * 计时实现（v4，拍点固定推进的节拍器 + 欠拍补跳）：window.setTimeout + nextTickAt 原定时刻。
 * - 等待时间是一个独立的计时器：到点自动触发 onAdvance，与上一跳是否完成无关，
 *   图片加载耗时不会被计入间隔，也不会阻塞下一次触发
 * - 每拍触发后 nextTickAt += intervalMs（拍点固定推进），间隔严格 = 设定间隔；
 *   调整间隔/手动翻页时 nextTickAt 重置为 now + intervalMs，立即生效
 * - 上一跳仍在进行（advancing）时跳过本拍并记录欠拍（pendingAdvance），
 *   当前跳完成后由 finally 立即补跳 —— 慢加载时节奏退化为 max(intervalMs, 加载耗时)
 *   而非翻倍成 intervalMs 的整数倍（v3 的「跳过不补拍」在 2s 档 + 加载 ≥2s 时
 *   实际间隔变成 4s/6s，2s 档位形同虚设）
 * - 补跳由 advance 完成回调驱动而非 0ms 定时器，不产生 v2「剩余时间式调度」
 *   的 0ms busy loop；快加载（< intervalMs）时间隔仍严格 = 设定间隔
 */

import { feLog } from "./logger";

export class Slideshow {
  /** 定时器句柄（null = 未播放） */
  private timerId: number | null = null;
  private intervalMs = 2000;
  private running = false;
  /** 正在执行 onAdvance（防重入：上一跳未完成时跳过本拍） */
  private advancing = false;
  /** 欠拍标记：advancing 期间跳过的拍，当前跳完成后立即补上（防止慢加载把节奏翻倍） */
  private pendingAdvance = false;
  /** 下一拍原定触发时刻：拍点固定推进（nextTickAt += intervalMs），间隔严格 = intervalMs */
  private nextTickAt = 0;

  /** 前进一帧：返回 false 表示撞到边界应停止（由 main.ts 提供） */
  onAdvance: (() => Promise<boolean>) | null = null;
  /** 播放状态变化（true=开始 false=停止），用于 UI 切换 */
  onStateChange: ((running: boolean) => void) | null = null;

  get isRunning(): boolean {
    feLog(`幻灯片 isRunning: ${this.running}`);
    return this.running;
  }

  /** 开始播放（从当前图片开始） */
  start(): void {
    feLog(`幻灯片 start: running=${this.running}, 间隔=${this.intervalMs}ms`);
    if (this.running) return;
    this.running = true;
    this.pendingAdvance = false;
    this.nextTickAt = performance.now() + this.intervalMs;
    this.onStateChange?.(true);
    this.schedule();
  }

  /** 停止播放 */
  stop(): void {
    feLog(`幻灯片 stop: running=${this.running}`);
    if (!this.running) return;
    this.running = false;
    this.pendingAdvance = false;
    this.clearTimer();
    this.onStateChange?.(false);
  }

  toggle(): void {
    feLog(`幻灯片 toggle: running=${this.running}`);
    if (this.running) this.stop();
    else this.start();
  }

  /** 设置间隔（毫秒），播放中立即生效：从当前时刻起算新间隔，下一拍按新间隔触发 */
  setInterval(ms: number): void {
    feLog(`幻灯片 setInterval: ${ms}ms (原 ${this.intervalMs}ms)`);
    this.intervalMs = ms;
    this.pendingAdvance = false; // 重新锚定拍点：旧欠拍作废，下一拍按新间隔起算
    if (this.running) {
      this.nextTickAt = performance.now() + this.intervalMs;
      this.schedule();
    }
  }

  get interval(): number {
    feLog(`幻灯片 interval: ${this.intervalMs}ms`);
    return this.intervalMs;
  }

  /** 手动翻页/跳转后重置计时（不中断播放）：从当前时刻起算下一间隔 */
  resetTimer(): void {
    feLog(`幻灯片 resetTimer: running=${this.running}`);
    if (!this.running) return;
    this.pendingAdvance = false; // 手动翻页后重新锚定拍点，旧欠拍作废
    this.nextTickAt = performance.now() + this.intervalMs;
    this.schedule();
  }

  private clearTimer(): void {
    feLog(`幻灯片 clearTimer: timerId=${this.timerId}`);
    if (this.timerId !== null) {
      window.clearTimeout(this.timerId);
      this.timerId = null;
    }
  }

  /** 按「下一拍原定时刻」排定：remaining = nextTickAt - now，已过则为 0（立即触发）。
   *  到点触发后自动排下一拍；timer 独立运行，与上一跳是否完成无关（不被加载阻塞） */
  private schedule(): void {
    this.clearTimer();
    if (!this.running) return;
    const remaining = Math.max(0, this.nextTickAt - performance.now());
    feLog(`幻灯片 schedule: ${remaining.toFixed(0)}ms 后触发下一拍`);
    this.timerId = window.setTimeout(() => {
      this.timerId = null;
      this.tick();
      if (this.running) this.schedule();
    }, remaining);
  }

  /** 每拍触发：拍点固定推进一个间隔（跳过拍也推进，绝不产生 0ms busy loop）。
   *  上一跳未完成（advancing）时跳过本拍但记录欠拍，当前跳完成后由 finally 立即补跳：
   *  慢加载时节奏 = max(intervalMs, 加载耗时)，不会翻倍成 intervalMs 的整数倍 */
  private tick(): void {
    feLog(`幻灯片 tick: running=${this.running}, advancing=${this.advancing}, pending=${this.pendingAdvance}`);
    if (!this.running) return;
    this.nextTickAt += this.intervalMs;
    if (this.advancing) {
      this.pendingAdvance = true;
      feLog("幻灯片 tick: 上一跳未完成，记录欠拍");
      return;
    }
    this.pendingAdvance = false;
    this.advancing = true;
    void this.advance().finally(() => {
      feLog(`幻灯片 tick finally: 跳转完成，pending=${this.pendingAdvance}`);
      this.advancing = false;
      if (this.pendingAdvance) {
        this.pendingAdvance = false;
        if (this.running) {
          // 欠拍补跳：remaining=0 → 立即触发下一拍（由完成回调驱动，非 0ms 定时器轮询）
          feLog("幻灯片 tick: 欠拍补跳");
          this.nextTickAt = performance.now();
          this.schedule();
        }
      }
    });
  }

  private async advance(): Promise<void> {
    feLog(`幻灯片 advance: running=${this.running}, 有 onAdvance=${this.onAdvance !== null}`);
    if (!this.running) return;
    if (!this.onAdvance) {
      feLog("幻灯片 advance: 无 onAdvance，停止");
      this.stop();
      return;
    }
    let keepGoing = true;
    try {
      keepGoing = await this.onAdvance();
      feLog(`幻灯片 advance: 完成，keepGoing=${keepGoing}`);
    } catch (err) {
      // 单跳失败不杀死播放：记录并继续下一跳（容错）
      feLog(`幻灯片 advance: 异常 ${String(err)}`);
      console.error("幻灯片跳转失败:", err);
      keepGoing = true;
    }
    if (!keepGoing) {
      // 撞到全局最后一张：停止（main.ts 负责 Toast）
      feLog("幻灯片 advance: keepGoing=false，停止播放");
      this.stop();
    }
  }
}
