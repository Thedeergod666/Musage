# Musage 项目说明

> 任何新打开此项目的 Codex 会话应先读这个文件。这是当前对话的精炼版。

## 这是什么

**Musage** = **My** + **Usage**（"我的用量"），MiniMax Token Plan 实时用量监控的桌面应用。

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
| 前端类型 | `@types/node` 20.x（vite.config.ts 用 `node:url`） |
| Providers | minimax / deepseek / xiaomimimo / tavily / zenmux / openrouter（6 个） |

## 环境与工具链（2026-06-13 实测，跑 `pnpm tauri build` 必看）

机器上各工具的真实位置，**新会话别再花时间找**：

| 工具 | 路径 | 备注 |
|---|---|---|
| Node.js | `D:\Develop\node20\` | npm 10.8.2 |
| pnpm | `D:\Develop\node20\node_modules\pnpm\` | **没有 `pnpm.cmd` shim**，已通过 `C:\Users\33348\.cargo\bin\pnpm.cmd` shim 解决（内容 1 行：`"D:\Develop\node20\node.exe" "D:\Develop\node20\node_modules\pnpm\bin\pnpm.cjs" %*`）|
| cargo / rustc | `C:\Users\33348\.cargo\bin\`（`%USERPROFILE%\.cargo\bin`）| GNU 工具链，rustc 1.96 |
| MinGW | `D:\Develop\mingw64\bin\` | 提供 `dlltool.exe`，**GNU 工具链下 Rust 链接时必须**，否则 `error calling dlltool 'dlltool.exe': program not found` |
| 镜像 | `registry.npmmirror.com`（npm）/ `rsproxy.cn`（crates）| 已在 `~/.cargo/config.toml` + `dev-env.bat` 配好 |

**关键环境变量**（写 wrapper 时必设对）：
- `CARGO_HOME` = `C:\Users\33348\.cargo`（cargo **根目录**，不是 `bin/`！写错 cargo 找不到 `config.toml`，会走默认 crates.io 然后超时）
- `PATH` 必须包含：Node bin + cargo bin + MinGW bin + System32

**用户标准启动命令**（dev-env.bat 加载环境后调用 pnpm）：
```bash
cmd /c "dev-env.bat && pnpm tauri:dev"    # 开发
cmd /c "dev-env.bat && pnpm tauri:build"  # 打包
```
> ⚠ **从 MSYS bash / Claude Code shell 调 cmd 时**：`%PATH%` 会被 MSYS 翻译成 `C;D:\...` 这种坏值传给 cmd，cmd 找不到 `where`/`pnpm` 任何命令。**绕开**：`MSYS_NO_PATHCONV=1 cmd /c "..."` + 在 bat 里**硬编码**所有路径，**完全不碰 `%PATH%`**（参考 [musage-build-quirks](memory/musage-build-quirks.md) 第 6/7 条）

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
✅ **首次 `pnpm tauri build` 通过**（2026-06-13，NSIS 安装包 2.5 MB）
   - 路径：`src-tauri/target/release/bundle/nsis/Musage_0.1.0_x64-setup.exe`
   - 裸 exe：`src-tauri/target/release/musage.exe`（6.7 MB）

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

## 构建与打包（2026-06-13 实测）

完整 `pnpm tauri build` 跑通，记录几个坑给后续会话：

1. **`@types/node` 必须装**：`vite.config.ts` 用了 `import ... from "node:url"`，没装 `tsc` 阶段就报 `TS2307`。`package.json` devDependencies 已加。
2. **MSI 打包（WiX）走不通**：Tauri bundler 要从 `https://github.com/wixtoolset/wix3/releases/...` 下 WiX 3.14.1，国内网络 timeout / Peer disconnected 必现。**改用 NSIS 即可**：
   ```bash
   pnpm tauri build --bundles nsis
   ```
   NSIS 走 Tauri 自己的 binary-releases 仓库，下载稳定。**建议把 `tauri.conf.json` 的 `"targets": "all"` 改成 `"nsis"`** 固化下来，免得每次都先撞 WiX 再 fall back。
3. **首发会下几百 MB Rust crates**：5–15 分钟正常，二次构建 ≤2 分钟。
4. **不要在 dev 模式 (`pnpm tauri dev`) 测完就发**：dev 挂了 Vite dev server，朋友那边跑不起来。
5. **占位图标 + 蓝色 icon**：是 AGENTS.md 第 95 行提的占位，不影响功能。
6. **分发物**：
   - 首选：`src-tauri/target/release/bundle/nsis/Musage_*_x64-setup.exe`（双击安装，自动处理 WebView2）
   - 备用：裸 `src-tauri/target/release/musage.exe`（需朋友自己装 WebView2 Runtime）

## 浮窗 logo 打包后裂开（2026-06-13 修）

**症状**：dev 模式正常，打包后 Tavily / ZenMux 浮窗卡片上的 logo 显示成 broken image（裂开图标 🖼️💥），其余 4 个 provider（minimax / deepseek / xiaomimimo / openrouter）正常。

**根因**：Tauri 默认 CSP `default-src 'self'`，不放行 `data:` URI。Vite 默认 `build.assetsInlineLimit = 4096` 字节，tavily-logo.svg (2.2 KB) 和 zenmux-logo.svg (2.5 KB) 都 < 4KB → 被 Vite **内联成 `data:image/svg+xml,...` 字符串**塞进 JS bundle。`<img src="data:...">` 在 CSP 下被 block，浏览器显示 broken icon。
- PNG 的 4 个 logo 都 > 7KB，没被内联，躲过一劫
- dev 模式 Vite dev server 走真实文件 URL，不触发内联，所以 dev 看不到

**修法**（[src/main.ts:18-19](src/main.ts#L18-L19) + [vite.config.ts](vite.config.ts) + [src/assets.d.ts](src/assets.d.ts)）：
1. `vite.config.ts` 加 `build: { assetsInlineLimit: 0 }` —— 强制所有资源走外部文件，**根因层修**
2. SVG import 加 `?url` 后缀（`import tavilyLogo from "./x.svg?url"`）—— 显式声明"我要 URL"，配合 `assets.d.ts` 里 `*.svg?url` 的类型声明
3. `src/assets.d.ts` 加 `declare module "*.svg?url"` / `*.png?url` —— 让 tsc 认 `?url` 语法
4. 改完构建后 dist/assets/ 里会出现 `tavily-logo-XXXX.svg` 和 `zenmux-logo-XXXX.svg`，证明走外部文件路径

**为什么 `?url` 单独不够**：Vite 5 对 SVG 默认是走 `?url` 行为的（也走外部文件），但 `assetsInlineLimit` 是**全局**的兜底。`?url` + `assetsInlineLimit: 0` 配合最稳，前者声明意图，后者兜底任何后续新增的小资源。

## 文件结构（2026-06-13 实测，反映当前状态）

```
D:\Project\Musage\
├── AGENTS.md                 ← 本文件（项目 handoff 文档）
├── README.md                 ← 暂无，待写
├── package.json
├── pnpm-lock.yaml
├── tsconfig.json
├── vite.config.ts            ← build.assetsInlineLimit: 0（防 CSP 挡 data:）
├── dev-env.bat               ← 加载 Node/cargo/MinGW + 中国镜像
├── index.html                ← 悬浮窗入口
├── settings.html             ← 设置面板入口
├── src/                      ← 前端（vanilla TS）
│   ├── main.ts               ← 悬浮窗逻辑（拖动、订阅、渲染）
│   ├── updater.ts            ← 应用自动更新
│   ├── assets.d.ts           ← *.png/svg/?url 模块声明
│   ├── styles.css
│   ├── assets/               ← provider logo 资源
│   │   ├── minimax-logo.png
│   │   ├── deepseek-icon.png
│   │   ├── deepseek-logo.png
│   │   ├── xiaomimimo-logo.png
│   │   ├── openrouter-logo.png
│   │   ├── tavily-logo.svg   ← 配合 ?url + assetsInlineLimit: 0
│   │   └── zenmux-logo.svg   ← 同上
│   └── settings/             ← 设置面板子模块（18 个 .ts 文件）
│       ├── main.ts / app.ts / config.ts / api.ts
│       ├── credentials.ts / providers.ts / floating.ts
│       ├── order.ts / logs.ts / test.ts / about.ts
│       ├── updater.ts / advanced.ts / types.ts / utils.ts
│       ├── logos.ts / source-extras.ts
│       └── ...
└── src-tauri/                ← Rust 后端
    ├── Cargo.toml            ← crate-type = ["staticlib", "rlib"]（不用 cdylib）
    ├── tauri.conf.json       ← bundle targets: "all"（建议改 "nsis"）
    ├── build.rs
    ├── capabilities/default.json
    ├── icons/                ← 32/128/ico/icns/128@2x/tray-base 占位图标
    ├── assets/               ← font.ttf 待选填（无则托盘无百分比文字）
    └── src/
        ├── main.rs           ← Windows 入口
        ├── lib.rs            ← Tauri Builder + CLI 分流
        ├── api.rs            ← ★ 核心：拉取 + 宽容解析
        ├── poller.rs         ← tokio interval
        ├── tray.rs           ← 托盘菜单 + 动态图标（合并了原 icon.rs）
        ├── config.rs         ← AppConfig + keys.json 文件存储
        ├── commands.rs       ← tauri::command 暴露给前端
        ├── providers/        ← 6 个 provider 实现
        │   ├── mod.rs / minimax.rs / deepseek.rs / xiaomi.rs
        │   ├── tavily.rs / zenmux.rs / openrouter.rs
        └── platform/         ← 平台特定代码（仅 macOS 有非 stub 实现）
            ├── mod.rs
            └── macos.rs
```

构建产物：
- 裸 exe：`src-tauri/target/release/musage.exe`（6.7 MB）
- NSIS 安装包：`src-tauri/target/release/bundle/nsis/Musage_0.1.0_x64-setup.exe`（2.5 MB）

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

1. **README 启动文档**：朋友拿到 exe 怎么装、怎么填 API Key、托盘交互说明
2. **`cargo run -- dump` 真实 schema 探针**：跑通后端独立 CLI 子命令，验证 [关键 API](#关键-api来自-ccswitch-源码逆向) 文档的 percent-based 解析对真实返回有效
3. **设置面板对接真实 key**：填入 API Key → 联调 minimax + 其他 5 个 provider
4. **（可选）固化 NSIS-only**：`tauri.conf.json` 的 `bundle.targets` 改成 `"nsis"`，免得未来会话又撞 WiX timeout
5. **（可选）`assets/font.ttf`**：托盘图标想画百分比文字时再补

## 关键文件链接（按重要性）

- **核心 API 解析**：`src-tauri/src/api.rs` ← 改 schema 主要改这里
- **Provider 实现**：`src-tauri/src/providers/{minimax,tavily,zenmux,deepseek,xiaomi,openrouter}.rs`
- **托盘 UI**：`src-tauri/src/tray.rs`（合并了原 icon.rs：动态图标 + 文字绘制 + 菜单 + tooltip）
- **悬浮窗 UI**：`src/main.ts` + `src/styles.css`
- **设置面板**：`src/settings/main.ts` + `settings.html`（子模块见 `src/settings/` 18 个文件）
- **Logo 资源**：`src/assets/` + `src/assets.d.ts`（含 `?url` 声明，必读 [浮窗 logo 打包后裂开](#浮窗-logo-打包后裂开2026-06-13-修)）

## 关键记忆

跨会话保留的踩坑经验写到了 [musage-build-quirks.md](memory/musage-build-quirks.md)：
- WiX 镜像不可达 → 用 NSIS
- `@types/node` 必装
- pnpm shim 放在 cargo/bin
- CARGO_HOME 必须 cargo root
- MinGW dlltool 必在 PATH
- Tauri CSP + Vite assetsInlineLimit 兼容性（导致 Tavily/ZenMux 裂开）
