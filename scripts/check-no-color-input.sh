#!/usr/bin/env bash
# check-no-color-input.sh — 禁止在前端使用原生 <input type="color">
#
# 背景（2026-09-28 全量审查 H-2 的根因）：
#   memory `wkwebview-color-input-change-footgun` 已经把这条写死成项目约定 ——
#   「WKWebView 里不要用 `<input type=color>`，取色 UI 一律纯 DOM 色板 +
#   hex 文本框」。但约定只靠人记：托盘那 4 个取色器在 commit 6931dfb 换成了
#   色板，**浮窗那 4 个（settings/floating.ts 的 color_overrides）漏换**，
#   至今仍是 `<input type="color">` + 只绑 `change`：
#     · macOS：NSColorPanel 能打开能取色，但 input/change **全程不派发**
#       → 选了色什么都不发生，color_overrides 永不落盘
#     · Windows WebView2 正常 → 只在 mac 暴露，很难在开发机复现
#   这是「约定写了但没有 enforcement」的典型，本脚本补上 enforcement。
#
# 用法: bash scripts/check-no-color-input.sh
# 非零退出 = 发现 <input type="color">
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# 匹配 TS 的 type: "color" / type:'color' 与 HTML 的 type="color"。
# 排除纯注释行（行首 // /* *），以及提到这条约定本身的注释。
hits=$(
  {
    grep -rnE "type[[:space:]]*:[[:space:]]*[\"']color[\"']" \
      --include='*.ts' "$PROJECT_DIR/src" || true
    grep -rnE 'type[[:space:]]*=[[:space:]]*"color"' \
      --include='*.html' "$PROJECT_DIR" --exclude-dir=node_modules \
      --exclude-dir=dist --exclude-dir=target || true
  } | grep -vE '^[^:]+:[0-9]+:[[:space:]]*(//|/\*|\*|<!--)' || true
)

if [ -n "$hits" ]; then
  echo "❌ 发现原生 <input type=\"color\">（macOS WKWebView 上不派发 input/change，取色静默失效）："
  echo "$hits"
  echo ""
  echo "改用纯 DOM 色板 + hex 文本框，参考 settings/app.ts 的 .accent-palette 实现："
  echo "  1. 一排 <button class=\"accent-swatch\"> 供点击选色"
  echo "  2. 一个 hex 文本框供精确输入（宽度口径跟后端 is_valid_hex_color 对齐：3|4|6|8）"
  echo "  3. 用闭包变量 lastGoodX 记录「最近一次成功值」，失败时回填它"
  exit 1
fi

echo "✓ 前端无原生 <input type=\"color\">"
