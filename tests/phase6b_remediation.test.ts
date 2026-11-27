import assert from "node:assert/strict";
import test from "node:test";
import {
  isAutostartPreferenceChangedEvent,
  isPresenceServiceStatus,
} from "../src/event_validation.ts";
import {
  computeNextModalFocusTarget,
  createModalFocusSession,
  type FocusTargetLike,
} from "../src/modal_focus.ts";
import {
  createNavigationController,
  resolveDirtyNavigationDecision,
} from "../src/navigation.ts";
import type { NavTabId } from "../src/types.ts";

class MockElement implements FocusTargetLike {
  readonly id: string;
  connected = true;
  focusCount = 0;

  constructor(id: string) {
    this.id = id;
  }

  focus(): void {
    MockElement.currentActive = this;
    this.focusCount += 1;
  }

  static currentActive: MockElement | null = null;
}

// ============================================================================
// FINDING-01 Regression Tests — Modal Keyboard Accessibility
// ============================================================================

test("FINDING-01: initial focus, Tab wrap, Shift+Tab wrap, focusin containment, Escape, and focus restoration", () => {
  const triggerBtn = new MockElement("open-modal-btn");
  const backgroundBtn = new MockElement("background-btn");
  const inputField = new MockElement("modal-input");
  const confirmBtn = new MockElement("modal-confirm");
  const cancelBtn = new MockElement("modal-cancel");
  const dialogContainer = new MockElement("modal-dialog");

  const focusables = [inputField, confirmBtn, cancelBtn];
  let closeRequested = 0;
  triggerBtn.focus();

  const session = createModalFocusSession<MockElement>(
    {
      getFocusables: () => focusables,
      getActiveElement: () => MockElement.currentActive,
      containsTarget: (target) => target !== null && focusables.includes(target),
      isPreviouslyFocusedConnected: (prev) => prev.connected,
      focusDialogContainer: () => dialogContainer.focus(),
    },
    triggerBtn,
    () => {
      closeRequested += 1;
    },
    true,
  );

  // Initial focus moves to first focusable element inside modal
  session.focusInitial();
  assert.equal(MockElement.currentActive, inputField);

  // Tab advances forward through elements and wraps last -> first
  let prevented = 0;
  session.handleKeyDown({
    key: "Tab",
    shiftKey: false,
    preventDefault: () => {
      prevented += 1;
    },
  });
  assert.equal(MockElement.currentActive, confirmBtn);

  session.handleKeyDown({
    key: "Tab",
    shiftKey: false,
    preventDefault: () => {
      prevented += 1;
    },
  });
  assert.equal(MockElement.currentActive, cancelBtn);

  session.handleKeyDown({
    key: "Tab",
    shiftKey: false,
    preventDefault: () => {
      prevented += 1;
    },
  });
  assert.equal(MockElement.currentActive, inputField);

  // Shift+Tab at first wraps to last element
  session.handleKeyDown({
    key: "Tab",
    shiftKey: true,
    preventDefault: () => {
      prevented += 1;
    },
  });
  assert.equal(MockElement.currentActive, cancelBtn);
  assert.equal(prevented, 4);

  // Attempted focus escape to background element is pulled back into modal
  MockElement.currentActive = backgroundBtn;
  session.handleFocusIn(backgroundBtn);
  assert.equal(MockElement.currentActive, inputField);

  // Escape requests modal close
  let escapePrevented = false;
  session.handleKeyDown({
    key: "Escape",
    shiftKey: false,
    preventDefault: () => {
      escapePrevented = true;
    },
  });
  assert.equal(escapePrevented, true);
  assert.equal(closeRequested, 1);

  // Closing restores focus to original invoking button
  session.restoreFocus();
  assert.equal(MockElement.currentActive, triggerBtn);
});

test("FINDING-01: dialog with zero focusable controls traps focus safely and skips detached trigger restoration", () => {
  const detachedTrigger = new MockElement("detached-trigger");
  detachedTrigger.connected = false;
  const dialogContainer = new MockElement("empty-dialog");

  MockElement.currentActive = detachedTrigger;

  const session = createModalFocusSession<MockElement>(
    {
      getFocusables: () => [],
      getActiveElement: () => MockElement.currentActive,
      containsTarget: (target) => target === dialogContainer,
      isPreviouslyFocusedConnected: (prev) => prev.connected,
      focusDialogContainer: () => dialogContainer.focus(),
    },
    detachedTrigger,
    () => {
      throw new Error("Escape should not close when allowEscape=false");
    },
    false,
  );

  session.focusInitial();
  assert.equal(MockElement.currentActive, dialogContainer);
  assert.equal(computeNextModalFocusTarget([], dialogContainer, false), null);

  let tabPrevented = 0;
  session.handleKeyDown({
    key: "Tab",
    shiftKey: false,
    preventDefault: () => {
      tabPrevented += 1;
    },
  });
  session.handleKeyDown({
    key: "Tab",
    shiftKey: true,
    preventDefault: () => {
      tabPrevented += 1;
    },
  });
  assert.equal(tabPrevented, 2);
  assert.equal(MockElement.currentActive, dialogContainer);

  // Escape disabled when allowEscape=false
  session.handleKeyDown({
    key: "Escape",
    shiftKey: false,
    preventDefault: () => {},
  });

  // Detached trigger should not receive focus after modal closes
  const restoreCallsBefore = detachedTrigger.focusCount;
  session.restoreFocus();
  assert.equal(detachedTrigger.focusCount, restoreCallsBefore);
});

// ============================================================================
// FINDING-02 Regression Tests — Unsaved Editor Changes Navigation Guard
// ============================================================================

test("FINDING-02: decision matrix matches all clean and dirty workspace/external navigation paths", () => {
  const allTabs: NavTabId[] = ["dashboard", "profiles", "editor", "preview", "settings"];

  // Clean editor allows all cross-tab transitions without prompt
  for (const from of allTabs) {
    for (const to of allTabs) {
      const expected = from === to ? "noop" : "allow";
      assert.equal(resolveDirtyNavigationDecision(from, to, false), expected);
    }
  }

  // Dirty editor: Editor <-> Preview allowed without prompt
  assert.equal(resolveDirtyNavigationDecision("editor", "preview", true), "allow");
  assert.equal(resolveDirtyNavigationDecision("preview", "editor", true), "allow");

  // Dirty editor: leaving Editor or Preview to any other screen prompts
  for (const external of ["dashboard", "profiles", "settings"] as const) {
    assert.equal(resolveDirtyNavigationDecision("editor", external, true), "prompt");
    assert.equal(resolveDirtyNavigationDecision("preview", external, true), "prompt");
  }
});

test("FINDING-02: full navigation workflow handles stay, abandon, round-trip preview, and re-entrancy", async () => {
  let currentTab: NavTabId = "editor";
  let dirty = true;
  let promptCount = 0;
  let nextPromptResult = false;
  let cancelledFrom: NavTabId | null = null;
  let resolveSlowPrompt: ((value: boolean) => void) | null = null;

  const nav = createNavigationController({
    getCurrentTab: () => currentTab,
    setCurrentTab: (tab) => {
      currentTab = tab;
    },
    isEditorDirty: () => dirty,
    confirmUnsaved: () => {
      promptCount += 1;
      if (resolveSlowPrompt) {
        return new Promise<boolean>((resolve) => {
          resolveSlowPrompt = resolve;
        });
      }
      return Promise.resolve(nextPromptResult);
    },
    onNavigateComplete: () => {},
    onNavigateCancelled: (tab) => {
      cancelledFrom = tab;
    },
  });

  // 1. Editor -> Preview without prompt; draft remains dirty
  assert.equal(await nav.navigateTo("preview"), true);
  assert.equal(currentTab, "preview");
  assert.equal(dirty, true);
  assert.equal(promptCount, 0);

  // 2. Preview -> Profiles while dirty prompts; user cancels (Stay Here)
  nextPromptResult = false;
  assert.equal(await nav.navigateTo("profiles"), false);
  assert.equal(currentTab, "preview");
  assert.equal(dirty, true);
  assert.equal(promptCount, 1);
  assert.equal(cancelledFrom, "preview");

  // 3. Preview -> Editor allowed without prompt and preserves dirty draft
  assert.equal(await nav.navigateTo("editor"), true);
  assert.equal(currentTab, "editor");
  assert.equal(dirty, true);
  assert.equal(promptCount, 1);

  // 4. Concurrent navigation attempts while prompt is pending do not double-prompt
  let slowFinish!: (v: boolean) => void;
  const slowNav = createNavigationController({
    getCurrentTab: () => currentTab,
    setCurrentTab: (tab) => {
      currentTab = tab;
    },
    isEditorDirty: () => dirty,
    confirmUnsaved: () => {
      promptCount += 1;
      return new Promise<boolean>((resolve) => {
        slowFinish = resolve;
      });
    },
    onNavigateComplete: () => {},
  });

  const firstAttempt = slowNav.navigateTo("settings");
  const duplicateAttempt = slowNav.navigateTo("dashboard");
  assert.equal(await duplicateAttempt, false);
  assert.equal(promptCount, 2);

  // Confirm abandonment completes intended navigation
  dirty = false; // simulates Discard/Save clearing dirty state inside confirm modal
  slowFinish(true);
  assert.equal(await firstAttempt, true);
  assert.equal(currentTab, "settings");

  // 5. Clean navigation from Editor/Settings never prompts
  assert.equal(await nav.navigateTo("dashboard"), true);
  assert.equal(currentTab, "dashboard");
  assert.equal(promptCount, 2);
});

// ============================================================================
// FINDING-03 Regression Tests — Autostart Event Validation & Sync Lifecycle
// ============================================================================

test("FINDING-03: autostart and status event payload validation rejects malformed data", () => {
  assert.equal(isAutostartPreferenceChangedEvent({ launch_at_login: true }), true);
  assert.equal(isAutostartPreferenceChangedEvent({ launch_at_login: false }), true);

  const malformedPayloads: unknown[] = [
    null,
    undefined,
    true,
    "true",
    1,
    [],
    {},
    { launch_at_login: "true" },
    { launch_at_login: 1 },
    { enabled: true },
  ];
  for (const invalid of malformedPayloads) {
    assert.equal(isAutostartPreferenceChangedEvent(invalid), false);
  }

  assert.equal(
    isPresenceServiceStatus({
      state: "published",
      status_label: "Published",
      masked_app_id: "****1234",
      is_connected: true,
      consecutive_failures: 0,
      pipe_slot: 0,
      protocol_version: 1,
      last_error: null,
    }),
    true,
  );
  assert.equal(isPresenceServiceStatus({ state: "unknown_state" }), false);
});

test("FINDING-03: open settings view synchronizes checkbox on tray updates and cleans up listener", () => {
  // Simulate minimal event bus + open Settings checkbox binding matching main.ts subscription
  const listeners = new Set<(payload: unknown) => void>();
  const mockCheckbox = { checked: false };
  let malformedCount = 0;

  const handler = (raw: unknown) => {
    if (isAutostartPreferenceChangedEvent(raw)) {
      mockCheckbox.checked = raw.launch_at_login;
    } else {
      malformedCount += 1;
    }
  };
  listeners.add(handler);
  const unlisten = () => {
    listeners.delete(handler);
  };

  const emit = (payload: unknown) => {
    for (const cb of listeners) {
      cb(payload);
    }
  };

  // Tray enables autostart -> Settings updates immediately
  emit({ launch_at_login: true });
  assert.equal(mockCheckbox.checked, true);

  // Malformed event does not corrupt Settings checkbox
  emit({ launch_at_login: "invalid" });
  assert.equal(mockCheckbox.checked, true);
  assert.equal(malformedCount, 1);

  // Tray disables autostart -> Settings updates immediately
  emit({ launch_at_login: false });
  assert.equal(mockCheckbox.checked, false);

  // Cleanup removes listener so later emits do nothing
  unlisten();
  assert.equal(listeners.size, 0);
  emit({ launch_at_login: true });
  assert.equal(mockCheckbox.checked, false);
});
