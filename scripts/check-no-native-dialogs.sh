#!/usr/bin/env bash
# check-no-native-dialogs.sh — 禁止在 src/ 前端代码里调 native confirm()/prompt()/alert()
#
# 背景（2026-07-27 删除按钮失效 bug 的根因）：
#   macOS WKWebView（tauri 2.x / wry 0.55）的 UIDelegate 没实现 JS dialog panel
#   方法（runJavaScriptConfirmPanel / TextInputPanel / AlertPanel），所以：
#     - confirm() 不弹窗、同步返回 false  → if (!confirm(...)) 守卫静默拦截后续逻辑
#     - prompt()  不弹窗、返回 null
#     - alert()   no-op
#   Windows WebView2 原生支持这三个对话框，所以 Win 端看不出问题。
#   替代方案：src/settings/modal.ts 的 confirmInApp() / showModal()（in-app <dialog>）。
#
# 用法: bash scripts/check-no-native-dialogs.sh
# 非零退出 = 发现 native dialog 调用
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# 匹配 confirm(/prompt(/alert( 调用（confirm 前允许 . 以覆盖 window.confirm，
# 不允许标识符字符，所以 confirmInApp( 不会误中），然后排除纯注释行
# （// ... / /* ... / * ... 开头的行）。
hits=$(
  grep -rnE '(^|[^A-Za-z0-9_$])(confirm|prompt|alert)[[:space:]]*\(' \
    --include='*.ts' "$PROJECT_DIR/src" \
    | grep -vE '^[^:]+:[0-9]+:[[:space:]]*(//|/\*|\*)' \
    || true
)

if [ -n "$hits" ]; then
  echo "❌ src/ 里发现 native dialog 调用（macOS WKWebView 上静默失效，见本脚本头注释）："
  echo "$hits"
  echo ""
  echo "请改用 src/settings/modal.ts 的 confirmInApp() / showModal()"
  exit 1
fi

echo "✓ src/ 无 native confirm/prompt/alert 调用"
