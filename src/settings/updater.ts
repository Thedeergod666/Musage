// 自动更新面板（设置面板动态注入的 section）
//
// 从 TS 动态注入，不动 settings.html。包含：
//   - 当前版本
//   - "检查更新" 按钮
//   - 状态显示（最新 / 有可用 / 下载中 / 错误）
//   - 有可用更新时的"立即下载" / "下载并重启" 按钮

import { getAppVersion } from "./api";
import { $ } from "./utils";
import { flash } from "./utils";
import { t } from "../i18n";
import {
  checkForUpdate,
  downloadAndInstall,
  onUpdateState,
  relaunchApp,
  type UpdateState,
} from "../updater";

export function setupUpdaterSection() {
  // 找到 "保存配置" 按钮所在 row，插一个新区块在它前面
  const saveRow = $("#save")?.closest(".row");
  if (!saveRow) return;

  const block = document.createElement("div");
  block.className = "row updater-section";
  block.id = "updater-section";
  block.innerHTML = `
    <h3>${t("settings.updater.section_title")}</h3>
    <div class="updater-meta">
      ${t("settings.about.current_version")}<span id="updater-current-version">—</span>
    </div>
    <div class="updater-actions">
      <button id="updater-check" class="primary">${t("settings.updater.check_update")}</button>
      <button id="updater-install" class="primary" hidden>${t("settings.updater.download_install")}</button>
      <button id="updater-relaunch" class="primary" hidden>${t("settings.updater.relaunch")}</button>
      <span id="updater-status"></span>
    </div>
    <div id="updater-notes" class="updater-notes"></div>
  `;
  saveRow.parentElement?.insertBefore(block, saveRow);

  // 读当前版本
  getAppVersion()
    .then((v) => {
      const el = document.getElementById("updater-current-version");
      if (el) el.textContent = `v${v}`;
    })
    .catch(() => {});

  // 绑按钮
  document.getElementById("updater-check")?.addEventListener("click", () => {
    void doCheck();
  });
  document.getElementById("updater-install")?.addEventListener("click", () => {
    void doInstall();
  });
  document.getElementById("updater-relaunch")?.addEventListener("click", () => {
    relaunchApp().catch((e) => flash(`✗ ${e}`, true));
  });

  // 订阅状态
  onUpdateState(renderUpdaterState);
}

async function doCheck() {
  const btn = document.getElementById("updater-check") as HTMLButtonElement | null;
  if (btn) btn.disabled = true;
  try {
    await checkForUpdate(false);
  } finally {
    if (btn) btn.disabled = false;
  }
}

async function doInstall() {
  const installBtn = document.getElementById(
    "updater-install",
  ) as HTMLButtonElement | null;
  const checkBtn = document.getElementById(
    "updater-check",
  ) as HTMLButtonElement | null;
  if (installBtn) installBtn.disabled = true;
  if (checkBtn) checkBtn.disabled = true;
  try {
    const result = await downloadAndInstall();
    if (result.status === "ready") {
      // 显示"立即重启"按钮
    }
  } finally {
    if (installBtn) installBtn.disabled = false;
    if (checkBtn) checkBtn.disabled = false;
  }
}

function renderUpdaterState(s: UpdateState) {
  const status = document.getElementById("updater-status");
  const installBtn = document.getElementById(
    "updater-install",
  ) as HTMLButtonElement | null;
  const relaunchBtn = document.getElementById(
    "updater-relaunch",
  ) as HTMLButtonElement | null;
  const notes = document.getElementById("updater-notes");
  if (!status) return;

  switch (s.status) {
    case "checking":
      status.textContent = t("settings.updater.checking");
      status.style.color = "";
      if (installBtn) installBtn.hidden = true;
      if (relaunchBtn) relaunchBtn.hidden = true;
      if (notes) notes.hidden = true;
      break;
    case "up-to-date":
      status.textContent = t("settings.updater.up_to_date");
      status.style.color = "#4caf50";
      if (installBtn) installBtn.hidden = true;
      if (relaunchBtn) relaunchBtn.hidden = true;
      if (notes) notes.hidden = true;
      break;
    case "available":
      status.textContent = t("settings.updater.available", { version: s.version ?? "" });
      status.style.color = "#2196f3";
      if (installBtn) installBtn.hidden = false;
      if (relaunchBtn) relaunchBtn.hidden = true;
      if (notes) {
        if (s.notes) {
          notes.textContent = s.notes;
          notes.hidden = false;
        } else {
          notes.hidden = true;
        }
      }
      break;
    case "downloading":
      status.textContent = s.progress != null
        ? t("settings.updater.downloading", { pct: (s.progress * 100).toFixed(0) })
        : t("settings.updater.downloading", { pct: "0" }).replace(" 0%", "");
      status.style.color = "#ff9800";
      if (installBtn) installBtn.hidden = true;
      if (relaunchBtn) relaunchBtn.hidden = true;
      break;
    case "ready":
      status.textContent = t("settings.updater.ready");
      status.style.color = "#4caf50";
      if (installBtn) installBtn.hidden = true;
      if (relaunchBtn) relaunchBtn.hidden = false;
      break;
    case "error":
      status.textContent = t("settings.updater.failed", { err: s.error ?? t("settings.updater.fallback_error") });
      status.style.color = "#f44336";
      if (installBtn) installBtn.hidden = true;
      if (relaunchBtn) relaunchBtn.hidden = true;
      break;
    default:
      // idle / disabled —— 不动 UI
      break;
  }
}
