# Musage

> **Mu**(sage) = **Mu**xima + **Usage**，MiniMax Token Plan 实时用量监控的桌面悬浮窗

![windows](https://img.shields.io/badge/platform-Windows-blue) ![tauri](https://img.shields.io/badge/Tauri-2-orange) ![rust](https://img.shields.io/badge/rust-1.96+-orange)

## 为什么做

ccswitch 3.16 的 MiniMax Token Plan 模板在 **2026-06-01 MiniMax 改 schema 后失效**。
切到 ccswitch 应用里看又繁琐，所以做了这个**常驻悬浮窗** + **托盘图标**：
- 桌面右上一个**小卡片**，实时显示 5h / 周用量 + 重置时间
- 任务栏托盘**动态图标**，颜色随用量变（绿/橙/红）
- **只需 API Key**，不依赖浏览器 session

> **2026-06-10 更新**：参考 ccswitch PR #3518 实现了 percent-based 新 schema 解析，详见下方 "关键 API" 章节。

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

## 技术栈

| | |
|---|---|
| 框架 | Tauri 2 |
| 后端 | Rust + tokio + reqwest |
| 前端 | Vanilla TypeScript + Vite |
| 密钥存储 | 本地 `keys.json` 文件（Unix 0600 权限，原子写）。原 keyring 方案在 macOS 上启动会弹 Keychain 访问窗 + 解锁密码框 |
| 托盘 | image + imageproc + ab_glyph 动态绘制 |

## 准备工作

**1. Rust 工具链**（项目用 GNU 版，**不需要 MSVC Build Tools**）

```bash
# 装 rustup 后:
rustup default stable-x86_64-pc-windows-gnu
```

**2. Node.js ≥ 20 + pnpm**

```bash
node --version   # 应 ≥ v20
pnpm --version
```

**3. WebView2**

Windows 11 自带；Windows 10 需要装一次（系统会自动提示）

## 启动

```bash
# 1. 装前端依赖
pnpm install

# 2. 开发模式（带热重载）
pnpm tauri:dev

# 3. 首次运行会弹出设置面板，填入 API key + 选区域
#    API key 以 0600 权限存到 keys.json（不弹 Keychain 窗）
```

## CLI 探针（定位 6/1 后新 schema）

```bash
# 在 src-tauri/ 下：
cargo run -- dump
```

会打印：
- 原始响应 JSON（用于肉眼对比新字段名）
- 解析结果

## 打包

```bash
pnpm tauri:build
# 产出：
#   src-tauri/target/release/bundle/msi/*.msi
#   src-tauri/target/release/bundle/nsis/*.exe
```

## 项目结构

```
D:\Codes\Musage\
├── CLAUDE.md                # 项目交接文档（新 Claude 会话必读）
├── package.json
├── tsconfig.json
├── vite.config.ts
├── index.html               # 悬浮窗入口
├── settings.html            # 设置面板
├── src/                     # 前端 TS
│   ├── main.ts              # 悬浮窗逻辑
│   ├── settings.ts          # 设置面板逻辑
│   └── styles.css
├── src-tauri/               # Rust 后端
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── capabilities/default.json
│   ├── icons/               # 占位图标（用 scripts/generate_icons.py 生成）
│   ├── assets/              # 可选 font.ttf（托盘文字）
│   └── src/
│       ├── main.rs          # 入口
│       ├── lib.rs           # Tauri Builder + CLI 分流
│       ├── api.rs           # ★ 核心：用量拉取 + 宽容 schema 解析
│       ├── poller.rs        # tokio 后台轮询
│       ├── tray.rs          # 托盘菜单 + 动态图标（合并了原 icon.rs）
│       ├── config.rs        # 配置 + keys.json 文件存储
│       └── commands.rs      # tauri::command 暴露给前端
└── scripts/
    └── generate_icons.py    # 占位图标生成器（Pillow）
```

## 关键 API

```
GET https://api.minimaxi.com/v1/api/openplatform/coding_plan/remains
Authorization: Bearer <api_key>
```

### Schema 历史

**2026-06-01 之前（count-based 旧 schema）** — 已对 Plus 订阅者失效（旧字段全为 0）：

```json
{
  "base_resp": { "status_code": 0, "status_msg": "success" },
  "model_remains": [{
    "current_interval_total_count": 200,
    "current_interval_usage_count":  56,    // 字段名"usage"实为"剩余"
    "end_time": 1748500000000,             // epoch ms
    "current_weekly_total_count": 300,
    "current_weekly_usage_count": 264,
    "weekly_end_time": 1749000000000
  }]
}
```

**2026-06-01 之后（percent-based 新 schema，参考 ccswitch PR #3518）**：

```json
{
  "base_resp": { "status_code": 0, "status_msg": "success" },
  "model_remains": [{
    "model_name": "general",                              // 取这一条
    "current_interval_remaining_percent": 72,             // 5h 剩余%
    "current_interval_status": 1,                         // 5h 状态（==1 有效）
    "end_time": 14523,                                    // ⚠ 距离重置的**秒数**（不是 epoch ms）
    "current_weekly_remaining_percent": 86,               // 周剩余%
    "current_weekly_status": 1,                           // 周状态（==1 有效；2/3 = 不在套餐）
    "weekly_end_time": 803245                             // ⚠ 同上，秒数
  }]
}
```

### 解析策略（`src-tauri/src/providers/minimax.rs`）

1. 从 `model_remains[]` 优先选 `model_name == "general"`，找不到则取第一条
2. **先试新 schema（percent-based）** — 5h / 周各自独立 gate on `status == 1`
3. 失败回退到旧 schema（count-based）作为兼容
4. `resets_at` 智能识别：值在 `[10^12, 4*10^12]` 范围当 epoch ms，否则当 duration-seconds 加到 now

### 百分比公式

- **新**：`utilization = 100 - *_remaining_percent`
- **旧**：`utilization = (total - remaining) / total * 100`

### 已知坑（参考 MiniMax-M2 #99, cli #165, cli #173）

- `*_remaining_percent=100` 不代表 "还有 100%"，可能是 `status=2/3`（不在套餐内）→ 必须 gate on status
- 旧字段对 Plus 订阅者全为 0，旧路径会得到空快照
- `end_time` 单位从 epoch ms 改成 seconds，需要 `now_ms + v * 1000` 换算

## 故障排查

| 现象 | 原因 | 解决 |
|---|---|---|
| `cargo build` 报 `link.exe not found` | 默认是 MSVC 工具链 | `rustup default stable-x86_64-pc-windows-gnu` |
| `cargo build` 报 `export ordinal too large: 141874` | MinGW ld 16-bit 导出表被撑爆（cdylib 自动生成 .def 含 14 万符号） | `Cargo.toml` 的 `[lib]` 用 `crate-type = ["staticlib", "rlib"]`（**不要**用 `RUSTFLAGS=-Wl,--no-export-all-symbols`，会污染 build script）|
| 托盘图标没有百分比文字 | 缺 `src-tauri/assets/font.ttf` | 丢一个 TTF 字体进去，或不管（色块也够用）|
| 悬浮窗"测试连接"报 401 | API key 错 | 检查 key 是不是 `sk-cp-...` 开头 |
| 拉不到数据 / `未返回结果` | MiniMax 改了 schema | 0.5.0+ 已实现 percent-based 新 schema。如仍失败：`cargo run -- dump` 看新字段，[开 issue](https://github.com/farion1231/cc-switch) |
| macOS / Linux 报错 | 工具链不同 | `rustup default stable-{aarch64,x86_64}-apple-darwin` 等 |

## License

MIT
