// Central Lucide icon registry.
//
// 所有 lucide-static SVG 都走 `?url` 后缀，跟 [src/main.ts:18-19] 的
// `import tavilyLogo from "./x.svg?url"` 同一机制。Vite 5 emit 时只
// emit 被 import 的文件，dist/assets/ 实际只多 ~10KB（18 个 SVG）。
//
// 24px stroke 2（lucide-static 默认），跟现有 logo 描边粗细一致。
// Color 跟文字色走：SVG 里 `stroke="currentColor"`，父级 CSS `color`
// 通过 `<img>` 的 CSS `color` 属性继承（实测 WebView2 / WKWebView
// / WebKitGTK 三平台 OK）。
//
// 未来要换 icon 库只动这一个文件。

import navProvidersIcon from "lucide-static/icons/chart-bar.svg?url";
import navFloatingIcon from "lucide-static/icons/app-window.svg?url";
import navAppIcon from "lucide-static/icons/settings-2.svg?url";
import navAdvancedIcon from "lucide-static/icons/wrench.svg?url";
import navLogsIcon from "lucide-static/icons/clipboard-list.svg?url";
import navAboutIcon from "lucide-static/icons/info.svg?url";

import groupTokenPlanIcon from "lucide-static/icons/wallet.svg?url";
import groupBalanceIcon from "lucide-static/icons/piggy-bank.svg?url";
import groupOfficialIcon from "lucide-static/icons/building-2.svg?url";
import groupXiaomiIcon from "lucide-static/icons/utensils.svg?url";
import groupCustomIcon from "lucide-static/icons/puzzle.svg?url";
import groupMiscIcon from "lucide-static/icons/package.svg?url";

import flashCheck from "lucide-static/icons/check.svg?url";
import flashX from "lucide-static/icons/x.svg?url";
import flashWarn from "lucide-static/icons/triangle-alert.svg?url";
import copyIcon from "lucide-static/icons/copy.svg?url";
import regionFlag from "lucide-static/icons/flag.svg?url";
import regionGlobe from "lucide-static/icons/globe.svg?url";
import logoEmpty from "lucide-static/icons/zap.svg?url";

export {
  navProvidersIcon,
  navFloatingIcon,
  navAppIcon,
  navAdvancedIcon,
  navLogsIcon,
  navAboutIcon,
  groupTokenPlanIcon,
  groupBalanceIcon,
  groupOfficialIcon,
  groupXiaomiIcon,
  groupCustomIcon,
  groupMiscIcon,
  flashCheck,
  flashX,
  flashWarn,
  copyIcon,
  regionFlag,
  regionGlobe,
  logoEmpty,
};
