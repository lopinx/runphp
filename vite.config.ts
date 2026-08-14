import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

// 桌面端 Tauri 期望固定端口；面板模式无影响
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [vue()],
  // Tauri 单文件优先，避免代码分割带来的路径问题
  build: {
    target: "es2022",
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    // 监听 Tauri 开发环境环境变量变化时自动重启
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
});
