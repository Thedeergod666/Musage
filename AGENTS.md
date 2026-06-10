# Musage 项目说明 (给未来的 Codex 会话)

> 任何新打开此项目的 Codex 会话应先读这个文件。这是当前对话的精炼版。

## 这是什么

**Musage** = MiniMax Token Plan 实时用量监控的桌面应用。

- 起源：ccswitch 3.16 的 MiniMax Token Plan 模板在 **2026-06-01 MiniMax 改 schema 后失效**（测试时报"未返回结果"），本项目自起炉灶
- 形态：**小悬浮窗 + 系统托盘**（始终置顶、可拖动、双行数据：5h 限额 / 周限额 + 重置时间）
- 鉴权：仅需 API Key（Bearer Token），不依赖浏览器 session
- 用户原始问题：[platform.minimaxi.com](https://platform.minimaxi.com/console/usage) 上的"套餐用量"页有数据，但 ccswitch 挂了
- **2026-06-10**：参考 ccswitch [PR #3518](https://github.com/farion1231/cc-switch/pull/3518) 实现了 percent-based 新 schema 解析

## 技术栈（已拍板）

| 层 | 选型 |
|---|---|
| 框架 | **Tauri 2.x** |
| 后端 | Rust (stable-x86_64-pc-windows-gnu) |
| 前端 | Vanilla TypeScript + Vite（无框架，极小） |
| HTTP | reqwest + rustls-tls |
| 密钥存储 | 本地 `keys.json` 文件（Unix 0600 权限，原子写）。原 keyring 方案在 macOS 上启动弹 Keychain 访问窗 + 解锁密码框，体验不优雅 |
| 动态托盘 | image + imageproc + ab_glyph |
| 异步 | tokio |
| 日志 | tracing + tracing-subscriber |
| 自动启动 | tauri-plugin-autostart |

## 关键 API（来自 ccswitch 源码逆向）

**端点**：
- CN: `https://api.minimaxi.com/v1/api/openplatform/coding_plan/remains`
- EN: `https://api.minimax.io/v1/api/openplatform/coding_plan/remains`

**请求**：`GET`，`Authorization: Bearer <api_key>`

**响应 schema（2026-06-01 之前的 count-based 版本，已对 Plus 订阅者失效）**：
```json
{
  "base_resp": { "status_code": 0, "status_msg": "success" },
  "model_remains": [{
    "current_interval_total_count": 200,    // 5h 总额度 (%)
    "current_interval_usage_count":  56,     // ⚠ 名字叫 usage 实际是"剩余"（满=total）
    "end_time": 1748500000000,               // 5h 重置 (毫秒)
    "current_weekly_total_count": 300,      // 周总额度
    "current_weekly_usage_count": 264,      // ⚠ 同上是剩余
    "weekly_end_time": 1749000000000
  }]
}
```

**响应 schema（2026-06-01 之后的 percent-based 新版本，参考 ccswitch PR #3518）**：
```json
{
  "base_resp": { "status_code": 0, "status_msg": "success" },
  "model_remains": [{
    "model_name": "general",                       // 选这一条
    "current_interval_remaining_percent": 72,      // 5h 剩余%
    "current_interval_status": 1,                  // 5h 状态（==1 有效；2/3 = 不在套餐）
    "end_time": 14523,                             // ⚠ 距离重置的**秒数**（不是 epoch ms）
    "current_weekly_remaining_percent": 86,        // 周剩余%
    "current_weekly_status": 1,                    // 周状态
    "weekly_end_time": 803245                      // ⚠ 同上，秒数
  }]
}
```

**已用百分比公式**：
- 新：`100 - *_remaining_percent`
- 旧：`((total - remaining) / total) * 100`

**解析策略**（见 [`src-tauri/src/providers/minimax.rs`](src-tauri/src/providers/minimax.rs)）：
1. 从 `model_remains[]` 优先选 `model_name == "general"`，找不到则取第一条
2. 先试 percent-based 路径（5h/周各自独立 gate on `status == 1`）
3. 失败回退到 count-based 路径
4. `resets_at` 智能识别：值在 `[10^12, 4*10^12]` 范围当 epoch ms，否则当 duration-seconds 加到 now

**已知坑**（参考 MiniMax-M2 #99, cli #165, cli #173）：
- `*_remaining_percent=100` 不代表"还有 100%"，可能是 `status=2/3`（不在套餐内）
- 旧字段对 Plus 订阅者全为 0

## 当前进度

✅ 项目骨架完整（D:\Codes\Musage\）
✅ Rust 核心代码：main/lib/api/poller/tray/config/commands/icon
✅ 前端：main.ts / styles.css / settings.ts / settings.html
✅ 托盘图标动态绘制（颜色 + 百分比文字）
✅ 本地 `keys.json`（0600）存 key，macOS 启动零弹窗
✅ 后台 tokio 轮询
✅ CLI `musage dump` 子命令
✅ Rust GNU 工具链已装
✅ Pillow 已装（用于生成占位 icons）
✅ **`cargo check` 0 错 0 警告**（10 个编译错误全部修复）
✅ **`cargo build` 通过**（修了一个 MinGW 16-bit ordinal 限制坑——见下）
✅ 占位 icons 已生成（32/128/ico/icns/128@2x/tray-base）

⚠️ **坑：MinGW 工具链 16-bit 导出表上限**
- 现象：`cdylib` 链接时 `ld.exe: error: export ordinal too large: 141874`
- 原因：cargo 给 cdylib 自动生成的 .def 文件含 14 万+ 符号，超 65535
- **解法**：`crate-type = ["staticlib", "rlib"]` —— 删 `cdylib`
- 依据：Tauri 2 在 Windows 上只用 staticlib 就够，cdylib 是为 iOS/Android 准备的
- **别再用 RUSTFLAGS 全局加 `-Wl,--no-export-all-symbols`**：会污染 build script exe

⏳ **待做**：
- README + 启动文档
- 联调真实 API（M1 验证，跑 `pnpm tauri dev` 后通过 dump 子命令探新 schema）
- 设置面板对接真实 key
- 选填 `assets/font.ttf`（目前托盘图标无百分比文字）

## 文件结构

```
D:\Codes\Musage\
├── AGENTS.md                 ← 本文件
├── package.json
├── tsconfig.json
├── vite.config.ts
├── index.html                ← 悬浮窗入口
├── settings.html             ← 设置面板
├── src/                      ← 前端
│   ├── main.ts               ← 悬浮窗逻辑（拖动、订阅事件、渲染）
│   ├── styles.css
│   └── settings.ts           ← 设置面板逻辑
└── src-tauri/                ← Rust 后端
    ├── Cargo.toml
    ├── tauri.conf.json
    ├── build.rs
    ├── capabilities/default.json
    ├── icons/                ← 占位图标待生成
    ├── assets/               ← font.ttf 待选填（无则托盘无文字）
    └── src/
        ├── main.rs           ← Windows 入口
        ├── lib.rs            ← Tauri Builder + CLI 分流
        ├── api.rs            ← ★ 核心：拉取 + 宽容解析
        ├── poller.rs         ← tokio interval
        ├── tray.rs           ← 托盘菜单 + 动态图标（合并了原 icon.rs）
        ├── config.rs         ← AppConfig + keys.json 文件存储
        ├── commands.rs       ← tauri::command 暴露给前端
        └── platform/         ← 平台特定代码（仅 macOS 有非 stub 实现）
            ├── mod.rs
            └── macos.rs
```

## 已确立的设计决策

1. **不用 MSVC 工具链**（环境无 cl.exe），用 GNU 工具链，自带 MinGW
2. **托盘图标动态生成**，不依赖打包好的图标资源
3. **API key 落 `keys.json`（0600 权限）**，不走 OS keyring。原 keyring 方案在 macOS 启动会弹 Keychain 访问窗 + 登录钥匙串密码框，体验糟
4. **前端极简**（vanilla TS），无 React/Vue，避免启动慢
5. **schema 宽容解析**：`api.rs` 支持字段名多版本，回退到原始 JSON 让开发者肉眼定位
6. **关闭悬浮窗 = 隐藏**，不退出 app（`WindowEvent::CloseRequested` 拦截）
7. **tray 左键单击 = 切换悬浮窗显隐**
8. **tray 菜单**：显示 / 设置 / 立即刷新 / 退出
9. **`crate-type` 不用 cdylib**：Tauri 2 + Windows + GNU 工具链下，cdylib 会触发 MinGW ld 16-bit ordinal 表溢出
10. **tray 逻辑合并在 tray.rs**：原 icon.rs 已删除，所有托盘 + 图标生成代码都集中在 `tray.rs`
11. **macOS 置底走私有 API（platform/macos.rs）**：仅 `set_always_on_top(false)` 在 macOS 上不够 —— 窗口会变成 `kCGNormalWindowLevel = 0`，前台调度会把它埋掉。`platform::macos` 用 `objc2` 直接调 `NSWindow.setLevel()`，PinBottom 时设到 `kCGNormalWindowLevel - 1`（即 -1），低于所有普通 app 窗口但高于桌面。同时启一个 background thread 轮询 `NSEvent.mouseLocation()` + 窗口 `frame` 做点-in-rect，因为窗口在 level -1 时被其它 app 盖住，JS `mouseenter` 触发不到。详见 [musage-ui-design](memory/musage-ui-design.md)。非 macOS 平台 stub 走 Tauri 原生 `set_always_on_top`。

## 已知风险

- 🟡 MiniMax 6/1 改 schema，新字段名未明
- 🟡 GNU 工具链 + Tauri 2 在 Windows 的稳定性需要实战验证
- 🟡 托盘图标的字体加载需要 `assets/font.ttf`，缺了就只有色块

## 下一步建议

1. 生成占位 icons + 可选 font.ttf
2. 写 README 启动文档
3. `pnpm install`（拉前端依赖）
4. `cargo build`（第一次会下大量 crate 依赖，可能 5-10 分钟）
5. 修复编译错误（窗口 API 在 Tauri 2 经常小改）
6. `pnpm tauri dev` 跑起来
7. 通过 `cargo run -- dump` 探针定位新 schema
8. 在前端填写 API key 试联调
9. `pnpm tauri build` 打包 .msi

## 关键文件链接（按重要性）

- **核心 API 解析**：`src-tauri/src/api.rs` ← 改 schema 主要改这里
- **托盘 UI**：`src-tauri/src/tray.rs`（合并了原 icon.rs：动态图标 + 文字绘制 + 菜单 + tooltip）
- **悬浮窗 UI**：`src/main.ts` + `src/styles.css`
- **设置面板**：`src/settings.ts` + `settings.html`
