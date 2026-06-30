# Musage 发布流程

> 给维护者（你自己）的一份 cheat sheet：怎么改版本号、怎么打 tag、怎么发新版、怎么排查。
>
> **v0.2.0 起不内置自动更新** —— 升级走"用户手动下 dmg/nsis 覆盖装"路径。设置面板「关于」页有 GitHub releases 链接。

---

## 1. 架构概览

```
┌──────────────┐  ① git push vX.Y.Z tag  ┌──────────────────┐
│   维护者     │ ───────────────────────→│  GitHub Actions   │
│  (你)        │                          │  release.yml      │
└──────────────┘                          └────────┬─────────┘
                                                  │ ② tauri build × 3
                                                  │   (mac arm64/x64, win)
                                                  ▼
                                         ┌──────────────────┐
                                         │  Bundle 输出     │
                                         │  (.dmg/.exe)     │
                                         └────────┬─────────┘
                                                  │ ③ tauri-action
                                                  │   上传为 release assets
                                                  ▼
                                         ┌──────────────────┐
                                         │  GitHub Release  │
                                         │  (Draft)         │
                                         └────────┬─────────┘
                                                  │
            ┌─────────────────────────────────────┘
            ▼
   ┌────────────────────┐
   │  用户看到 GitHub    │  浏览器/feed reader 推送
   │  release notification│  → 访问 release 页
   └────────┬───────────┘
            │ 下载 dmg / setup.exe
            ▼
   ┌────────────────────┐
   │  覆盖安装          │  macOS: 拖入 Applications 替换
   │                    │  Windows: NSIS 安装器覆盖
   └────────┬───────────┘
            │
            ▼
   ┌────────────────────┐
   │  手动重启 app      │  (无 relaunch() hook)
   └────────────────────┘
```

**为什么不走 tauri-plugin-updater**: v0.2.0 删了 updater plugin,原因见 [AGENTS.md "v0.2.0 follow-up" 段 + commit `586e55c` 之后那次修复](AGENTS.md)。简单说 —— tauri-action 签 `latest.json` 需要 `TAURI_SIGNING_PRIVATE_KEY` GitHub Secret + 对应密码,维护者没配这套 → windows build 报 "Missing comment in secret key" → 整批 release 挂。**走 manual upgrade 更省事** —— 用户量不大,推送通知 + 显式下载也够用。

---

## 2. 一次性配置

### 2.1（可选）macOS 签名 + 公证

需要 Apple Developer 账号（$99/年）。

```bash
# 1. 导出 Developer ID Application 证书为 .p12
#    (Keychain Access → 找到证书 → 右键 Export)

# 2. base64 编码
base64 -i Certificates.p12 | tr -d '\n' > cert.b64

# 3. 创建 App-specific password
#    https://appleid.apple.com → App-Specific Passwords
```

配这些 secrets（GitHub repo → Settings → Secrets and variables → Actions）：

| Secret | 值 |
|---|---|
| `APPLE_CERTIFICATE` | .p12 的 base64（单行） |
| `APPLE_CERTIFICATE_PASSWORD` | .p12 导出时设的密码 |
| `APPLE_SIGNING_IDENTITY` | `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_ID` | 你的 Apple ID 邮箱 |
| `APPLE_PASSWORD` | App-specific password（上一步生成的） |
| `APPLE_TEAM_ID` | 10 位 Team ID |

**没配这 6 个 → 也能构建**,但用户首次打开会卡 Gatekeeper 30 秒并需右键打开。

### 2.2（可选）Windows EV 代码签名

OV 证书触发 SmartScreen 太慢（按下载量累积信誉），**强烈建议 EV**。

| Secret | 值 |
|---|---|
| `WINDOWS_CERTIFICATE` | .pfx 的 base64（单行） |
| `WINDOWS_CERTIFICATE_PASSWORD` | .pfx 密码 |

EV 证书需要物理 USB key（如 DigiCert Keymate），需要用 `tauri-action` 配套的证书管理机制，详见 [tauri-action 文档](https://github.com/tauri-apps/tauri-action#windows-signing)。

**没配这 2 个 → 也能构建**,但 Windows 用户首次运行会卡 SmartScreen "未知发布者" 警告。

---

## 3. 日常发布流程

### 3.1 改版本号

```bash
# 一条命令搞定：改 tauri.conf.json + 同步到 package.json + Cargo.toml
pnpm bump -- 0.3.0

# 检查 diff（3 个文件都改了）
git diff
#  M package.json
#  M src-tauri/Cargo.toml
#  M src-tauri/tauri.conf.json
```

### 3.2 提交 + 打 tag + 推送

```bash
git add -A
git commit -m "chore: bump to v0.3.0"
git tag v0.3.0
git push origin main --tags
```

> ⚠ **GitHub Actions 不允许同 tag 重跑**。如果 v0.3.0 release workflow 失败需要重跑:
> ```bash
> # 删 tag 远端 + 本地
> git push origin :refs/tags/v0.3.0
> git tag -d v0.3.0
> # 修复后,重新打 tag push
> git tag v0.3.0
> git push origin v0.3.0
> ```
> 或者去 GitHub Actions 页对失败的 run 点 **Re-run all jobs**。

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
- Draft release 自动创建,含 3 个 bundle
- 检查产物大小、CHANGELOG
- 点 **Publish release**

### 3.5 用户收到更新

- v0.2.0 起没有自动通知,用户在设置面板「关于」页看到当前版本 + GitHub releases 链接
- 用户访问 release 页 → 下载 dmg / setup.exe → 覆盖安装
- **macOS**: 拖入 `/Applications` 替换旧版（系统会保留配置）
- **Windows NSIS**: passive 模式升级,无需卸载（也会保留配置）

---

## 4. 故障排查

| 现象 | 原因 | 解法 |
|---|---|---|
| `pnpm tauri:build` 报 "Bundling failed" (Win) | `tauri.conf.json` 缺 macOS entitlement 路径 | 检查 `bundle.macOS.entitlements` 路径对不对（macOS-only 字段不影响 win build,但有时 validator 抱怨）|
| windows build 报 "permission denied" | 权限/sandbox 拦了 tauri build | 看具体 stack trace,通常是 admin 权限或杀毒软件 |
| macOS build 报 "xcrun: error: unable to find utility" | Xcode CLT 没装 | `xcode-select --install` |
| WiX 镜像 timeout (历史问题) | v0.2.0 起 bundle targets 只剩 `["nsis", "dmg"]`,不走 WiX | 不需要再处理 |
| Workflow 报 "no target" | tag 格式不匹配 | tag 必须是 `vX.Y.Z` 格式（带 v） |
| Workflow 报 "failed to decode secret key" | `TAURI_SIGNING_PRIVATE_KEY` secret 没配 | v0.2.0 不再用 updater,这种情况不该再出现 |

---

## 5. 安全要点

1. **私钥丢失 = 永远发不了新版本**（已签名的 manifest 验证会失败，所有用户都升不上去）
   - 备份到 1Password / Bitwarden 等密码管理器
   - 至少在 2 个不同地方存
2. **私钥泄露 = 立即轮换**（攻击者可签发假更新让用户装木马）
   - 重新生成 key
   - 更新 tauri.conf.json 的 pubkey
   - 更新 GitHub secret
   - 发一版"信任切换"更新
3. **Latest.json 必须走 HTTPS**（已配 GitHub Releases，自动 HTTPS）
4. **不要把 endpoint 改成第三方**（如自建 CDN），除非你懂 HSTS + cert pinning

---

## 6. 之前用过 updater?

v0.1.0 / v0.2.0 早期曾用 tauri-plugin-updater,代码已在本 fix 删干净（[commit `586e55c` 之后那次 fix](AGENTS.md)）。如果哪天想加回来,需要:

1. **生成 keypair** (一次性):
   ```bash
   cargo install tauri-cli --version "^2.0.0" --locked
   tauri signer generate -w ~/.tauri/musage.key -p <设个密码>
   # 把输出的 pubkey 填到 tauri.conf.json 的 plugins.updater.pubkey
   ```
2. **配 GitHub Secrets**:
   - `TAURI_SIGNING_PRIVATE_KEY` = `cat ~/.tauri/musage.key | base64 -w 0`（一行）
   - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` = 私钥密码
3. **加回代码** (反向操作本次 fix):
   - `src-tauri/Cargo.toml` 加 `tauri-plugin-updater = "2"`
   - `src-tauri/src/lib.rs` 加 `.plugin(tauri_plugin_updater::Builder::new().build())`
   - `src-tauri/capabilities/settings.json` 加 `"updater:default"`
   - `src-tauri/tauri.conf.json` 加 `plugins.updater` 段
   - `package.json` 加 `@tauri-apps/plugin-updater`
   - 重新建 `src/updater.ts` + `src/settings/updater.ts`
   - `src/main.ts` 加回 5s setTimeout + onUpdateState 订阅
   - `src/settings/main.ts` 加回 `setupUpdaterSection()` 调用
   - `.github/workflows/release.yml` 加回 `TAURI_SIGNING_PRIVATE_KEY` env + `includeUpdaterJson: true`
