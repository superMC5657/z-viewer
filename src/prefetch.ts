/**
 * 前端预解码池（方案四：WebView2 层面预解码 DOM）
 *
 * asset 通道图片（jpg/bmp/ico/svg 等浏览器原生解码）提前用隐藏 <img> 加载，
 * 让 WebView2 预解码缓存；切图时 convertFileSrc 同一 URL 命中缓存瞬时显示。
 * 隐藏 img 不挂载 DOM（无布局/渲染开销），仅占解码缓存。
 */

import { convertFileSrc } from "@tauri-apps/api/core";

const MAX_POOL = 10;

export class PrefetchPool {
  private imgs: HTMLImageElement[] = [];
  private loading = new Set<string>();

  /** 预热一组 asset 路径（仅原生解码的，跳过需 IPC 的） */
  warm(paths: string[], needsIpc: (p: string) => boolean): void {
    for (const p of paths) {
      if (this.loading.has(p)) continue;
      if (needsIpc(p)) continue; // 动画/RAW 走 Rust 缓存，不在此预热
      const url = convertFileSrc(p);
      if (this.imgs.some((i) => i.dataset.src === url)) continue;
      const img = new Image();
      img.dataset.src = url;
      img.dataset.path = p;
      img.decoding = "async";
      this.loading.add(p);
      img.onload = () => this.loading.delete(p);
      img.onerror = () => this.loading.delete(p);
      img.src = url;
      this.imgs.push(img);
      this.trim();
    }
  }

  /** 裁剪池（保留最近 MAX_POOL 个），被裁剪的加载中路径同步释放登记 */
  private trim(): void {
    if (this.imgs.length <= MAX_POOL) return;
    const overflow = this.imgs.length - MAX_POOL;
    for (let i = 0; i < overflow; i++) {
      const img = this.imgs.shift()!;
      const path = img.dataset.path;
      if (path) this.loading.delete(path);
      img.src = "";
      img.removeAttribute("src");
    }
  }

  /** 清空池（打开新文件夹/切图时可选调用，释放解码缓存） */
  clear(): void {
    for (const img of this.imgs) {
      img.src = "";
      img.removeAttribute("src");
    }
    this.imgs = [];
    this.loading.clear();
  }
}

