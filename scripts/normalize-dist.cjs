/**
 * 跨平台一致性补丁(idempotent):
 *   1) 剥 UTF-8 BOM
 *   2) 剥 CR
 *   3) 反 double-UTF-8
 *   4) HTML 中 `>` 到 `</body>`/`</html>` 之间空白 canonical 化成 1 空行
 *
 * CI 三个平台(ubuntu/macos/windows)各跑一遍,保证 dist/ 哈希一致。
 */

const fs = require('fs');
const path = require('path');

const exts = new Set(['.js', '.mjs', '.cjs', '.css', '.html', '.svg', '.json', '.txt', '.xml']);

function walk(d) {
  for (const e of fs.readdirSync(d, { withFileTypes: true })) {
    const p = path.join(d, e.name);
    if (e.isDirectory()) {
      walk(p);
    } else if (exts.has(path.extname(e.name).toLowerCase())) {
      let b = fs.readFileSync(p);
      let changed = false;

      // 1) strip leading UTF-8 BOM
      if (b.length >= 3 && b[0] === 0xef && b[1] === 0xbb && b[2] === 0xbf) {
        b = b.subarray(3);
        changed = true;
      }

      // 2) strip CR (CRLF or lone CR)
      if (b.includes(0x0d)) {
        b = Buffer.from(b.toString('binary').replace(/\r\n?/g, '\n'), 'binary');
        changed = true;
      }

      // 3) undo double-UTF-8: if file is all code points <= 0xFF when
      //    decoded as UTF-8, it's been Latin-1-then-UTF-8-d roundtripped
      const asUtf8 = b.toString('utf-8');
      let isDoubled = true;
      for (let i = 0; i < asUtf8.length; i++) {
        if (asUtf8.charCodeAt(i) > 0xff) {
          isDoubled = false;
          break;
        }
      }
      if (isDoubled && /[\x80-\xff]/.test(asUtf8)) {
        // The original source was treated as Latin-1 code points then
        // re-encoded as UTF-8; recover by encoding this UTF-8 string
        // back to Latin-1 (gives the bytes the source had as code points)
        const recovered = Buffer.from(asUtf8, 'latin1');
        if (recovered.length !== b.length) {
          b = recovered;
          changed = true;
        }
      }

      // 4) Vite 在 Windows 上生成的 HTML 在 `>` (前一个元素的 close)
      //    跟 `</body>` / `</html>` 之间比 *nix 多一个空行(还可能多
      //    一个,settings.html 这种有 intentional 空行的会被翻倍)。
      //    修法:统一 canonical 化 —— `>` 跟 `</(body|html)>` 之间
      //    任何空白(>0 个 \n + 任意缩进)都压成 1 个空行。
      if (path.extname(e.name).toLowerCase() === '.html') {
        const s = b.toString('utf-8').replace(/>\s*<\/(body|html)>/g, '>\n\n</$1>');
        if (Buffer.byteLength(s, 'utf-8') !== b.length) {
          b = Buffer.from(s, 'utf-8');
          changed = true;
        }
      }

      if (changed) fs.writeFileSync(p, b);
    }
  }
}

walk('dist');
