/**
 * 幻灯片播放器（《需求报告与技术方案.md》2.4 + 《UI设计草图.md》3.6）
 *
 * - 从当前图片开始自动轮播，遵循跨文件夹无缝衔接（next_image）
 * - 间隔可调：2s / 5s / 10s
 * - 播放到全局最后一张自动停止并回调 onEnd（前端弹「已播放完所有图片」Toast）
 * - 手动翻页不中断播放，但重置计时（从当前图重新计间隔）
 *
 * 计时实现（v2 重写，独立计时器）：window.setTimeout 剩余时间式调度（deadline-aware）。
 * - 等待时间是一个独立的计时器：到点自动触发 onAdvance，与上一跳是否完成无关，
 *   图片加载耗时不会被计入间隔，也不会阻塞下一次触发
 * - 每拍触发后按「剩余时间 = 间隔 − 距上一拍已过时间」排定下一拍；
 *   调整间隔立即生效——距上一拍已超过新间隔时剩余 0，立刻触发（改 2s 马上有反应）
 * - 上一跳仍在进行（advancing）时跳过本拍（不排队、不堆积），且不移动计时基准，
 *   加载完成后立即补上，节奏稳定为设定间隔（不会因慢加载翻倍）
 * - 弃用 rAF + performance.now() 差值判断：rAF 只在渲染帧回调时检查，且旧的
 *   「完成后重新计时」语义把加载耗时算进间隔，实际节奏会慢于设定间隔
 */

export class Slideshow {
  /** 定时器句柄（null = 未播放） */
  private timerId: number | null = null;
  private intervalMs = 5000;
  private running = false;
  /** 正在执行 onAdvance（防重入：上一跳未完成时跳过本拍） */
  private advancing = false;
  /** 上一拍触发时刻（performance.now）：剩余时间 = intervalMs - (now - lastTickAt) */
  private lastTickAt = 0;

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
    this.lastTickAt = performance.now();
    this.onStateChange?.(true);
    this.schedule();
  }

  /** 停止播放 */
  stop(): void {
    if (!this.running) return;
    this.running = false;
    this.clearTimer();
    this.onStateChange?.(false);
  }

  toggle(): void {
    if (this.running) this.stop();
    else this.start();
  }

  /** 设置间隔（毫秒），播放中立即生效：
   *  按新间隔重算剩余时间（schedule 内部处理）；距上一拍已超过新间隔 → 剩余 0 → 立刻触发下一拍 */
  setInterval(ms: number): void {
    this.intervalMs = ms;
    if (this.running) this.schedule();
  }

  get interval(): number {
    return this.intervalMs;
  }

  /** 手动翻页/跳转后重置计时（不中断播放）：从当前时刻起算下一间隔 */
  resetTimer(): void {
    if (!this.running) return;
    this.lastTickAt = performance.now();
    this.schedule();
  }

  private clearTimer(): void {
    if (this.timerId !== null) {
      window.clearTimeout(this.timerId);
      this.timerId = null;
    }
  }

  /** 按剩余时间排定下一拍：remaining = intervalMs - 距上一拍已过时间，已超时则为 0（立即触发）。
   *  到点触发后自动排下一拍；timer 独立运行，与上一跳是否完成无关（不被加载阻塞） */
  private schedule(): void {
    this.clearTimer();
    if (!this.running) return;
    const elapsed = performance.now() - this.lastTickAt;
    const remaining = Math.max(0, this.intervalMs - elapsed);
    this.timerId = window.setTimeout(() => {
      this.timerId = null;
      this.tick();
      if (this.running) this.schedule();
    }, remaining);
  }

  /** 每拍触发：上一跳未完成则跳过本拍，但**不更新计时基准**——
   *  否则基准后移会让节奏翻倍（慢加载跨过下一拍后要再等满一个间隔）。
   *  跳过时 lastTickAt 保持原值，下一拍剩余时间很短，加载完成后立即补上，节奏稳定为 interval */
  private tick(): void {
    if (!this.running) return;
    if (this.advancing) return;
    this.lastTickAt = performance.now();
    this.advancing = true;
    void this.advance().finally(() => {
      this.advancing = false;
    });
  }

  private async advance(): Promise<void> {
    if (!this.running) return;
    if (!this.onAdvance) {
      this.stop();
      return;
    }
    let keepGoing = true;
    try {
      keepGoing = await this.onAdvance();
    } catch (err) {
      // 单跳失败不杀死播放：记录并继续下一跳（容错）
      console.error("幻灯片跳转失败:", err);
      keepGoing = true;
    }
    if (!keepGoing) {
      // 撞到全局最后一张：停止（main.ts 负责 Toast）
      this.stop();
    }
  }
}
