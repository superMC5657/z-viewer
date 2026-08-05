import { defineConfig } from "vite";

// https://vitejs.dev/config/
export default defineConfig(async () => ({
  // Vite options tailored for Tauri development.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // Tell vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
  build: {
    // 产物目标：WebView2（Chromium 系）支持现代语法，无需为旧浏览器降级
    // 保留 ES2021（与 tsconfig 一致），让 Vite 不做多余转译，产物更小
    target: "es2021",
    // 单页应用无多路由，单 chunk 即可；CSS 独立文件
    cssCodeSplit: false,
    // 压缩：构建产物 minify（esbuild，快且小）
    minify: "esbuild",
    // 资源内联阈值：小于 4KB 的资源内联为 data URL，减少请求
    assetsInlineLimit: 4096,
    // 产物结构：hash 文件名，利于缓存
    chunkSizeWarningLimit: 600,
    rollupOptions: {
      output: {
        // 手动分包：把 @tauri-apps/api 单独拆出（体积稳定，利于缓存）
        manualChunks: {
          "tauri-api": ["@tauri-apps/api"],
        },
      },
    },
    // 预构建优化：依赖预打包用 esbuild，与 dev 一致（避免 build/dev 行为差异）
    sourcemap: false,
  },
}));
