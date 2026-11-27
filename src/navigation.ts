import type { NavTabId } from "./types.ts";

export type DirtyNavigationDecision = "noop" | "allow" | "prompt";

/**
 * Determines whether tab navigation may proceed immediately, requires an unsaved-changes
 * confirmation prompt, or is a no-op (`FINDING-02`).
 *
 * `Editor <-> Preview` preserves drafts without prompting so users can inspect Draft Preview.
 * Leaving either `Editor` or `Preview` to `Dashboard`, `Profiles`, or `Settings` while dirty
 * prompts before abandoning unsaved edits.
 */
export function resolveDirtyNavigationDecision(
  currentTab: NavTabId,
  targetTab: NavTabId,
  isDirty: boolean,
): DirtyNavigationDecision {
  if (currentTab === targetTab) {
    return "noop";
  }
  if (!isDirty) {
    return "allow";
  }
  const staysInDraftWorkspace =
    (currentTab === "editor" && targetTab === "preview") ||
    (currentTab === "preview" && targetTab === "editor");
  if (staysInDraftWorkspace) {
    return "allow";
  }
  if (currentTab === "editor" || currentTab === "preview") {
    return "prompt";
  }
  return "allow";
}

export interface NavigationHost {
  getCurrentTab: () => NavTabId;
  setCurrentTab: (tab: NavTabId) => void;
  isEditorDirty: () => boolean;
  confirmUnsaved: () => Promise<boolean>;
  onNavigateComplete: (tab: NavTabId) => void;
  onNavigateCancelled?: (currentTab: NavTabId) => void;
}

export function createNavigationController(host: NavigationHost): {
  navigateTo: (targetTab: NavTabId) => Promise<boolean>;
  isNavigationPending: () => boolean;
} {
  let pending = false;

  async function navigateTo(targetTab: NavTabId): Promise<boolean> {
    if (pending) {
      return false;
    }

    const currentTab = host.getCurrentTab();
    const decision = resolveDirtyNavigationDecision(
      currentTab,
      targetTab,
      host.isEditorDirty(),
    );

    if (decision === "noop") {
      return false;
    }

    if (decision === "prompt") {
      pending = true;
      try {
        const proceed = await host.confirmUnsaved();
        if (!proceed) {
          host.onNavigateCancelled?.(currentTab);
          return false;
        }
      } finally {
        pending = false;
      }
    }

    host.setCurrentTab(targetTab);
    host.onNavigateComplete(targetTab);
    return true;
  }

  return {
    navigateTo,
    isNavigationPending: () => pending,
  };
}
