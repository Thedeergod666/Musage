#!/usr/bin/env node
/**
 * 单一来源：src-tauri/tauri.conf.json 里的 version 字段
 * 同步到：
 *   - package.json
 *   - src-tauri/Cargo.toml 的 [package] 段
 *
 * 触发时机（通过 package.json 的钩子自动跑）：
 *   - pnpm dev    →  predev
 *   - pnpm build  →  prebuild
 *   - CI 在 tauri build 前显式跑一次（覆盖 tag 版本）
 *
 * 维护者手动改版本：用 scripts/bump-version.cjs 0.2.0
 */

const fs = require("fs");
const path = require("path");

const ROOT = path.resolve(__dirname, "..");
const CONF = path.join(ROOT, "src-tauri", "tauri.conf.json");
const PKG = path.join(ROOT, "package.json");
const CARGO = path.join(ROOT, "src-tauri", "Cargo.toml");

function readJSON(p) {
  return JSON.parse(fs.readFileSync(p, "utf8"));
}
function writeJSON(p, obj) {
  fs.writeFileSync(p, JSON.stringify(obj, null, 2) + "\n");
}

function main() {
  const conf = readJSON(CONF);
  const v = conf.version;
  if (!v || !/^\d+\.\d+\.\d+/.test(v)) {
    throw new Error(`tauri.conf.json 里的 version 非法: ${v}`);
  }

  const changed = [];

  // 1) package.json
  const pkg = readJSON(PKG);
  if (pkg.version !== v) {
    pkg.version = v;
    writeJSON(PKG, pkg);
    changed.push("package.json");
  }

  // 2) src-tauri/Cargo.toml —— 只改 [package] 段下的 version
  //    （[lib] 段没 version；[features] 段也没；不影响）
  const cargo = fs.readFileSync(CARGO, "utf8");
  const updated = cargo.replace(
    /^(\[package\][\s\S]*?\n)version\s*=\s*"[^"]+"/m,
    (_m, prefix) => `${prefix}version = "${v}"`,
  );
  if (updated !== cargo) {
    fs.writeFileSync(CARGO, updated);
    changed.push("Cargo.toml");
  }

  if (changed.length) {
    console.log(`[sync-version] v${v} → ${changed.join(", ")}`);
  } else {
    console.log(`[sync-version] v${v} (no change)`);
  }
}

main();
