#!/usr/bin/env bash
# check-provider-helper-parity.sh — provider 共享 helper 的「姊妹路径」接入清单
#
# 背景（2026-09-28 全量审查的元模式 B「修复未平行移植」）：
#   审查里 4 条 Medium 是同一个形态 —— 改了 A 函数，忘了同款 B 函数：
#     · 9-04 用 json_i64 修了 anysearch 的业务码 → 漏 xiaomi 两处 + siliconflow 一处
#       （结果：业务码序列化成字符串时整个拦截分支被 and_then(|v| v.as_i64()) 跳过）
#     · 9-04 把 custom.rs 的 URL 配置/SSRF 错误从 FetchError::auth 改成
#       config_error → 漏 zenmux 三处（结果：base_url 填错时前端弹「重新登录」
#       而不是「配置有问题」）
#     · 595a9b1 的 H-Provider 双字段守卫加在 build_window_row → 漏同文件的
#       parse_total_quota（结果：totalQuota 只带 limit 时恒显示 100%）
#     · volcengine 的 "Error": null 守卫 openrouter 有、本文件漏
#   这类 bug 人工审查很难全覆盖 —— 改 A 的时候根本不会想起 B。
#   本脚本把「哪些 provider 还没接」列出来，让它变成可收敛的清单。
#
# ⚠ 这是**提示型**检查（列清单 + 退出码 0），不是硬失败：
#   现在必然有待补项，硬失败会立刻把 CI 打红。全绿后可以改成硬失败。
#
# 用法: bash scripts/check-provider-helper-parity.sh
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
PROV_DIR="$PROJECT_DIR/src-tauri/src/providers"

if [ ! -d "$PROV_DIR" ]; then
  echo "❌ 找不到 $PROV_DIR"
  exit 1
fi

all_providers=$(ls "$PROV_DIR"/*.rs 2>/dev/null | xargs -n1 basename | grep -v '^mod.rs$\|^parse.rs$')

# report <名字> <说明> <"有该构造"的正则> <"已接修复"的正则>
#
# 候选判定必须是「确实有需要修的构造」，不能简单地把所有 provider 算进去 ——
# 否则清单噪声大到没人看，守门就等于没有守门（这正是元模式 B 想解决的问题本身）。
report() {
  local name="$1" scope_desc="$2" has_construct="$3" has_fix="$4"
  local files=() missing=()
  for f in $all_providers; do
    local path="$PROV_DIR/$f"
    # 先确认这个 provider 真的存在"需要修的构造"
    grep -qE "$has_construct" "$path" 2>/dev/null || continue
    # 逐个排除的假阳性（附理由，便于将来清单收敛到空后升级成硬失败）
    case "$name|$f" in
      # volcengine 的 Authorization 是 AK/SK 算出的 HMAC 签名串（十六进制），
      # 不是用户原样粘贴的 key —— 不存在"key 里带控制字符"这条注入路径。
      "validate_bearer_key|volcengine_ark.rs") continue ;;
    esac
    if grep -qE "$has_fix" "$path" 2>/dev/null; then
      files+=("$f")
    else
      missing+=("$f")
    fi
  done
  local total=$(( ${#files[@]} + ${#missing[@]} ))
  echo ""
  echo "── ${name} ──"
  printf '  范围（%s）: %d 个已接入 / %d 个候选\n' "${scope_desc}" "${#files[@]}" "${total}"
  if [ ${#missing[@]} -gt 0 ]; then
    echo "  ⬜ 待补: ${missing[*]}"
  else
    echo "  ✅ 全部已接入"
  fi
}

echo "provider 共享 helper 接入清单（提示型检查，不阻断 CI）"
echo "用法注释见 scripts/check-provider-helper-parity.sh 文件头"

# 1. validate_bearer_key —— 9-04 L-6 的注释写着「各 provider 在拼 Authorization
#    头前调用」，实际只有 4/14 接了。没接的 provider 遇到 key 里带换行（从终端
#    / 聊天窗口复制粘贴）会走到 reqwest builder error，humanize_reqwest_err 三个
#    分类全不命中 → 用户看到「网络错误 [URL]: ...」，与真因（key 带了控制字符）无关。
# 1. validate_bearer_key —— 9-04 L-6 的注释写着「各 provider 在拼 Authorization
#    头前调用」，实际只有一部分接了。没接的 provider 遇到 key 里带换行（从终端
#    / 聊天窗口复制粘贴）会走到 reqwest builder error，humanize_reqwest_err 三个
#    分类全不命中 → 用户看到「网络错误 [URL]: ...」，与真因（key 带了控制字符）无关。
report "validate_bearer_key" "拼 Bearer Authorization 头的 provider" \
       'header\("Authorization"' "validate_bearer_key"

# 2. json_i64 —— 业务码宽松解析。业务码序列化成字符串（"40101"）时，
#    只吃 as_i64 的地方整个拦截分支被跳过，401 降级成 Parse 错 → 前端不亮
#    「重新登录」按钮。9-04 已在 anysearch 修过同款。
report "json_i64（业务码宽松解析）" '用 get("code").as_i64() 判业务码的 provider' \
       'get\("code"\)[^;]*as_i64' "json_i64"

# 3. is_null 守卫 —— HTTP 200 + {"error": null, "data": {...}} 是成功信封。
#    没有 .filter(|e| !e.is_null()) 时 Value::get 对「键存在值为 null」返回
#    Some(&Value::Null)，成功响应会被打成业务错误（code unknown + 空 msg）。
report "body-error 的 null/object 守卫" '解析带 error 封套的 provider' \
       'get\("[Ee]rror"\)' "is_null|is_object"

# 4. config_error —— URL 配置错误（scheme 非 https / authority 含 @ / SSRF 命中）
#    应该归 config_error（ErrorKind::Other），不是 auth。用 auth 会让前端弹
#    「重新登录 / 打开设置」，而真因是 base_url 写错了。
report "URL 配置/SSRF 错误用 config_error" '做 URL scheme / SSRF 校验的 provider' \
       'url_scheme_invalid|ssrf_blocked|url_authority_has_userinfo' "config_error"

echo ""
echo "（提示型检查：以上 ⬜ 项不阻断 CI，但每次新增 provider 或修这类问题时"
echo "  顺手把对应文件接上，接一个少一个。）"
exit 0
