// scripts/strip-emoji-i18n.mjs
// en + zh-CN 同步去 emoji + flag，dry-run 默认；--write 才落盘。
//
// 范围：Misc Symbols(2600-26FF) + Dingbats(2700-27BF) + Flags(1F1E6-1F1FF)
//      + Misc Pictographs(1F300-1F5FF) + Emoticons(1F600-1F64F)
//      + Symbols Extended-A (1F900-1F9FF) + 9 个特殊码点
//      + ✓✗⚠ (2713/2717/26A0) + ⚙ (2699) + 🅼 (1F17D)

import { readFileSync, writeFileSync } from "node:fs";

const FILES = ["src/i18n/en.json", "src/i18n/zh-CN.json"];

// 覆盖所有 emoji Unicode block + 9 个特殊码点
const RE =
  /[\u{2600}-\u{27BF}\u{1F1E6}-\u{1F1FF}\u{1F300}-\u{1F5FF}\u{1F600}-\u{1F64F}\u{1F900}-\u{1F9FF}\u{1F680}\u{1F6A9}\u{1F389}\u{1F17D}]/gu;

let total = 0;
for (const f of FILES) {
  const s = readFileSync(f, "utf8");
  const n = (s.match(RE) || []).length;
  total += n;
  if (process.argv.includes("--write")) {
    // 去 emoji 后顺便 trim 掉值开头的多余空格（"✓ 已保存" → "已保存"）
    const out = s.replace(RE, "").replace(/: "(\s+)/g, ': "');
    writeFileSync(f, out);
  }
  console.log(`${f}: ${n} emoji${process.argv.includes("--write") ? " stripped" : " (dry-run)"}`);
}
console.log(`total: ${total}`);
