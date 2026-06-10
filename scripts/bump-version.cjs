#!/usr/bin/env node
/**
 * 用法：
 *   node scripts/bump-version.cjs 0.2.0
 *   node scripts/bump-version.cjs 0.2.0-beta.1
 *
 * 步骤：
 *   1. 校验新版本号格式
 *   2. 写回 src-tauri/tauri.conf.json（单一来源）
 *   3. 触发 sync-version.cjs 同步到 package.json + Cargo.toml
 *   4. 打印后续 git 命令提示
 *
 * CI 路径（无需走此脚本）：
 *   - workflow 抽 tag → 直接改 tauri.conf.json → 跑 sync-version.cjs
 */

const fs = require("fs");
const path = require("path");
const { execFileSync } = require("child_process");

const ROOT = path.resolve(__dirname, "..");
const CONF = path.join(ROOT, "src-tauri", "tauri.conf.json");

const newVersion = process.argv[2];
if (!newVersion) {
  console.error("用法: node scripts/bump-version.cjs <semver>");
  console.error("示例: node scripts/bump-version.cjs 0.2.0");
  process.exit(1);
}
// 接受 0.2.0 / 0.2.0-beta.1 / 0.2.0-rc.2 之类
if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(newVersion)) {
  console.error(`版本号格式不合法: ${newVersion}`);
  console.error("应是 semver，例如 0.2.0 / 1.0.0-rc.1");
  process.exit(1);
}

const conf = JSON.parse(fs.readFileSync(CONF, "utf8"));
const oldVersion = conf.version;
if (oldVersion === newVersion) {
  console.log(`已经是 v${newVersion}，无需改动`);
  process.exit(0);
}

conf.version = newVersion;
fs.writeFileSync(CONF, JSON.stringify(conf, null, 2) + "\n");

console.log(`[bump-version] ${oldVersion} → ${newVersion}`);

// 同步到 package.json + Cargo.toml
execFileSync("node", [path.join(__dirname, "sync-version.cjs")], {
  stdio: "inherit",
});

console.log(`
✅ 版本已统一为 v${newVersion}

接下来：
  git diff                       # 看看 3 个文件都改对了
  git add -A
  git commit -m "chore: bump to v${newVersion}"
  git tag v${newVersion}
  git push origin main --tags    # 触发 GitHub Actions release workflow
`);
