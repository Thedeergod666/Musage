# Musage

> **Musage** = **My** + **Usage**，多 provider AI 套餐实时用量监控的桌面悬浮窗

![platforms](https://img.shields.io/badge/platforms-Windows%20%7C%20macOS%20%7C%20Linux-blue) ![tauri](https://img.shields.io/badge/Tauri-2-orange) ![rust](https://img.shields.io/badge/rust-1.77+-orange) ![license](https://img.shields.io/badge/license-MIT-green)

## 为什么做

ccswitch 3.16 的 MiniMax Token Plan 模板在 **2026-06-01 MiniMax 改 schema 后失效**。
切到 ccswitch 应用里看又繁琐，所以做了这个**常驻悬浮窗** + **托盘图标**，能盯 **11 内置 + 任意 New API 中转站** 的实时用量：

- 桌面右上**小卡片**，实时显示每个 provider 的用量 + 重置时间
- 任务栏托盘**动态图标**，颜色随用量变（绿/橙/红）
- **只需 API Key / Cookie**，不依赖浏览器 session
- 支持**同 provider 多实例**（同时持 2 个 MiniMax 套餐 / 多个 New API 中转站时一次看完）

## 形态

```
┌─────────────────────────┐
│ 5h        144%         │  ← 5h 限额 + 已用百分比
│ ████████████░░░░░░░░░  │  ← 进度条
│ 5h 重置 14:23          │  ← 倒计时
│                         │
│ 周          36%         │  ← 周限额
│ ███░░░░░░░░░░░░░░░░░░  │
│ 周重置 周三 18:00       │
│ ─────────────────────── │
│ 🇨🇳 CN · 拖动 · 右键   │
└─────────────────────────┘
```

托盘图标（任务栏右下）：

```
  [圆]    ← 灰色 = 启动中
 [144]    ← 中心数字 = 5h 已用百分比缩写
  [圆]    ← 颜色：绿<70% / 橙70-90% / 红>=90%
```

## 支持的 quota source

### 11 个内置 provider

| Provider | 鉴权 | 数据 | 说明 |
|---|---|---|---|
| **MiniMax** Token Plan | Bearer | 5h / 周用量 + 重置 | 支持 2026-06-01 前后两套 schema |
| **DeepSeek** | Bearer | 余额 | |
| **Xiaomi MiMo** | Bearer / Cookie | 套餐 + 总额度 | 应用内 WebView 一键登录抓 cookie |
| **Tavily** | Bearer | Credits | |
| **ZenMux** | Bearer | PAYG / Subscription | |
| **OpenRouter** | Bearer | 余额 | |
| **Kimi (Moonshot Coding)** | Bearer | 套餐用量 | |
| **智谱 GLM** | Bearer | 套餐用量 | 国内/国际 endpoint 切换 |
| **StepFun** | Oasis-Token | Step Plan 套餐 | |
| **SiliconFlow** | Bearer | 钱包余额 | |
| **Claude 官方** (Pro/Max) | Cookie | OAuth 用量 | |

### 任意 New API 中转站

通过 **「+ 添加新来源」** 自定义（CustomSource），支持 3 个 Extract 模板：

1. **New API 系**（dmxapi / byteplus / lemondata / ctok / silicon / crazyrouter / cubence / dds / runapi / ucloud / shengsuanyun 等）—— 写死 `data.quota / data.used_quota`
2. **余额系** —— 用户填 `balance_path` JSONPath
3. **自定义** —— 3 个独立 JSONPath

UUID 持久化，1 次配置永久使用。

### 多实例（v0.2 起）

同 provider 可以加**任意份副本**，每份独立 API key + 独立 quota 显示。例如：

- 2 个 MiniMax 套餐 → 浮窗显示 `MiniMax 5h 45%` + `MiniMax #2 5h 12%`
- 1 个官方 + 2 个中转 → 浮窗按顺序排，托盘 tooltip 拼 `#N` 后缀

## 技术栈

| | |
|---|---|
| 框架 | Tauri 2 |
| 后端 | Rust + tokio + reqwest (rustls) |
| 前端 | Vanilla TypeScript + Vite（无框架，极小） |
| 异步运行时 | tauri::async_runtime（windows 端 v0.1 sleep bug 修过） |
| 密钥存储 | 本地 `keys.json` 文件（Unix 0600 权限，原子写）。原 keyring 方案在 macOS 启动会弹 Keychain 访问窗 + 解锁密码框 |
| 托盘 | image + imageproc + ab_glyph 动态绘制（color + 百分比数字） |
| i18n | 后端 rust-i18n + 前端自写 helper，en + zh-CN，运行时切换 |
| 自动启动 | tauri-plugin-autostart |
| 系统通知 | tauri-plugin-notification（v0.2 起，cookie 失效提醒） |
| 自动更新 | tauri-plugin-updater（GitHub release 签名 manifest） |

## 准备工作

**1. Rust 工具链**（Windows 推荐 GNU 版，**不需要 MSVC Build Tools**）

```bash
# Windows:
rustup default stable-x86_64-pc-windows-gnu

# macOS:
rustup default stable-aarch64-apple-darwin   # 或 x86_64
```

**2. Node.js ≥ 20 + pnpm**

```bash
node --version   # 应 ≥ v20
pnpm --version
```

**3. WebView2**

Windows 11 自带；Windows 10 需要装一次（NSIS 安装包会自动处理）

## 启动

```bash
# 1. 装前端依赖
pnpm install

# 2. 开发模式（带热重载）
pnpm tauri:dev

# 3. 首次运行会弹出设置面板，填入 API key + 选区域
#    API key 以 0600 权限存到 keys.json（不弹 Keychain 窗）
```

## CLI 探针

```bash
# 在 src-tauri/ 下：
cargo run -- dump
```

会打印所有 source 的原始响应 JSON + 解析结果（用于排查 schema 变更）。

## 打包

```bash
pnpm tauri:build
# 产出（macOS aarch64+x64 dmg, Windows NSIS exe + MSI, Linux AppImage/deb/rpm）：
#   src-tauri/target/release/bundle/dmg/Musage_*_aarch64.dmg
#   src-tauri/target/release/bundle/dmg/Musage_*_x64.dmg
#   src-tauri/target/release/bundle/nsis/Musage_*_x64-setup.exe
#   src-tauri/target/release/bundle/msi/Musage_*_x64_en-US.msi
#   src-tauri/target/release/bundle/appimage/Musage_*_amd64.AppImage
#   src-tauri/target/release/bundle/deb/musage_*_amd64.deb
#   src-tauri/target/release/bundle/rpm/musage-*.x86_64.rpm
```

**Linux 安装**（Ubuntu 22.04+ / Debian 12+）：

```bash
# 通用：AppImage 免安装,双击即可
chmod +x Musage_*_amd64.AppImage
./Musage_*_amd64.AppImage

# Debian/Ubuntu: 双击 deb 或
sudo apt install ./musage_*_amd64.deb

# Fedora/RHEL:
sudo dnf install ./musage-*.x86_64.rpm
```

**GNOME 桌面额外步骤**：系统托盘需要装 [AppIndicator extension](https://extensions.gnome.org/extension/615/appindicator-support/) 才能在顶部状态栏看到 Musage 托盘。其它桌面环境（KDE / XFCE / Cinnamon）开箱即用。

## 项目结构

```
Musage/
├── AGENTS.md                # 项目交接文档（新 AI 会话必读）
├── README.md                # 本文件
├── CHANGELOG.md             # 版本变更日志
├── ROADMAP.md               # 当前路线图
├── FUTURE.md                # 暂缓 / 砍掉的想法
├── RELEASING.md             # 维护者发版 cheat sheet
├── package.json             # pnpm workspace
├── tsconfig.json / vite.config.ts
├── index.html               # 悬浮窗入口
├── settings.html            # 设置面板入口
├── src/                     # 前端 TS（vanilla，无框架）
│   ├── main.ts              # 悬浮窗逻辑（拖动 / 订阅 / 渲染 / i18n）
│   ├── settings.ts          # 设置面板入口
│   ├── settings/            # 设置面板 21 个子模块
│   │   ├── api.ts / config.ts / types.ts / utils.ts
│   │   ├── credentials.ts / providers.ts / floating.ts
│   │   ├── order.ts / logs.ts / test.ts / about.ts
│   │   ├── updater.ts / advanced.ts
│   │   ├── extra-instance-form.ts   # + 添加新来源（两段式 picker）
│   │   ├── groups.ts                # 6 组分组（token_plan/balance/official/xiaomi/custom/misc）
│   │   ├── modal.ts                 # 原生 <dialog> 包装
│   │   ├── region-wizard.ts         # 多 region provider 区域选择向导
│   │   └── ...
│   ├── i18n/                # 前端 i18n (en.json / zh-CN.json)
│   └── assets/              # provider logo
└── src-tauri/               # Rust 后端
    ├── Cargo.toml
    ├── tauri.conf.json
    ├── entitlements.plist   # macOS Hardened Runtime entitlement
    ├── locales/             # 后端 i18n (en.json / zh-CN.json)
    ├── icons/               # 32/128/ico/icns/128@2x/tray-base
    ├── assets/              # 可选 font.ttf（托盘文字）
    └── src/
        ├── main.rs / lib.rs / poller.rs / tray.rs / config.rs
        ├── poller_backoff.rs          # per-provider 指数退避
        ├── xiaomi_login.rs            # Xiaomi 一键登录 WebView
        ├── logstore.rs                # 内存日志环形缓冲（设置面板 logs section）
        ├── commands/                  # Tauri IPC commands
        │   └── extra_instances.rs     # 6 IPC (list/add/update/delete/list_picker/test)
        ├── config/                    # 持久化
        │   └── extra_instances.rs     # ExtraInstance + load/save/compact_indexes
        ├── providers/                 # 12 provider (11 内置 + custom)
        │   ├── mod.rs                 # QuotaSource trait + builtin_sources 注册表
        │   ├── parse.rs               # JSONPath + num_f64 helper
        │   ├── custom.rs              # CustomSource + CustomSourceSpec
        │   └── minimax/deepseek/xiaomi/tavily/zenmux/openrouter/
        │       kimi/zhipu/stepfun/siliconflow/claude_official.rs
        └── platform/                  # 平台特定代码
            └── macos.rs               # PinBottom 走 NSWindow.setLevel(-1)
```

## 关键 API 模式

### MiniMax（corss-switch PR #3518 后）

```
GET https://api.minimaxi.com/v1/api/openplatform/coding_plan/remains
Authorization: Bearer <api_key>
```

返回 percent-based 新 schema（`current_interval_remaining_percent` / `current_interval_status`）+ count-based 老 schema 双兼容。详见 [`src-tauri/src/providers/minimax.rs`](src-tauri/src/providers/minimax.rs)。

### 其他 provider

各 provider 端点 + schema 见 `src-tauri/src/providers/<id>.rs`。所有 provider 实现 `QuotaSource` trait：
- `id()` → base provider_id（多实例共享）
- `unique_id()` → `"<id>#N"`（多实例区分）
- `display_name()` → `Cow<str>`（内置走 `t!()`，custom 走 spec.display_name）
- `do_fetch(creds, source_id, display_name)` → `Result<ProviderSnapshot>`
- `instance_index: u32`（v0.2 字段，默认 1，副本用 `with_instance_index` 构造）

新增一个 provider 只需要 3 步（不用改 commands.rs 的 match）：
1. `src-tauri/src/providers/<id>.rs` 写 `XxxSource: QuotaSource`
2. `builtin_sources()` 注册表加 `Box::new(XxxSource::default())`
3. `src-tauri/locales/{en,zh-CN}.json` 加 `provider_name.<id>`

## 故障排查

| 现象 | 原因 | 解决 |
|---|---|---|
| `cargo build` 报 `link.exe not found` | 默认是 MSVC 工具链 | `rustup default stable-x86_64-pc-windows-gnu` |
| `cargo build` 报 `export ordinal too large: 141874` | MinGW ld 16-bit 导出表被撑爆（cdylib 自动生成 .def 含 14 万符号） | `Cargo.toml` 的 `[lib]` 用 `crate-type = ["staticlib", "rlib"]` |
| 托盘图标没有百分比文字 | 缺 `src-tauri/assets/font.ttf` | 丢一个 TTF 字体进去，或不管（色块也够用）|
| 悬浮窗"测试连接"报 401 | API key 错 | 检查 key 前缀（`sk-cp-` / `tp-` / `tvly-` 等） |
| 拉不到数据 / `未返回结果` | MiniMax 改了 schema | v0.2 已实现 percent-based 新 schema；仍失败：`cargo run -- dump` 看新字段 |
| macOS 弹窗「应用已损坏」 | 未签名 + quarantine xattr | 装好后跑 `xattr -cr /Applications/Musage.app && codesign --force --deep --sign - /Applications/Musage.app`，或 [RELEASING.md](RELEASING.md) 配 Apple Developer ID 走真签名 + 公证 |
| macOS 跑起来 UI 全是 `provider.minimax.name` / `settings.nav.providers` 这种 raw key | v0.2.1 的 frontend i18n bundle bug（Vite 动态 import 模板字符串没生成 chunk，dicts 永远是空）。**v0.2.2 已修** | 升 v0.2.2 即可；如果还看到 raw key 说明 dist 没重建，跑 `pnpm build && pnpm tauri build` |
| Linux 报错 | Tauri 2 Linux 工具链可能未装全 | `rustup default stable-x86_64-unknown-linux-gnu`，再加 webkit2gtk-4.1 dev pkg |

## License

MIT — see [LICENSE](LICENSE) file.

Copyright (c) 2026 Thedeergod666

## Acknowledgements

API schema parsing (MiniMax `coding_plan/remains` percent-based & count-based, DeepSeek `user/balance`) was reverse-engineered and adapted from [**farion1231/cc-switch**](https://github.com/farion1231/cc-switch) (MIT, Copyright (c) 2025 Jason Young). No code was copied — only schema field names, semantics, and the `isValid`-style error classification pattern were referenced.
