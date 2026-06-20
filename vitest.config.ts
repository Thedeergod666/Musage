// Vitest 配置 —— 极简：只 include src/**/*.test.ts
// 与 vite.config.ts 的 Tauri 端口设置 / assetsInlineLimit / modulePreload
// 行为无关；测试纯函数 + 简单 DOM 场景，不启 dev server。
//
// **2026-06-20 audit**：当前只 src/settings/order.test.ts 一个文件，全部是纯函数
//（computeInsertIndex / isPlaceholderBeforeDivider / computeSameSectionMove），
// 跟 DOM / Tauri API 都无关，node env 即可。如果未来给 src/main.ts /
// settings.ts 加 test，需切 happy-dom + 在 setupFiles 里 mock
// @tauri-apps/api/core（否则 window.__TAURI_INTERNALS__ undefined 会抛）。
import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["src/**/*.test.ts"],
    // 已知约束：新增测试若 import main.ts / settings.ts，必须切 happy-dom
    // 并 mock @tauri-apps/api/core（详见上方注释）。
    environment: "node",
  },
});
