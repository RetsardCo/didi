import { createModalFocusSession } from "./modal_focus";

export function getRequiredElement<T extends HTMLElement = HTMLElement>(id: string): T {
  const element = document.getElementById(id);
  if (!element) {
    throw new Error(`Missing required DOM element: #${id}`);
  }
  return element as T;
}

export function clearChildren(node: HTMLElement): void {
  while (node.firstChild) {
    node.removeChild(node.firstChild);
  }
}

export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  options?: {
    className?: string;
    text?: string;
    id?: string;
    attrs?: Record<string, string>;
  },
  children?: Array<HTMLElement | Text | null | undefined>,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (options?.className) {
    node.className = options.className;
  }
  if (options?.id) {
    node.id = options.id;
  }
  if (options?.text !== undefined) {
    node.textContent = options.text;
  }
  if (options?.attrs) {
    for (const [key, value] of Object.entries(options.attrs)) {
      node.setAttribute(key, value);
    }
  }
  if (children) {
    for (const child of children) {
      if (child) {
        node.appendChild(child);
      }
    }
  }
  return node;
}

export function createButton(
  label: string,
  className: string,
  onClick: () => void,
  options?: { disabled?: boolean; title?: string; id?: string },
): HTMLButtonElement {
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = className;
  btn.textContent = label;
  if (options?.disabled) {
    btn.disabled = true;
  }
  if (options?.title) {
    btn.title = options.title;
  }
  if (options?.id) {
    btn.id = options.id;
  }
  btn.addEventListener("click", onClick);
  return btn;
}

let toastTimer: number | null = null;

export function showToast(
  message: string,
  tone: "success" | "warning" | "danger" | "neutral" = "success",
): void {
  const toast = getRequiredElement("status-toast");
  toast.textContent = message;
  toast.className = `status-toast status-toast-${tone}`;
  toast.hidden = false;

  if (toastTimer !== null) {
    window.clearTimeout(toastTimer);
  }
  toastTimer = window.setTimeout(() => {
    toast.hidden = true;
    toast.textContent = "";
    toastTimer = null;
  }, 4000);
}

export interface ModalOptions {
  title: string;
  body: HTMLElement[];
  actions: Array<{
    label: string;
    className: string;
    value: string;
  }>;
  allowEscape?: boolean;
}

const MODAL_FOCUSABLE_SELECTOR = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled]):not([type='hidden'])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "[tabindex]:not([tabindex='-1'])",
].join(", ");

function getVisibleModalFocusables(dialogEl: HTMLElement): HTMLElement[] {
  const candidates = Array.from(dialogEl.querySelectorAll<HTMLElement>(MODAL_FOCUSABLE_SELECTOR));
  return candidates.filter((el) => !el.hidden && el.closest("[hidden]") === null);
}

/**
 * Displays an accessible modal dialog anchored inside `#modal-backdrop`, traps `Tab` / `Shift+Tab`
 * and `focusin` inside `.modal-dialog`, handles `Escape` where permitted, supports zero-focusable
 * dialogs safely, and restores focus cleanly to the invoking element on close (`FINDING-01`).
 */
export function openModalDialog(options: ModalOptions): Promise<string | null> {
  const previouslyFocused =
    document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const backdrop = getRequiredElement("modal-backdrop");
  const dialogEl =
    backdrop.querySelector<HTMLElement>(".modal-dialog") ?? backdrop;
  const titleEl = getRequiredElement("modal-title");
  const bodyEl = getRequiredElement("modal-body");
  const actionsEl = getRequiredElement("modal-actions");

  titleEl.textContent = options.title;
  clearChildren(bodyEl);
  for (const child of options.body) {
    bodyEl.appendChild(child);
  }
  clearChildren(actionsEl);
  dialogEl.setAttribute("tabindex", "-1");

  return new Promise((resolve) => {
    let settled = false;

    const session = createModalFocusSession<HTMLElement>(
      {
        getFocusables: () => getVisibleModalFocusables(dialogEl),
        getActiveElement: () =>
          document.activeElement instanceof HTMLElement ? document.activeElement : null,
        containsTarget: (target) => target !== null && dialogEl.contains(target),
        isPreviouslyFocusedConnected: (prev) => document.contains(prev),
        focusDialogContainer: () => {
          dialogEl.focus();
        },
      },
      previouslyFocused,
      () => close(null),
      options.allowEscape ?? true,
    );

    const handleKeyDown = (event: KeyboardEvent) => {
      session.handleKeyDown(event);
    };

    const handleFocusIn = (event: FocusEvent) => {
      const target = event.target instanceof HTMLElement ? event.target : null;
      session.handleFocusIn(target);
    };

    const close = (result: string | null) => {
      if (settled) return;
      settled = true;
      backdrop.hidden = true;
      clearChildren(bodyEl);
      clearChildren(actionsEl);
      document.removeEventListener("keydown", handleKeyDown, true);
      document.removeEventListener("focusin", handleFocusIn, true);
      session.restoreFocus();
      resolve(result);
    };

    for (const action of options.actions) {
      const btn = createButton(action.label, action.className, () => close(action.value));
      actionsEl.appendChild(btn);
    }

    backdrop.hidden = false;
    document.addEventListener("keydown", handleKeyDown, true);
    document.addEventListener("focusin", handleFocusIn, true);
    queueMicrotask(() => {
      session.focusInitial();
    });
  });
}
