import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

// 桌面端 Tauri 期望固定端口；面板模式无影响
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [vue()],
  build: {
    target: "es2022",
    // 拆分大依赖到独立 chunk，避免单个文件过大导致加载慢
    rollupOptions: {
      output: {
        manualChunks: {
          // Vue 核心运行时
          vue: ["vue", "vue-router", "pinia"],
          // Naive UI 组件库（全量导入体积大，独立 chunk 利于缓存）
          "naive-ui": ["naive-ui"],
          // 图标库
          "@vicons/ionicons5": ["@vicons/ionicons5"],
          // Tauri API
          "@tauri-apps/api": ["@tauri-apps/api"],
        },
      },
    },
    // naive-ui 全量导入是既定取舍，提高阈值避免无效警告
    chunkSizeWarningLimit: 1500,
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    // 监听 Tauri 开发环境环境变量变化时自动重启
    watch: {
      // 忽略 Rust 构建产物目录：Cargo 并发写入时 chokidar 监听 target/ 会触发 EBUSY 崩溃
      ignored: ["**/src-tauri/**", "**/target/**"],
    },
  },
  // 预构建大依赖，加速 dev 模式首次加载
  optimizeDeps: {
    include: ["vue", "vue-router", "pinia", "naive-ui", "@vicons/ionicons5"],
  },
});
