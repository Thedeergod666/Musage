# Musage 发布流程

> 给维护者（你自己）的一份 cheat sheet：怎么生成签名密钥、怎么配 GitHub Secrets、怎么发新版本、怎么排查。

---

## 1. 架构概览

```
┌──────────────┐  ① git push v0.2.0   ┌──────────────────┐
│   维护者     │ ────────────────────→│  GitHub Actions   │
│  (你)        │                       │  release.yml      │
└──────────────┘                       └────────┬─────────┘
                                                │ ② tauri build × 3
                                                │   (mac arm64/x64, win)
                                                ▼
                                       ┌──────────────────┐
                                       │  Bundle 输出     │
                                       │  (.dmg/.msi/.exe)│
                                       └────────┬─────────┘
                                                │ ③ 用私钥签 manifest
                                                │    (tauri-action 自动)
                                                ▼
                                       ┌──────────────────┐
                                       │  GitHub Release  │
                                       │  (Draft)         │
                                       │  + latest.json   │
                                       └────────┬─────────┘
                                                │
            ┌───────────────────────────────────┘
            │ ④ 用户打开旧版 Musage
            ▼
   ┌─────────────────┐
   │  静默检查       │  启动 5s 后调 checkForUpdate(true)
   │  (main.ts)      │
   └────────┬────────┘
            │ ⑤ 比对版本
            ▼
   ┌─────────────────┐         ┌──────────────────┐
   │  拉到 latest.json│────────→│ 设置面板 banner   │
   │  + 验签          │         │ "发现新版本 v0.2.0"│
   └────────┬────────┘         └────────┬─────────┘
            │                            │ 点"下载并安装"
            ▼                            ▼
   ┌──────────────────────────────────────────┐
   │  downloadAndInstall()                    │
   │  - 流式下载 + 进度回调                    │
   │  - 验签每个 chunk                        │
   │  - NSIS/MSI: passive 模式安装             │
   │  - macOS: 替换 .app                      │
   └────────┬─────────────────────────────────┘
            │
            ▼
   ┌─────────────────┐
   │  relaunch()     │  用户点"立即重启"
   └─────────────────┘
```

---

## 2. 首次配置（一次性）

### 2.1 生成 Tauri updater 签名密钥对

需要装 [Tauri CLI](https://tauri.app/start/cli/)：

```bash
cargo install tauri-cli --version "^2.0.0" --locked

# 生成 keypair
#  - 私钥存到本地（绝对不要 commit！已加 .gitignore）
#  - 公钥会打印在 stdout，复制到 tauri.conf.json
tauri signer generate -w ~/.tauri/musage.key -p <设个密码>

# 输出长这样：
#   Public Key: dW50cnVzdGVkIGNvbW1lbnQ6...
#   Private Key: /Users/you/.tauri/musage.key
#   Private Key Password: <你设的密码>
```

### 2.2 把公钥塞进 tauri.conf.json

```bash
# src-tauri/tauri.conf.json
{
  "plugins": {
    "updater": {
      "endpoints": [
        "https://github.com/Thedeergod666/Musage/releases/latest/download/latest.json"
      ],
      "pubkey": "dW50cnVzdGVkIGNvbW1lbnQ6...",   ← 填这里
      "windows": { "installMode": "passive" }
    }
  }
}
```

⚠️ **公钥 commit 没事，私钥绝不能进 git**。`.gitignore` 已加 `*.tauri.key` / `musage.key` / `tauri-updater-key*`。

### 2.3 把私钥 + 密码配成 GitHub Actions Secrets

仓库页 → **Settings** → **Secrets and variables** → **Actions** → **New repository secret**

| Secret 名称 | 值 | 说明 |
|---|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` | `cat ~/.tauri/musage.key \| base64 -w 0`（一行） | base64 编码的私钥，**单行** |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | 你设的密码 | 私钥解密密码 |

把私钥变成单行 base64 的命令：

```bash
# macOS / Linux
base64 -i ~/.tauri/musage.key | tr -d '\n' | pbcopy   # mac 直接进剪贴板
# Windows (PowerShell)
[Convert]::ToBase64String([IO.File]::ReadAllBytes("$env:USERPROFILE\.tauri\musage.key")) | Set-Clipboard
```

> **不要**把 `~/.tauri/musage.key` 本身 commit 或粘贴到 secrets 之外的任何地方。

### 2.4（可选）macOS 签名 + 公证

需要 Apple Developer 账号（$99/年）。

```bash
# 1. 导出 Developer ID Application 证书为 .p12
#    (Keychain Access → 找到证书 → 右键 Export)

# 2. base64 编码
base64 -i Certificates.p12 | tr -d '\n' > cert.b64

# 3. 创建 App-specific password
#    https://appleid.apple.com → App-Specific Passwords
```

配这些 secrets：

| Secret | 值 |
|---|---|
| `APPLE_CERTIFICATE` | .p12 的 base64（单行） |
| `APPLE_CERTIFICATE_PASSWORD` | .p12 导出时设的密码 |
| `APPLE_SIGNING_IDENTITY` | `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_ID` | 你的 Apple ID 邮箱 |
| `APPLE_PASSWORD` | App-specific password（上一步生成的） |
| `APPLE_TEAM_ID` | 10 位 Team ID |

**没配这 6 个 → 也能构建**，但用户首次打开会卡 Gatekeeper 30 秒并需右键打开。

### 2.5（可选）Windows EV 代码签名

OV 证书触发 SmartScreen 太慢（按下载量累积信誉），**强烈建议 EV**。

| Secret | 值 |
|---|---|
| `WINDOWS_CERTIFICATE` | .pfx 的 base64（单行） |
| `WINDOWS_CERTIFICATE_PASSWORD` | .pfx 密码 |

EV 证书需要物理 USB key（如 DigiCert Keymate），需要用 `tauri-action` 配套的证书管理机制，详见 [tauri-action 文档](https://github.com/tauri-apps/tauri-action#windows-signing)。

**没配这 2 个 → 也能构建**，但 Windows 用户首次运行会卡 SmartScreen "未知发布者" 警告。

---

## 3. 日常发布流程

### 3.1 改版本号

```bash
# 一条命令搞定：改 tauri.conf.json + 同步到 package.json + Cargo.toml
pnpm bump -- 0.2.0

# 检查 diff（3 个文件都改了）
git diff
#  M package.json
#  M src-tauri/Cargo.toml
#  M src-tauri/tauri.conf.json
```

### 3.2 提交 + 打 tag + 推送

```bash
git add -A
git commit -m "chore: bump to v0.2.0"
git tag v0.2.0
git push origin main --tags
```

### 3.3 监控 workflow

`https://github.com/Thedeergod666/Musage/actions` → 选 `Release` workflow → 看三个 job：

```
build (macos-arm64)   ~8 min
build (macos-x64)     ~9 min
build (windows-x64)   ~7 min
verify release assets ~10 s
```

### 3.4 审核 + 发布 Draft

workflow 完事后到 [Releases 页](https://github.com/Thedeergod666/Musage/releases)：
- Draft release 自动创建，含 3 个 bundle + `latest.json`
- 检查 CHANGELOG、产物大小
- 点 **Publish release**

### 3.5 用户收到更新

- 已装旧版的用户：启动 5s 后静默检查 → 拉到 v0.2.0 → 设置面板 banner 提示
- 弹出"下载并安装"按钮
- 下载完（流式 + 进度条）→ 点"立即重启" → relaunch
- **macOS 静默替换** `.app` 内容，无需卸载
- **Windows NSIS 静默升级**（passive 模式），无需卸载
- **Windows MSI** 走 MSI installer，自动跑 minor upgrade

---

## 4. 故障排查

| 现象 | 原因 | 解法 |
|---|---|---|
| `latest.json` 验证失败 | pubkey 跟私钥不匹配 | 重新 `tauri signer generate`，两端都用新值 |
| 用户报"已是最新"但实际有新版本 | `version` 字段没同步 | `pnpm sync-version` 检查；CI 应该已经跑过 |
| Windows 升级后旧配置丢了 | NSIS installMode 没用 passive | tauri.conf.json 的 `windows.installMode: "passive"` |
| macOS Gatekeeper 拦截 | 没签名 + 没公证 | 配 Apple secrets，见 2.4 |
| Windows SmartScreen 警告 | 没 EV 签名 | 配 Windows cert，见 2.5 |
| Workflow 报 "no target" | tag 格式不匹配 | tag 必须是 `vX.Y.Z` 格式（带 v） |
| `pnpm tauri:build` 报 pubkey 错 | pubkey 是占位符 | 按 2.2 填入真实公钥 |

---

## 5. 安全要点

1. **私钥丢失 = 永远发不了新版本**（已签名的 manifest 验证会失败，所有用户都升不上去）
   - 备份到 1Password / Bitwarden 等密码管理器
   - 至少在 2 个不同地方存
2. **私钥泄露 = 立即轮换**（攻击者可签发假更新让用户装木马）
   - `tauri signer generate` 生成新 key
   - 更新 tauri.conf.json 的 pubkey
   - 更新 GitHub secret
   - 发一版"信任切换"更新
3. **Latest.json 必须走 HTTPS**（已配 GitHub Releases，自动 HTTPS）
4. **不要把 endpoint 改成第三方**（如自建 CDN），除非你懂 HSTS + cert pinning

---

## 6. 不想要自动更新？

`src-tauri/tauri.conf.json` 删掉 `plugins.updater` 段即可，编译时不会带 updater 功能。
`src-tauri/Cargo.toml` 可以删 `tauri-plugin-updater` 和 `tauri-plugin-process`。
`src/main.ts` 删掉那段 `setTimeout` + `onUpdateState`。
`src/settings.ts` 删掉 `setupUpdaterSection` 调用 + import。
