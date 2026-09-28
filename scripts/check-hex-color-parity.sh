#!/usr/bin/env bash
# check-hex-color-parity.sh — 三处 hex 颜色长度口径必须一致
#
# 背景（2026-09-28 全量审查的簇 7）：
#   同一个 hex 颜色值在三处各有一份独立的「合法长度」定义，而且**一度三份
#   全不一样**：
#     · 前端 settings/app.ts 的 HEX6_RE          → 只认 6 位
#     · 后端写侧 commands/mod.rs is_valid_hex_color → 认 3|6|8（漏 4 位）
#     · 后端读侧 tray.rs parse_hex_color          → 认 3|4|6|8（多 4 位）
#   后果是**写侧比读侧严、读侧比写侧宽**：
#     · config.json 里若有 "tray_icon_color": "#f00a"（手改/跨机拷贝），
#       托盘渲染得好好的，但设置面板的**任何一次 save_config** 都返 Err
#       `commands.color_value_invalid` —— 用户改个轮询间隔都存不下去
#     · 反过来前端只认 6 位，用户从 DevTools 复制带 alpha 的 "#FFFFFFFF"
#       粘进 hex 框 → flash 报错，颜色没变
#   同一个值在「图标 / tooltip / 设置面板」三处表现不一致，这类 bug 极难靠
#   肉眼 review 抓住，必须机器校验。
#
# 用法: bash scripts/check-hex-color-parity.sh
# 非零退出 = 三处口径不一致
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

FE="$PROJECT_DIR/src/settings/app.ts"
BE_WRITE="$PROJECT_DIR/src-tauri/src/commands/mod.rs"
BE_READ="$PROJECT_DIR/src-tauri/src/tray.rs"

# 三处的「合法长度集合」分别长什么样：
#   前端   HEX_RE = /^#(?:[0-9a-fA-F]{3,4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$/
#   写侧   matches!(hex.len(), 3 | 4 | 6 | 8)
#   读侧   match b.len() { 3 => .., 4 => .., 6 => .., 8 => .. }
# 这里不试图精确解析语法，只提取「出现的数字长度」做集合比对 —— 口径变了
# （比如有人加了 2 位 #RG）一定会体现为集合差异。

extract_lens() {
  # $1 = 文件, $2 = 提取用的正则
  grep -oE "$2" "$1" 2>/dev/null \
    | grep -oE '[0-9]+' \
    | awk '$1 >= 2 && $1 <= 8' \
    | sort -n -u \
    | tr '\n' ' '
}

FE_LENS=$(extract_lens "$FE" '\{3,4\}|\{6\}|\{8\}')
# 写侧是 `matches!(hex.len(), 3 | 4 | 6 | 8)` 一行 —— 逐个数字提取，
# 不能要求每个长度后面都跟 `|`（最后一个 8 后面没有，会被漏掉）。
BE_WRITE_LENS=$(
  grep -oE 'matches!\(hex\.len\(\),[^)]*\)' "$BE_WRITE" 2>/dev/null \
    | grep -oE '[0-9]' | sort -n -u | tr '\n' ' '
)
BE_READ_LENS=$(extract_lens "$BE_READ" '^ *[34568] =>')

echo "hex 长度口径三处比对："
printf '  前端读值  %-22s %s\n' "src/settings/app.ts" "${FE_LENS:-<未提取到>}"
printf '  后端写侧  %-22s %s\n' "src-tauri/.../commands/mod.rs" "${BE_WRITE_LENS:-<未提取到>}"
printf '  后端读侧  %-22s %s\n' "src-tauri/.../tray.rs" "${BE_READ_LENS:-<未提取到>}"
echo ""

missing=""
for f in "$FE" "$BE_WRITE" "$BE_READ"; do
  [ -f "$f" ] || { echo "❌ 找不到 $f"; exit 1; }
done

# 口径一致 = 三处都含 3/4/6/8（4 位 #RGBA 从 2026-09-28 起三侧全部支持）
for lens in "$FE_LENS" "$BE_WRITE_LENS" "$BE_READ_LENS"; do
  for len in 3 4 6 8; do
    if ! echo "$lens" | grep -qE "(^| )${len}( |$)"; then
      missing="${missing}${len} "
    fi
  done
done

if [ -n "$missing" ]; then
  echo "❌ 三处 hex 长度口径不一致，缺少：${missing}"
  echo ""
  echo "后果：同一个颜色值在「托盘图标 / tooltip / 设置面板」三处表现不同 ——"
  echo "  写侧比读侧严会让一个渲染得好好的值把 save_config 永久拒掉。"
  echo "统一成 3|4|6|8（3=#RGB, 4=#RGBA, 6=#RRGGBB, 8=#RRGGBBAA）。"
  exit 1
fi

echo "✅ 三处 hex 长度口径一致（3|4|6|8）"
