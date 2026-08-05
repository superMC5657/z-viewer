/**
 * 幻灯片播放器（《需求报告与技术方案.md》2.4 + 《UI设计草图.md》3.6）
 *
 * - 从当前图片开始自动轮播，遵循跨文件夹无缝衔接（next_image）
 * - 间隔可调：2s / 5s / 10s
 * - 播放到全局最后一张自动停止并回调 onEnd（前端弹「已播放完所有图片」Toast）
 * - 手动翻页不中断播放，但重置计时（从当前图重新计间隔）
 */

export class Slideshow {
  private timer: number | undefined;
  private intervalMs = 5000;
  private running = false;

  /** 前进一帧：返回 false 表示撞到边界应停止（由 main.ts 提供） */
  onAdvance: (() => Promise<boolean>) | null = null;
  /** 播放状态变化（true=开始 false=停止），用于 UI 切换 */
  onStateChange: ((running: boolean) => void) | null = null;

  get isRunning(): boolean {
    return this.running;
  }

  /** 开始播放（从当前图片开始） */
  start(): void {
    if (this.running) return;
    this.running = true;
    this.onStateChange?.(true);
    this.scheduleNext();
  }

  /** 停止播放 */
  stop(): void {
    if (!this.running) return;
    this.running = false;
    window.clearTimeout(this.timer);
    this.onStateChange?.(false);
  }

  toggle(): void {
    if (this.running) this.stop();
    else this.start();
  }

  /** 设置间隔（毫秒），播放中立即生效 */
  setInterval(ms: number): void {
    this.intervalMs = ms;
    if (this.running) {
      // 重新计时，让新间隔立即生效
      window.clearTimeout(this.timer);
      this.scheduleNext();
    }
  }

  get interval(): number {
    return this.intervalMs;
  }

  /** 手动翻页/跳转后重置计时（不中断播放） */
  resetTimer(): void {
    if (!this.running) return;
    window.clearTimeout(this.timer);
    this.scheduleNext();
  }

  private scheduleNext(): void {
    window.clearTimeout(this.timer);
    this.timer = window.setTimeout(() => {
      void this.advance();
    }, this.intervalMs);
  }

  private async advance(): Promise<void> {
    if (!this.running) return;
    if (!this.onAdvance) {
      this.stop();
      return;
    }
    const keepGoing = await this.onAdvance();
    if (!keepGoing) {
      // 撞到全局最后一张：停止（main.ts 负责 Toast）
      this.stop();
      return;
    }
    if (this.running) this.scheduleNext();
  }
}
