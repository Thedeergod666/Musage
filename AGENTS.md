# Musage 项目说明

> 任何新打开此项目的 AI 会话应先读这个文件。这是当前对话的精炼版（v0.2.4 / 2026-07-21 快照）。

## 这是什么

**Musage** = **My** + **Usage**（"我的用量"），多 provider AI 套餐实时用量监控的桌面应用。

- 形态：**小悬浮窗 + 系统托盘**（始终置顶、可拖动、双行数据：5h 限额 / 周限额 + 重置时间）
- 鉴权：仅需 API Key / Cookie（Bearer Token），不依赖浏览器 session
- **11 内置 provider + 任意 New API 中转站 (custom_<uuid>) + 同 provider 多实例副本**
- 关键 schema 见下方"关键 API"章节（2026-06-01 MiniMax 改 schema，v0.2.0 已实现兼容）

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
| Providers | minimax / deepseek / xiaomimimo / tavily / zenmux / openrouter / kimi / zhipu / stepfun / siliconflow / claude_official + **用户自定义 New API 中转站 (custom_<uuid>)**（11 内置 + N 动态） |

## 环境与工具链

### macOS（维护者本地，2026-06-29 实测）

| 工具 | 路径 | 备注 |
|---|---|---|
| Node.js | `node --version` ≥ 20 | pnpm 9.15.4 (packageManager 钉死) |
| pnpm | `pnpm --version` 9.15.4 | |
| cargo / rustc | `~/.cargo/bin/` | rustc 1.96+ (rustup default stable) |
| Xcode CLT | `xcode-select --install` | 提供 clang 链接器 + macOS SDK |
| `MACOSX_DEPLOYMENT_TARGET` | 11.0 | CI 注入 + 本地 objc2 build 也需要 |

### Windows（开发 + 打包 target，2026-06-13 实测）

| 工具 | 路径 | 备注 |
|---|---|---|
| Node.js | `D:\Develop\node20\` | npm 10.8.2 |
| pnpm | `D:\Develop\node20\node_modules\pnpm\` | **没有 `pnpm.cmd` shim**，已通过 `C:\Users\33348\.cargo\bin\pnpm.cmd` shim 解决（内容 1 行：`"D:\Develop\node20\node.exe" "D:\Develop\node20\node_modules\pnpm\bin\pnpm.cjs" %*`）|
| cargo / rustc | `C:\Users\33348\.cargo\bin\` | GNU 工具链，rustc 1.96 |
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
> ⚠ **从 MSYS bash / Claude Code shell 调 cmd 时**：`%PATH%` 会被 MSYS 翻译成 `C;D;\...` 这种坏值传给 cmd，cmd 找不到 `where`/`pnpm` 任何命令。**绕开**：`MSYS_NO_PATHCONV=1 cmd /c "..."` + 在 bat 里**硬编码**所有路径，**完全不碰 `%PATH%`**（参考 [musage-build-quirks](memory/musage-build-quirks.md) 第 6/7 条）

## 2026-06-20 全量代码审查修复 (10 个 commit)

[musage-known-bugs.md](memory/musage-known-bugs.md) 记录的 **1 critical + 6 high + 19 medium + 31 low = 60 个 bug 全部修复**。最关键的几条:

- **CRITICAL 死锁** (`write_keys_atomic` 内部 `save_lock()` + 6 个调用方外层 `save_lock()` = std::sync::Mutex 不可重入 → 所有 save_key / delete_key / save_cookie IPC 永久阻塞) — 删内部锁
- **HIGH schema_overrides 保存失效** (`loadConfig` / `saveConfig` 全死代码) — 加新 `set_schema_overrides` IPC + advanced.ts 3 个 textarea 接 debounce
- **HIGH 升级面板不渲染** (`updater.ts` 找 `#save`,settings.html 没这元素) — 改挂到 `about.ts` 建的 `#updater-section`
- **HIGH CSP 挡 data: URI** (浮窗 fallback logo 裂开) — 加 `img-src 'self' data:` + `connect-src 'self' ipc:`
- **HIGH `bundle.targets "all"` 含 MSI** (WiX 镜像 timeout) — 改 `["nsis"]`
- **HIGH capabilities 一锅炖** (process:default 给所有 webview) — 拆 default + settings,process:default 全部移走
- **MEDIUM poller 用 JoinSet<()>** 替代 fire-and-forget,防 panic storm
- **LOW release.yml action 全部钉 SHA** 跟 ci.yml 一致
- **LOW t() regex 扩 `[\w.-]+`**,允许 `{user-id}` / `{err.code}` placeholder

完整 commit 列表走 `git log --oneline | head -20`。

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

## 当前进度（v0.2.4 快照，2026-07-21）

✅ **v0.2.4 已发布**（git tag `v0.2.4`，2026-07-17）
   - feat(kimi)：浮窗左侧标签改动态窗口剩余（剩 <1 天 → `5h`，≥1 天 → `7d`），替代 used/total；foot 前缀跟随（`5h重置`/`7d重置`），Tavily 无 kind 标记保持原样（commit `75a5d8f`）
   - feat(floating)：双击浮窗打开设置面板（原双击"立即刷新"移除，托盘菜单仍可触发；跳过 button/input/select/a 防误触）（commit `361fc55`）
   - fix：5h 用量达 100% 上限时 kimi / zhipu / claude_official 行被隐藏（commit `de6668b`）
   - v0.2.1/v0.2.2 frontend i18n hotfix（[src/i18n/index.ts](src/i18n/index.ts) static import 修 Vite dynamic import chunk 缺失）
   - v0.2.3 macOS 26 tray icon visual hotfix：[src-tauri/icons/tray-base.png](src-tauri/icons/tray-base.png) 重做为 64×64（48px 内容 + 8px 透明 padding 四边），圆外径 32→24 (-25%)，halo 消失
   - v0.2.2 + v0.2.3 都没正式 ship（v0.2.1 → v0.2.3 直接跨度），CHANGELOG 两段都保留

⏳ **Unreleased（v0.2.5 候选，详见 CHANGELOG [Unreleased] 段）**：
   - **StepFun 集成重写**（commit `0d51124`，2026-07-21）：端点迁 `platform.stepfun.com`；`Oasis-Webid` 请求头从 token refresh half 的 JWT `device_id` claim 本地提取（CodexBar 逆向，新增 `base64` 依赖 `URL_SAFE_NO_PAD`），缺 webid 一律 401；token 过期/格式本地预检（`token_expired_hint` 带过期分钟数 / `token_malformed_hint`），不再让用户拿 401 猜原因；credit 套餐（`plan_family=2` Mini/Pro）解析 + 单行「额度」（新 i18n key `row.credit`）；支持整段 `Cookie: Oasis-Token=...` 粘贴自动剥离
   - **Win PinBottom hover-raise 重写**（commit `ff309bb`，2026-07-20）：dwell hysteresis + 两级命中（`Visible` 1 tick / `Covered` 250ms dwell / `Outside` 150ms）+ edge-trigger + 1s re-assert 兜底，详见下方专节
   - **玻璃 backdrop throttling 三层防御**（commit `1a38d89`）：`will-change: backdrop-filter` + 4s 心跳 keyframes + `set_window_level` 后 emit `musage://backdrop-refresh` 强制重采；idle 玻璃参数向 Usticky 对齐（blur 28px / saturate 180% 写死，不再 idle 切换）

✅ v0.2.1 全部完成 + v0.2.2/v0.2.3/v0.2.4 增量（详见 CHANGELOG 对应段）

✅ 项目骨架完整
✅ 12 个 provider 全实装（11 内置 + custom），全部加 `instance_index` + `unique_id()` + `with_instance_index()`
✅ 12 个 provider 全部支持**多实例**（`minimax#2` / `minimax#3` 共存）
✅ Rust 核心代码：main / lib / poller / poller_backoff / tray / config / commands / xiaomi_login / logstore（icon.rs 已并入 tray.rs，api.rs 已拆进 providers/）
✅ 前端：main.ts / settings.ts + settings/ 21 个子模块
✅ 托盘图标动态绘制（颜色 + 百分比文字 + 多实例 `#N` 后缀）
✅ 本地 `keys.json`（0600）存 key + cookie，macOS 启动零弹窗
✅ 后台 tokio 轮询 + per-provider 指数退避（30min 上限）
✅ CLI `musage dump` 子命令
✅ i18n P0-P3 完整（rust-i18n 后端 + 自写 helper 前端，en + zh-CN，运行时切换）
✅ 设置面板重构（6 组分组 + 搜索 + 两段式 picker modal）
✅ Extra Instance（PR 1b）：内置 provider 副本 + 统一 `extra_instances.json` 持久化
✅ 11 内置 + N 动态架构：`QuotaSource` trait + `builtin_sources()` + `CustomSource`
✅ Xiaomi 一键登录 WebView + 系统通知 cookie 失效
✅ import/export 配置（无 keys）
❌ ~~自动更新~~：**v0.2.0 已删 tauri-plugin-updater**（`TAURI_SIGNING_PRIVATE_KEY` GitHub Secret 未配 → Windows build 报 "Missing comment in secret key" 整批 release 挂，commit `586e55c`）。升级走「GitHub release 手动下载 dmg/nsis/AppImage/deb/rpm 覆盖装」，设置面板「关于」页放 releases 链接（[src/settings/about.ts](src/settings/about.ts)）。详见 [RELEASING.md](RELEASING.md)
✅ **`cargo check` 0 错**（v0.2 cleanup 砍 dead code + Provider enum，剩 `#[allow(dead_code)]` 2 处是 v2 预留）
✅ **`cargo test --lib` 196 passed**（v0.2.0 follow-up 修 10 broken test + 23 i18n assertion + 1 production i18n bug）
✅ **`pnpm tsc --noEmit` 0 errors**
✅ **`pnpm tauri build` 通过**（macOS dmg + Windows NSIS + Linux AppImage/deb/rpm）

⚠️ **坑：MinGW 工具链 16-bit 导出表上限**
- 现象：`cdylib` 链接时 `ld.exe: error: export ordinal too large: 141874`
- 原因：cargo 给 cdylib 自动生成的 .def 文件含 14 万+ 符号，超 65535
- **解法**：`crate-type = ["staticlib", "rlib"]` —— 删 `cdylib`
- 依据：Tauri 2 在 Windows 上只用 staticlib 就够，cdylib 是为 iOS/Android 准备的
- **别再用 RUSTFLAGS 全局加 `-Wl,--no-export-all-symbols`**：会污染 build script exe

⏳ **v0.3 待做**（tech debt 收尾 + 新需求）：
- Claude cookie 一键重登（小米已做，Claude cookie 抓取要研究）
- monitor hotplug 监听（拔插副屏时实时重新判定浮窗位置）
- 错误卡"忽略本次错误"按钮
- Frontend 单元测试 4 核心函数（contentFingerprint / render / updateCard / autoResizeWindow）
- ~~`http_status_to_error_kind` helper~~ → 已落地为 [`classify_http_status`](src-tauri/src/providers/mod.rs)（2026-07-02 audit L1 fix），kimi 先用；其余 provider 保留各自的具体 msg 短路，**全面推广留 v0.3**
- `refresh_inner` 每次 `Box::new` 12 个 source 优化（按 Arc 缓存）
- Backoff 状态持久化到 disk
- Per-provider poller task shutdown signal（App 退出时不泄漏）
- `delete_extra_instance` v2（重命名 keys.json + spec）

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

## macOS 安装后显示「应用已损坏」（2026-06-17 修）

**症状**：从 GitHub release 下载 `Musage_0.1.0_aarch64.dmg`，挂载后把 `Musage.app` 拖进 `/Applications`，双击启动弹窗 "「Musage」已损坏，无法打开。你应该将它移到废纸篓"。

**根因**（已确证）：
- DMG 上有 `com.apple.quarantine` xattr（Edge 下载的产物）→ `xattr -l Musage_0.1.0_aarch64.dmg` 能看到 `com.apple.quarantine: 0281;...;Edge;...`
- DMG / `.app` **完全没有代码签名** → `codesign -dv /Users/wyh/Downloads/Musage_0.1.0_aarch64.dmg` 报 `code object is not signed at all`
- `tauri.conf.json` 的 `bundle.macOS` 字段原本**完全缺失**（没有 `signingIdentity` / `entitlements`）
- 触发链：quarantine xattr 存在 + 没有 Developer ID 签名 → macOS Gatekeeper (Big Sur+) 拦截，显示"已损坏"。**和"应用真坏了"无关**，是签名/公证问题。

**修法**（两步）：

**A. 项目侧**（已落地，本次 commit）：
- 新增 [`src-tauri/entitlements.plist`](src-tauri/entitlements.plist) — Hardened Runtime 必需的 entitlement 集合：
  - `com.apple.security.cs.allow-jit` — WKWebView 内部 JIT
  - `com.apple.security.cs.allow-unsigned-executable-memory` — WKWebView 内部
  - `com.apple.security.cs.disable-library-validation` — 让 objc2 链系统 framework + `setLevel(-1)` 等私有 API 生效
  - `com.apple.security.network.client` / `.server` — provider 拉 API + GitHub release 页跳转（自动更新已删，见「当前进度」段）
  - **不**开 `com.apple.security.app-sandbox`（与 `platform/macos.rs` 把窗口放到 `kCGNormalWindowLevel - 1` 互斥）
- `tauri.conf.json` 加 `bundle.macOS`：
  ```json
  "macOS": {
    "signingIdentity": null,            // null = 跳过签名 → 出 unsigned dmg
    "providerShortName": null,          // signingIdentity 非 null 时填 notarytool keychain profile
    "entitlements": "entitlements.plist",
    "minimumSystemVersion": "10.15"
  }
  ```

**B. 用户侧应急**（针对已下载的 v0.1.0 dmg，无 Apple Developer ID 也能跑）：
```bash
# 1. 挂载 dmg,把 Musage.app 拖进 /Applications
open /Users/wyh/Downloads/Musage_0.1.0_aarch64.dmg
cp -R /Volumes/Musage/Musage.app /Applications/

# 2. 清 quarantine + ad-hoc 签
xattr -cr /Applications/Musage.app
codesign --force --deep --options runtime --sign - /Applications/Musage.app

# 3. 双击启动即可 (spctl 会报 rejected, 但能跑)
```

**有 Apple Developer ID 之后**（走真签名 + 公证，零提示弹窗）：
```bash
# 1. 一次性配置 notarytool keychain profile
xcrun notarytool store-credentials Thedeergod666-Notary \
    --apple-id "you@example.com" --team-id "TEAMID" --password "app-specific-pw"

# 2. 改 tauri.conf.json:
#    signingIdentity: "Developer ID Application: Your Name (TEAMID)"
#    providerShortName: "Thedeergod666-Notary"
# 3. pnpm tauri build   # Tauri 自动签名 + 公证 + staple
```

**为什么用 Hardened Runtime + 这些 entitlement**：
- Tauri 2 的 `app.macOSPrivateApi: true`（`tauri.conf.json:13`）启用 Tauri 内部用私有 macOS API，**必须** Hardened Runtime
- 后端 `src-tauri/src/platform/macos.rs` 用 `objc2` 直接调 `NSWindow.setLevel(-1)` —— 需要 `disable-library-validation` 让 objc2 链系统 framework
- Hardened Runtime 默认拒绝以上行为 → 必须显式 grant entitlement

**为什么 `signingIdentity: null` 而不是"想办法签上"**：
- macOS 上**没有** Apple Developer ID 没法真签（用户机器 `security find-identity -p codesigning -v` 报 `0 valid identities found`）
- 留 null 走未签名构建是 honest default：build 不会偷偷失败，输出 unsigned dmg 让你明明白白手动 ad-hoc sign

**已知次生坑**：昨天的 crash report `~/Library/Logs/DiagnosticReports/musage-2026-06-16-133409.ips` 是 dev 模式（`parentProc: "node"` = Tauri dev server）+ Tauri 自己 subprocess 跑飞，跟本 bug **无关**，是另一条线。

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

## Win 端 PinBottom 模式 hover-raise 修复（2026-07-20 根因定位 + 重写）

**症状**（v0.2.4 用户反馈）：PinBottom 模式下，鼠标 hover 进悬浮窗可见区时浮窗**不**临时置顶。

**根因**（修正 2026-06-12 的"OS 持续 demote"误判）：`WS_EX_TOPMOST` 是 sticky 的，OS 不会自发清掉。真正的失败机制是**自己撤销自己** —— 旧 tracker 没有防抖：hit test（`WindowFromPoint`）单 tick 抖动返一拍 false（DWM 重绘 / WebView2 瞬态子窗口 / 光标压 rect 边界 / 光标在"被遮挡与未遮挡"细条间摆动），旧代码立刻 edge-drop 把窗口塞回 `HWND_BOTTOM`。raise 后 50~100ms 内就被自己的 drop 撤销，肉眼看到"hover 不变置顶"。历史实测的 3/7 命中率（7 次 inside=true 只 3 次肉眼可见）也是同一机制。

**修复**（[src-tauri/src/platform/windows.rs](src-tauri/src/platform/windows.rs) `start_hover_emitter`，结构对齐 macos.rs）：
- **dwell-time hysteresis**：enter 1 tick（`Visible`，≤50ms 响应；`Covered` 走下方两级命中）/ exit 3 tick（150ms）—— 单/双 tick 抖动被吞，不再误 drop。macOS 用 exit 2，Win 的 `WindowFromPoint` 抖动更频繁所以取 3
- **edge-trigger 切换**：raise / drop 只在**采纳的**状态切换时各做一次，不再 20Hz 反复 `SetWindowPos`（无效 churn + 扩大跟 tao/WebView2 主线程窗口管理的竞争窗口）
- **稳定 hover 期间 1s 低频 re-assert 一次 TopMost** 兜底（防极端情况被别的窗口管理操作顶掉）
- emit 也改到采纳后触发 —— 顺带修了 Win 端玻璃 hover 的 spring 闪烁（同 macOS 2026-07-03 修复的收益）
- 采纳切换时 `tracing::debug!` 落日志（默认 `musage=debug` 可见）

**两级命中语义**（2026-07-20 第二轮，取代原"未被遮挡才算"macOS-parity 语义）：hit test 返三态 —— `Visible`（鼠标在未被盖区域）1 tick 即抬；`Covered`（鼠标在被盖区域）**连续 dwell 5 tick（250ms）** 也抬；`Outside` 3 tick（150ms）落回 `HWND_BOTTOM`。改动动机：Win 用户真实场景是浮窗**长期被最大化窗口盖住大半、只露一条边**，严格"未被遮挡"语义下鼠标几乎永远落在被盖区域，hover-raise 等于不存在（v0.2.4 用户实测反馈）。250ms dwell 挡住路过式误触发（鼠标横穿浮窗所在屏幕区域通常 <100ms）；抬起走 `SWP_NOACTIVATE` 不抢焦点。已知代价：用户把鼠标停在浮窗被盖区域干别的事（>250ms）时浮窗会弹出遮一下，移开即恢复。注意 macOS 端仍是严格"未被遮挡"语义（`windowNumberAtPoint`），两平台**有意分歧** —— macOS 窗口很少被完全盖住（fullscreen 是独立 Space），Win 最大化是常态。

**逃生口**（保留）：tray 右键菜单"强制置顶浮窗（Win 逃生口）"走 `AllowSetForegroundWindow(ASFW_ANY) + SetForegroundWindow` —— **会**抢焦点，但**用户主动**点菜单触发的操作 UX 上可接受。代码 [src-tauri/src/tray.rs](src-tauri/src/tray.rs) 的 `"force_top_floating"` 分支。

**如果 2026-07-20 版仍不生效**，再考虑 **WndProc subclass**（拦截 `WM_WINDOWPOSCHANGING`），侵入性大、可能跟 Tauri/tao 自己的 WndProc 打架，得严格测。

**历史记录（2026-06-12/13 实测，已被上方根因取代）**：当时试过的无效路径 —— 16ms level-trigger re-assert / dual-path（`SetWindowLongW` + `SetWindowPos`）/ focus event hook / `WS_EX_NOACTIVATE`（反而更糟，撤回）。covered check 的**判定机制**（`WindowFromPoint + GetAncestor(GA_ROOT)`）是当时落地的正确部分，保留至今 —— 2026-07-20 第二轮只改了它的**语义**（从"被盖一律不抬"改成"被盖 dwell 250ms 抬"）。

## 文件结构（v0.2.4 / 2026-07-21 快照）

```
~/Project/Musage/                  ← 当前在 macOS 上,Win 路径 D:\Codes\Musage\
├── AGENTS.md                 ← 本文件（项目 handoff 文档）
├── README.md                 ← GitHub 访客入口,11 内置 + N 动态 + 多实例介绍
├── CHANGELOG.md              ← 版本变更日志
├── ROADMAP.md                ← 当前路线图
├── FUTURE.md                 ← 暂缓 / 砍掉的想法
├── RELEASING.md              ← 维护者发版 cheat sheet
├── package.json
├── pnpm-lock.yaml
├── tsconfig.json
├── vite.config.ts            ← build.assetsInlineLimit: 0（防 CSP 挡 data:）
├── dev-env.bat               ← 加载 Node/cargo/MinGW + 中国镜像 (Win 专用)
├── index.html                ← 悬浮窗入口
├── settings.html             ← 设置面板入口
├── src/                      ← 前端（vanilla TS）
│   ├── main.ts               ← 悬浮窗逻辑（拖动、订阅、渲染、i18n）
│   ├── settings.ts           ← 设置面板入口
│   ├── assets.d.ts           ← *.png/svg/?url 模块声明
│   ├── styles.css / tokens.css
│   ├── assets/               ← provider logo 资源（11 内置 + 部分 SVG）
│   └── settings/             ← 设置面板 21 个子模块
│       ├── main.ts / app.ts / config.ts / api.ts / types.ts / utils.ts
│       ├── credentials.ts / providers.ts / floating.ts / order.ts
│       ├── logs.ts / test.ts / about.ts / advanced.ts
│       ├── extra-instance-form.ts  ← + 添加新来源（PR 1b 两段式 picker）
│       ├── source-extras.ts        ← per-provider 渲染 region/mode/extras
│       ├── groups.ts                ← 6 组分组
│       ├── modal.ts                 ← 原生 <dialog> 包装
│       ├── region-wizard.ts         ← set_region command UI（多 region provider）
│       ├── logos.ts                 ← logo 路径映射
│       ├── icons.ts                 ← 首字母 fallback logo
│       └── order.test.ts            ← vitest 单元测试（前端首批）
├── src-tauri/                ← Rust 后端
│   ├── Cargo.toml            ← crate-type = ["staticlib", "rlib"]（不用 cdylib）
│   ├── tauri.conf.json       ← bundle targets: nsis + dmg（CI matrix 另加 msi / appimage,deb,rpm）,version 0.2.4
│   ├── entitlements.plist    ← macOS Hardened Runtime entitlement
│   ├── build.rs
│   ├── capabilities/         ← default.json + settings.json + xiaomi-login.json（权限拆分）
│   ├── icons/                ← 32/128/ico/icns/128@2x/tray-base
│   ├── locales/              ← 后端 i18n (en.json + zh-CN.json)
│   ├── assets/               ← font.ttf 待选填（无则托盘无百分比文字）
│   └── src/
│       ├── main.rs           ← Windows / Linux 入口
│       ├── lib.rs            ← Tauri Builder + CLI 分流 + load_or_migrate
│       ├── poller.rs         ← tokio interval + per-provider 调度
│       ├── poller_backoff.rs ← 指数退避 (429/5xx 翻倍,30min 上限)
│       ├── tray.rs           ← 托盘菜单 + 动态图标 + 多实例 #N tooltip
│       ├── config.rs         ← AppConfig + keys.json 文件存储
│       ├── logstore.rs       ← 内存日志环形缓冲（设置面板 logs section）
│       ├── xiaomi_login.rs   ← Xiaomi 一键登录 WebView
│       ├── commands/         ← tauri::command 暴露给前端
│       │   ├── mod.rs            ← 30+ IPC
│       │   ├── extra_instances.rs ← 6 IPC (list/add/update/delete/list_picker/test)
│       │   └── i18n.rs           ← set_app_locale + locale 切换
│       ├── config/            ← 持久化子模块
│       │   └── extra_instances.rs ← ExtraInstance + load/save/compact_indexes (9 单测)
│       ├── providers/         ← 12 provider (11 内置 + custom)
│       │   ├── mod.rs             ← QuotaSource trait + builtin_sources 注册表
│       │   ├── parse.rs           ← JSONPath + num_f64 helpers
│       │   ├── custom.rs          ← CustomSource + CustomSourceSpec + ExtractSpec
│       │   ├── minimax.rs         ← 2026-06-01 前后双 schema
│       │   ├── deepseek.rs / xiaomi.rs / tavily.rs / zenmux.rs
│       │   ├── openrouter.rs / kimi.rs / zhipu.rs
│       │   ├── stepfun.rs / siliconflow.rs / claude_official.rs
│       └── platform/         ← 平台特定代码
│           ├── mod.rs
│           ├── macos.rs      ← PinBottom 走 NSWindow.setLevel(-1) + hover emitter
│           └── windows.rs    ← hover emitter（dwell hysteresis + 两级命中）+ Per-Monitor V2 DPI
└── docs/
    ├── codeplan/             ← 历史 plan / review notes
    │   └── 2026-06-15-extend-providers.md
    └── research/             ← 调研报告
```

构建产物（本地 `pnpm tauri build` 只出 nsis + dmg；CI matrix 全量）：
- macOS dmg：`src-tauri/target/release/bundle/dmg/Musage_0.2.4_*.dmg`（aarch64 + x64 两个）
- Windows NSIS：`src-tauri/target/release/bundle/nsis/Musage_0.2.4_x64-setup.exe`（CI 另有 MSI）
- Linux（CI）：`bundle/appimage/Musage_0.2.4_amd64.AppImage` + `bundle/deb/musage_0.2.4_amd64.deb` + `bundle/rpm/musage-0.2.4-1.x86_64.rpm`
- 裸 exe：`src-tauri/target/release/musage.exe`（仅 Windows）

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

**PR 3（2026-06-16，CustomSource + 设置面板重构）新增决策**：
- **QuotaSource trait** `id()` / `display_name()` 从 `&'static str` 改 `Cow<'_, str>`：内置 source 返 `Cow::Borrowed`（零分配），CustomSource 返 `Cow::Owned`
- **CustomSource**：让用户加/改/删自己的 New API 中转站（dmx / byteplus / lemondata / ctok / silicon / crazyrouter / cubence / dds / runapi / ucloud / shengsuanyun 等）。一次实现吃掉 10+ 个异构 provider
- **持久化**：`custom_sources.json`（独立文件，原子写 + 0600，parse 失败 backup 到 `.bak.<ts>`）；key 仍存 `keys.json` 的 `custom_<uuid>` key
- **Extract 模板**（v1 = 3 选 1）：New API 系（写死 `data.quota / data.used_quota`，divide 500000）/ 余额系（用户填 `balance_path`）/ 自定义（3 个独立 path）
- **设置面板**：原生 `<details>/<summary>` 分组（6 组：token_plan / balance / official / xiaomi / custom / misc）+ 顶部搜索框 + 「+ 添加自定义来源」按钮 + 原生 `<dialog>` modal
- **删除确认**：`confirm()` + 二次输入 display_name（防误删短 id）

**PR 1b（2026-06-24，Extra Instance · 内置 provider 副本）新增决策**：
- **动机**：PR 3 的 "custom" 通道只支持 New API 中转站，**不能加同种内置 provider 的副本**。但现实中一个用户常有多份同种主流套餐（同时持 2 个 MiniMax 套餐 / 2 个 DeepSeek 账号等）。PR 1b 把"任意 source 的额外实例"统一到 `extra_instances.json`
- **QuotaSource trait 新增**：
  - `instance_index: u32` 字段（11 provider + custom 全实装，Default = 1）
  - `with_instance_index(idx) -> Self` 构造方法（11 provider 全实装）
  - `unique_id() -> String` —— 含 `#N` 后缀（`"minimax#2"`），**给 poller `next_fetch` map key / 浮窗 DOM `data-source-id` 用**
  - `id()` 决策 1：永远返 base provider_id（`"minimax"`），多实例共享 base；走 `unique_id()` 做区分
  - `display_name()` 决策 3：渲染时拼 `t!("provider.suffix.dup", n = idx)` = `" #{}"`（中英都带 1 空格）
- **持久化**：`extra_instances.json`（top-level array，结构跟 PR 3 `custom_sources.json` 同款：原子写 + 0600 + parse 失败 backup 到 `.bak.<ts>`）。老 `custom_sources.json` 在 `config::custom_sources::load_or_migrate` 启动时一次性迁移后 rename 成 `.migrated`
- **编号规则（决策 1 紧凑 + 决策 5 按类型内编号）**：
  - 内置 provider 第 1 份 instance_index=1 **不**进 extra_instances.json
  - 副本从 #2 起按 created_at 升序重排（删除中间一个后 `compact_indexes_for` 自动紧凑）
  - 同 provider_id 内紧凑，`minimax#2` 和 `deepseek#2` 可共存（按类型内编号）
  - custom 第 1 份也走 #2 起，但 `unique_id` 仍用 `custom_<uuid>` 不带 `#N`（custom spec.id 已 UUID 唯一）
- **API key 命名**：
  - 内置副本：`api_key_ref = "minimax#2"`（`keys.json` 里的 key 名）
  - custom：`api_key_ref = "custom_<uuid>"`（跟 PR 3 一致，不重命名）
- **AppState 字段**：`custom_sources: Arc<RwLock<Vec<ExtraInstance>>>` 重命名为 `extra_instances`（type 跟字段名一起改，PR 1a 临时改 type、PR 1b 同步 rename）
- **6 个新 IPC**（PR 3 的 5 个 `commands::custom_sources::*` 整文件删除）：
  - `list_extra_instances` / `add_extra_instance` / `update_extra_instance` / `delete_extra_instance` / `list_picker_providers` / `test_extra_instance`
  - DTO 加 `#[serde(rename_all = "camelCase")]` —— Tauri 2 对 struct 字段也走 camelCase 转换，不加会报 "missing required key providerId"
- **Poller**：`next_fetch` map key 改 `unique_id()`（PR 1a fix：否则 `minimax#2` 会覆盖 `minimax` entry）
- **UI 关键变更**：
  - 「+ 添加自定义来源」→「+ 添加新来源」（按钮文案 + modal 标题）
  - modal 改成**两段式**：Step 1 provider picker（11 内置下拉 + 1 custom 选项）/ Step 2 内置只填 key / custom 3 选 1 Extract 模板
  - 内置行 header 加 ⎘ **复制按钮**（在 `enabled` checkbox 左边，弹 modal 时 picker **预选当前 provider**）
  - extra 行（副本 / custom）header 加 🗑️ 删除按钮（二次确认 prompt 输入含 `#N` 的完整 display_name）
  - `src/settings/custom-source-form.ts` → `extra-instance-form.ts`（重命名 + 重写两段式）
- **i18n 关键变更**：
  - 后端 `commands.extra.*` 6 个错误 key
  - 后端 `provider.suffix.dup = " #{}"`（中英都带空格，i18n 后缀）
  - 前端 `extra.*` modal / err / added 段
  - 前端 `delete_extra.*` 6 个 key（替 PR 3 的 `delete_custom.*`）
  - 前端 `add_source` 段（替 PR 3 的 `add_custom`）
  - **前端 i18n 独立维护 `provider_name.*` 11 项**(后端 `src-tauri/locales/` 也有;v0.2.0 follow-up commit 4 后端 `list_picker_providers` 直接返翻译好的 `display_name` 字符串,前端 `provider_name.*` 镜像已删,单一来源 = 后端 `src-tauri/locales/{en,zh-CN}.json`)
- **迁移 + 兼容**：
  - PR 1a：老 `custom_sources.json` 启动自动迁移到 `extra_instances.json`
  - PR 1b：5 个老 IPC 删除，**前端必须**用新 6 个 IPC（前端 `src/settings/api.ts` 已同步）
  - **v0.2.0 follow-up commit 2**:`load_or_migrate` 从 `config/custom_sources.rs` 内联到 `src-tauri/src/lib.rs`,wrapper 文件删除（v0.2.0 清理点 2 天后老用户的 `custom_sources.json` 已被启动 rename 成 `.migrated`,wrapper 已无 active caller）
- **PR 1b 后 12 provider 全实装**:`minimax` / `deepseek` / `xiaomimimo` / `tavily` / `zenmux` / `openrouter` / `kimi` / `zhipu` / `stepfun` / `siliconflow` / `claude_official` / `custom` 全部加了 `instance_index: u32` 字段 + `with_instance_index(idx)` 方法 + `unique_id()` 返回 `"<base>#N"` 格式(PR 1b 落地,2026-06-24 验证无遗漏)
- **多 instance 一致识别**:`unique_id()` 在 poller next_fetch map key / 浮窗 DOM `data-unique-id`(v0.2.0 follow-up commit 3 把原 `data-source-id` 改过来)/ 托盘 tooltip `#N` 后缀(commit 5)/ 后端 ProviderSnapshot 字段都共用,做到"一份字符串单一来源"

## 已知风险

- 🟡 MiniMax 6/1 改 schema，新字段名未明
- 🟡 GNU 工具链 + Tauri 2 在 Windows 的稳定性需要实战验证
- 🟡 托盘图标的字体加载需要 `assets/font.ttf`，缺了就只有色块

## 下一步建议（v0.3 候选）

1. **Claude cookie 一键重登**：仿 Xiaomi 一键登录的 WebView 方案，研究 Claude 官方 cookie 抓取路径
2. **monitor hotplug 监听**：拔插副屏时实时重新判定浮窗位置
3. **错误卡"忽略本次错误"按钮**：P2-A-7 增量 2/3
4. **Frontend 单元测试 4 核心函数**：contentFingerprint / render / updateCard / autoResizeWindow
5. **~~`http_status_to_error_kind` helper~~ → 已落地为 `classify_http_status`**（[providers/mod.rs](src-tauri/src/providers/mod.rs)，2026-07-02 audit L1 fix；kimi 先用）：剩 10 个 provider 各自保留具体 msg 短路，全面推广留 v0.3
6. **`refresh_inner` 每次 `Box::new` 12 个 source 优化**：按 `Arc<dyn QuotaSource>` 缓存避免每 tick 重建（参考 [memory/source-instance-rebuild-footgun](memory/source-instance-rebuild-footgun.md)）
7. **Backoff 状态持久化到 disk**：重启后 30min 退避归零是用户痛点
8. **Per-provider poller task shutdown signal**：App 退出时不泄漏 task
9. **`delete_extra_instance` v2**：改 instance_index 时同步重命名 keys.json 里的 key
10. **i18n 收尾**：`types.ts` 6 行 / `credentials.ts:307/387` 等 <5% 残留硬编码中文（`updater.ts` 已随自动更新删除）

## i18n 约定（P0-P2 已铺好，详见 `memory/musage-i18n-conventions.md`）

**双 locale 架构**：
- 后端走 rust-i18n（`src-tauri/locales/{en,zh-CN}.json` + `t!()` macro，编译期展开）
- 前端走自写 helper（`src/i18n/index.ts` + `src/i18n/{en,zh-CN}.json` + `t()` 函数）

**Key 命名规范**：
- 语义命名（`button.save` 不是 `Save`），`.` 分层
- 错误消息用 `error.<kind>`（kind 跟 `ErrorKind::as_str()` 对齐）
- plural 用 key 后缀：`footer.count.one` / `footer.count.other`，中文不分单复数所以只有一个 `footer.count`

**三处 PROVIDER_META 合一**：
- ~~`src/settings/utils.ts:providerDisplay`~~ — 删
- 单一来源：[`src/main.ts:37`](src/main.ts#L37) + 共享 [`src/i18n/{en,zh-CN}.json`](src/i18n/) 的 `provider.<id>.name`
- 加新 provider 改 3 处（后端 `providers/<id>.rs` + `providers/mod.rs::builtin_sources()` 注册 + `i18n.json` 的 `provider.<id>.name`）。**注**:settings 面板主结构是动态的（PR 3 改完），但 `src/settings/source-extras.ts` 里的 7 个 `renderXxx` 函数（每个有 region/mode/extras 字段的 provider 1 个）+ `EXTRAS` 表注册**不**是零行 — 真正零行修改的场景是"无 region/mode/extras 字段的纯 Bearer provider"
- **PR 1b 偏离 → v0.2.0 follow-up commit 4 解决**：picker modal 走后端返 display_name 字符串,前端 `src/i18n/*.json` 的 `provider_name.*` 11 项镜像删除。单一来源 = 后端 `src-tauri/locales/{en,zh-CN}.json`

**Locale 切换链路**：
1. 前端 `setLocale(locale)` → 调 `set_app_locale` Tauri command
2. 后端 `rust_i18n::set_locale()` + `cfg.locale` 持久化 + emit `musage://locale-changed`
3. 后端 listener → `tray::rebuild_tray()` 重建菜单 + 同步窗口 title
4. 前端 listener → 重新 `applyDataI18n()` 遍历 `[data-i18n]` 元素

**踩过的坑（详见 memory）**：
- rust-i18n 3.x 不接受 `features = ["json"]`（默认支持）
- `#[tauri::command]` + 同名函数在 lib.rs 顶层会触发 `__cmd__xxx` macro 重复定义（放子模块）
- 子模块用 `t!()` 需 `use crate::t;`（不能 `use rust_i18n::t;`）
- **Tauri 2 `#[tauri::command]` 对 struct 字段也走 camelCase 转换**：DTO 必须加 `#[serde(rename_all = "camelCase")]`，否则前端 `invoke("foo", { req: { provider_id: "x" } })` 会被后端报 "missing required key providerId"（PR 1b 实测）
- **JSON i18n 文件双引号坑**：中文字符串里用 `"内置"` 直接写会提前结束 string（position 19xxx 报错）。改用全角引号 `『内置』` 或 `\"` 转义

## 关键文件链接（按重要性）

- **核心 schema 解析**：`src-tauri/src/providers/minimax.rs`（兼容 2026-06-01 前后的两种 schema）
- **Provider 实现**：`src-tauri/src/providers/{minimax,deepseek,xiaomi,tavily,zenmux,openrouter,kimi,zhipu,stepfun,siliconflow,claude_official}.rs`（PR 1b：11 provider + custom 全加 `instance_index` + `with_instance_index` + override `unique_id` / `display_name`）
- **Extra instance 持久化**（PR 1b）：`src-tauri/src/config/extra_instances.rs`（`ExtraInstance` + `load` / `save` / `next_index_for` / `compact_indexes_for`，9 单测）
- **Extra instance IPC**（PR 1b）：`src-tauri/src/commands/extra_instances.rs`（6 IPC + DTOs；DTO `#[serde(rename_all = "camelCase")]`）
- **Provider 注册 + all_sources**：`src-tauri/src/providers/mod.rs`（`builtin_sources()` + `instantiate_builtin_with_index()` 11 provider 全实装）
- **托盘 UI**：`src-tauri/src/tray.rs`（合并了原 icon.rs：动态图标 + 文字绘制 + 菜单 + tooltip；PR 3 待做：tooltip 多实例聚合）
- **悬浮窗 UI**：`src/main.ts` + `src/styles.css`（PR 3 待做：`data-source-id` 改 `unique_id`）
- **设置面板**：`src/settings/main.ts` + `settings.html`（PR 1b 后 21 个文件：`extra-instance-form.ts` 替 `custom-source-form.ts`）
- **Logo 资源**：`src/assets/` + `src/assets.d.ts`（含 `?url` 声明，必读 [浮窗 logo 打包后裂开](#浮窗-logo-打包后裂开2026-06-13-修)）

## 关键记忆

跨会话保留的踩坑经验写到了 [musage-build-quirks.md](memory/musage-build-quirks.md)：
- WiX 镜像不可达 → 用 NSIS
- `@types/node` 必装
- pnpm shim 放在 cargo/bin
- CARGO_HOME 必须 cargo root
- MinGW dlltool 必在 PATH
- Tauri CSP + Vite assetsInlineLimit 兼容性（导致 Tavily/ZenMux 裂开）
