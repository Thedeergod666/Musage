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
    // 关掉 Vite 默认的 assetsInlineLimit(<4KB 资源自动内联成 data: URI)。
    // Tauri CSP `default-src 'self'` 不放行 data:, <img src="data:..."> 会被 block
    // → 浮窗里的 tavily / zenmux 小 SVG logo 显示成 broken icon。
    // 全部强制走外部文件后, dev (Vite dev server) 和 prod (Tauri bundled assets)
    // 行为一致, 都从 /assets/xxx.svg 取。
    assetsInlineLimit: 0,
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
    //
    // **文件名策略：拿掉 [hash]** —— Vite/Rollup 默认在 dist 资产文件名里
    // 掺 content hash(eg main-XXX.js),Tauri app 里完全用不到(资产都打 .app
    // / .exe / .msi 包里,版本由 Tauri updater 管,不需要浏览器 cache-busting)。
    // 反过来 macos arm64 runner 上 Vite 偶尔把 path/platform 信息漏进 hash
    // input,导致同源代码 ubuntu/windows dist 一致、macos dist hash 漂走
    // (CI 18 撞过这个 bug)。改用纯源名后,dist 跨 100% 平台一致。
    //
    // **关 modulePreload 全部** —— Tauri 内嵌 WebView2(macOS WKWebView、
    // Windows WebView2)都原生支持 <link rel="modulepreload">。Vite 默认
    // 会(1)生成一个含 polyfill IIFE 的共享 chunk,(2)给 index.html 追加
    // 一堆 <link rel="modulepreload"> 预热所有 import 链。
    // 关键问题:Vite 决定 modulepreload 列表的算法依赖 module graph 遍历
    // 顺序,macOS arm64 上偶尔会跟 linux/windows 不一致(CI 18/19/20 撞过),
    // 反映到 dist/index.html 上就是同样内容但不同 modulepreload 链接/顺序。
    // 这里 modulePreload 完全不优化(只留个 <script> + <link rel=stylesheet>),
    // HTML 跨 3 平台 byte-for-byte 一致。Trade-off:首次冷启动多一轮 RTT,
    // 对 Tauri 本地资源(都是 file:// 或 tauri://)完全可忽略。
    modulePreload: { polyfill: false, resolveDependencies: () => [] },
    rollupOptions: {
      input: {
        main: `${root}index.html`,
        settings: `${root}settings.html`,
      },
      output: {
        entryFileNames: "assets/[name].js",
        chunkFileNames: "assets/[name].js",
        assetFileNames: "assets/[name][extname]",
      },
    },
  },
});
