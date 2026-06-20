// 设置面板 modal 组件
//
// PR 3 起需要弹窗（"添加自定义来源"）。用原生 `<dialog>` 元素：
// - a11y / focus trap / ESC 关闭 免费
// - Tauri 2 的 WebView2 / WKWebView 都支持
// - backdrop 用 `::backdrop` 伪元素 + 半透明黑
//
// 不引入第三方 modal 库（bootstrap / sweetalert / etc）—— 项目本身就是
// 0 runtime dep 的 vanilla TS。

import { el } from "./utils";
import { t } from "../i18n";

export interface ModalOptions {
  title: string;
  body: HTMLElement;
  /** 提交按钮回调。返 true = 关闭 modal，false = 留在原位 */
  onSubmit: () => Promise<boolean>;
  submitLabel?: string;
  cancelLabel?: string;
}

/** 弹出 modal。多次调用可以嵌套多个（每个独立一个 `<dialog>`）。
 *
 * **2026-06-20 audit**：之前 dialog 没 aria-labelledby / aria-describedby，
 * 屏幕阅读器朗读 dialog 内容时缺上下文。给 title h2 / body wrapper 分配 id，
 * 在 dialog 上 aria-labelledby 指向 title。
 */
export function showModal(opts: ModalOptions): void {
  const dlg = el("dialog", { class: "modal" });
  const titleId = `modal-title-${Math.random().toString(36).slice(2, 9)}`;
  const descId = `modal-desc-${Math.random().toString(36).slice(2, 9)}`;
  const form = el("form", { method: "dialog" });
  const titleEl = el("h2", { id: titleId }, opts.title);
  const bodyWrapper = el("div", { id: descId });
  bodyWrapper.appendChild(opts.body);
  form.appendChild(titleEl);
  form.appendChild(bodyWrapper);
  form.appendChild(
    el(
      "div",
      { class: "modal-actions" },
      el(
        "button",
        { type: "button", value: "cancel", class: "btn-secondary" },
        opts.cancelLabel ?? t("settings.common.cancel"),
      ),
      el(
        "button",
        { type: "submit", value: "submit", class: "btn-primary" },
        opts.submitLabel ?? t("settings.common.save"),
      ),
    ),
  );
  dlg.setAttribute("aria-labelledby", titleId);
  dlg.setAttribute("aria-describedby", descId);
  dlg.appendChild(form);
  document.body.appendChild(dlg);

  // 取消按钮：直接 close（form 不提交，submit 按钮不会触发）
  form.querySelector<HTMLButtonElement>('button[value="cancel"]')!
    .addEventListener("click", () => dlg.close());

  // 提交按钮
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const submitBtn = form.querySelector<HTMLButtonElement>(
      'button[value="submit"]',
    )!;
    submitBtn.disabled = true;
    try {
      if (await opts.onSubmit()) dlg.close();
    } finally {
      submitBtn.disabled = false;
    }
  });

  // 关闭时从 DOM 摘掉（防止多次弹 modal 堆 DOM 节点）
  dlg.addEventListener("close", () => dlg.remove());
  dlg.showModal();
}
