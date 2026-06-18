// Vitest 配置 —— 极简：只 include src/**/*.test.ts
// 与 vite.config.ts 的 Tauri 端口设置 / assetsInlineLimit / modulePreload
// 行为无关；测试纯函数 + 简单 DOM 场景，不启 dev server。
import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["src/**/*.test.ts"],
    // jsdom 用不到（被测函数全是纯 rects + index 计算），默认 node env 即可
    environment: "node",
  },
});
