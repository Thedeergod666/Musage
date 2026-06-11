import { fileURLToPath, URL } from "node:url";
import { defineConfig } from "vite";

// Tauri 推荐的 Vite 配置：固定端口 + 监听
// 参考 https://tauri.app/start/frontend/vite/
//
// **注意 ESM**：package.json `"type": "module"` 让 vite.config.ts 走 ESM 加载，
// CommonJS 的 `__dirname` / `__filename` 都不可用 —— 直接用会 ReferenceError
// 把 dev/build 全打死。这里用 `new URL('./', import.meta.url)` 拿到当前文件
// 所在目录的 file:// URL，再 `fileURLToPath` 转回平台 path。
const root = fileURLToPath(new URL("./", import.meta.url));

export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: "127.0.0.1",
    watch: {
      // 防止 Vite 监听 src-tauri 触发重建
      ignored: ["**/src-tauri/**"],
    },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "esnext",
    minify: "esbuild",
    sourcemap: false,
    // **多页入口**：index.html（浮窗）+ settings.html（设置）。
    // 不加这个，prod build 只把 index.html 当 entry 处理，settings.html
    // 不会被编译 / 不会复制到 dist/，release 包打开设置窗会 404。
    // dev 模式（pnpm tauri dev）下 Vite dev server 从项目根目录直接 serve
    // 任意 HTML，所以 dev 看不到这个 bug。
    //
    // 附带收益：注册成 entry 后 Vite 会把 src/settings.css 编译并自动注入
    // <link rel="stylesheet"> 到 dist/settings.html <head> —— 不再等 JS
    // 执行完才 inject CSS，设置窗 FOUC 闪白进一步收敛（背景色本身在
    // src-tauri/src/commands.rs::build_settings_window 的 background_color
    // 里已经覆盖）。
    rollupOptions: {
      input: {
        main: `${root}index.html`,
        settings: `${root}settings.html`,
      },
    },
  },
});
