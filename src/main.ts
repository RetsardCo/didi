import type { UnlistenFn } from "@tauri-apps/api/event";
import { didiApi, formatCommandError } from "./api";
import { clearChildren, createButton, el, getRequiredElement, showToast } from "./dom";
import { createNavigationController } from "./navigation";
import {
  resolvePausedClearWarning,
  resolvePresenceStatusTransitionToast,
} from "./presence_feedback";
import {
  appState,
  canMutateConfig,
  getActiveProfile,
  isEditorDirty,
  syncEditorSelection,
} from "./state";
import type { NavTabId } from "./types";
import { renderDashboardView } from "./views/dashboard";
import { renderEditorView } from "./views/editor";
import { renderPreviewView } from "./views/preview";
import { renderProfilesView } from "./views/profiles";
import { renderSettingsView } from "./views/settings";
import {
  applyLayoutClass,
  confirmUnsavedEditorNavigation,
  getPresenceBadgeSpec,
  togglePresenceEnabled,
  type AppCallbacks,
} from "./views/shared";

const TAB_ORDER: readonly NavTabId[] = [
  "dashboard",
  "profiles",
  "editor",
  "preview",
  "settings",
];

let unlistenPresence: UnlistenFn | null = null;
let unlistenAutostart: UnlistenFn | null = null;
let previewClockTimer: number | null = null;

function stopPreviewClock(): void {
  if (previewClockTimer !== null) {
    window.clearInterval(previewClockTimer);
    previewClockTimer = null;
  }
}

function syncPreviewClock(): void {
  stopPreviewClock();
  const isPreviewVisible =
    appState.activeTab === "preview" ||
    (appState.activeTab === "editor" &&
      appState.config?.ui_layout === "expanded" &&
      window.innerWidth >= 720);

  if (!isPreviewVisible || !appState.config) {
    return;
  }

  const activeMode = appState.activeTab === "editor" ? "draft" : appState.previewMode;
  const activeTimestamp =
    activeMode === "live"
      ? appState.presenceStatus?.active_activity?.timestamp_mode
      : (appState.editorDraft ?? getActiveProfile())?.timestamp_mode;

  if (activeTimestamp === "countdown_until" || (activeMode === "live" && activeTimestamp === "session_elapsed")) {
    previewClockTimer = window.setInterval(() => {
      if (appState.activeTab === "preview") {
        renderPreviewView(getRequiredElement("panel-preview"), callbacks);
      } else if (
        appState.activeTab === "editor" &&
        appState.config?.ui_layout === "expanded" &&
        window.innerWidth >= 720
      ) {
        renderPreviewView(getRequiredElement("panel-editor-docked-preview"), callbacks, {
          docked: true,
        });
      }
    }, 1000);
  }
}

function renderTopHeader(): void {
  const versionEl = getRequiredElement("header-version");
  const activeProfileEl = getRequiredElement("header-active-profile");
  const statusBadgeEl = getRequiredElement("header-status-badge");
  const toggleBtn = getRequiredElement<HTMLButtonElement>("header-toggle-btn");

  versionEl.textContent = appState.scaffold ? `v${appState.scaffold.version}` : "v0.1.0";

  if (appState.loading) {
    activeProfileEl.textContent = "Loading active profile...";
    statusBadgeEl.textContent = "○ Starting";
    statusBadgeEl.className = "status-pill status-neutral";
    toggleBtn.disabled = true;
    return;
  }

  const active = getActiveProfile();
  activeProfileEl.textContent = active
    ? `Active: ${active.name}`
    : "No active profile";

  if (appState.presenceStatus) {
    const spec = getPresenceBadgeSpec(appState.presenceStatus.state);
    statusBadgeEl.textContent = `${spec.symbol} ${spec.label}`;
    statusBadgeEl.className = `status-pill ${spec.toneClass}`;
  } else {
    statusBadgeEl.textContent = "✕ Unavailable";
    statusBadgeEl.className = "status-pill status-danger";
  }

  if (appState.config) {
    toggleBtn.textContent = appState.config.presence_enabled ? "Pause" : "Resume";
    toggleBtn.disabled = !canMutateConfig() || appState.busy;
  } else {
    toggleBtn.disabled = true;
  }
}

function renderGlobalNotices(): void {
  const stack = getRequiredElement("global-notices");
  clearChildren(stack);

  const notices: HTMLElement[] = [];

  if (appState.startupNotice) {
    const sn = appState.startupNotice;
    if (sn.kind === "unsupported_future_schema" || sn.kind === "storage_load_failed") {
      notices.push(
        el(
          "div",
          {
            className: "notice-banner notice-banner-danger",
            attrs: { role: "alert" },
          },
          [
            el("span", {
              text:
                sn.message ??
                "Configuration cannot be edited safely. Please upgrade or inspect logs.",
            }),
          ],
        ),
      );
    } else if (
      sn.kind === "recovered_from_backup" ||
      sn.kind === "reset_to_default_after_corruption"
    ) {
      const suffix = sn.quarantined_filename
        ? ` (Quarantined: ${sn.quarantined_filename})`
        : "";
      notices.push(
        el(
          "div",
          {
            className: "notice-banner notice-banner-warning",
            attrs: { role: "status" },
          },
          [
            el("span", {
              text: `${sn.message ?? "Configuration was recovered on startup."}${suffix}`,
            }),
            el("div", { className: "notice-banner-actions" }, [
              createButton("Dismiss", "btn btn-secondary btn-sm", () => {
                appState.startupNotice = null;
                renderGlobalNotices();
              }),
            ]),
          ],
        ),
      );
    }
  }

  if (appState.bannerError) {
    notices.push(
      el(
        "div",
        {
          className: "notice-banner notice-banner-danger",
          attrs: { role: "alert" },
        },
        [
          el("span", { text: appState.bannerError }),
          el("div", { className: "notice-banner-actions" }, [
            createButton("Dismiss", "btn btn-secondary btn-sm", () => {
              appState.bannerError = null;
              renderGlobalNotices();
            }),
          ]),
        ],
      ),
    );
  }

  const clearWarning = resolvePausedClearWarning(
    appState.presenceStatus,
    appState.dismissedClearWarning,
  );
  if (clearWarning) {
    notices.push(
      el(
        "div",
        {
          className: "notice-banner notice-banner-warning",
          attrs: { role: "status" },
        },
        [
          el("span", { text: clearWarning }),
          el("div", { className: "notice-banner-actions" }, [
            createButton("Dismiss", "btn btn-secondary btn-sm", () => {
              appState.dismissedClearWarning = clearWarning;
              renderGlobalNotices();
            }),
          ]),
        ],
      ),
    );
  }

  for (const n of notices) {
    stack.appendChild(n);
  }
  stack.hidden = notices.length === 0;
}

function renderTabs(): void {
  for (const tabId of TAB_ORDER) {
    const tabBtn = getRequiredElement(`tab-${tabId}`);
    const isActive = tabId === appState.activeTab;
    tabBtn.classList.toggle("is-active", isActive);
    tabBtn.setAttribute("aria-selected", isActive ? "true" : "false");
    tabBtn.setAttribute("tabindex", isActive ? "0" : "-1");
  }

  const panelDashboard = getRequiredElement("panel-dashboard");
  const panelProfiles = getRequiredElement("panel-profiles");
  const panelEditorWorkspace = getRequiredElement("panel-editor-workspace");
  const panelDockedPreview = getRequiredElement("panel-editor-docked-preview");
  const panelPreview = getRequiredElement("panel-preview");
  const panelSettings = getRequiredElement("panel-settings");

  panelDashboard.hidden = appState.activeTab !== "dashboard";
  panelProfiles.hidden = appState.activeTab !== "profiles";
  panelEditorWorkspace.hidden = appState.activeTab !== "editor";
  panelPreview.hidden = appState.activeTab !== "preview";
  panelSettings.hidden = appState.activeTab !== "settings";

  const showDockedPreview =
    appState.activeTab === "editor" &&
    appState.config?.ui_layout === "expanded" &&
    window.innerWidth >= 720;
  panelEditorWorkspace.classList.toggle("has-docked-preview", showDockedPreview);
  panelDockedPreview.hidden = !showDockedPreview;
}

function renderActiveScreen(): void {
  switch (appState.activeTab) {
    case "dashboard":
      renderDashboardView(getRequiredElement("panel-dashboard"), callbacks);
      break;
    case "profiles":
      renderProfilesView(getRequiredElement("panel-profiles"), callbacks);
      break;
    case "editor": {
      const dockedPreview = getRequiredElement("panel-editor-docked-preview");
      renderEditorView(getRequiredElement("panel-editor"), {
        ...callbacks,
        onDraftUpdated: () => {
          if (!dockedPreview.hidden) {
            renderPreviewView(dockedPreview, callbacks, { docked: true });
          }
        },
      });
      if (!dockedPreview.hidden) {
        renderPreviewView(dockedPreview, callbacks, { docked: true });
      }
      break;
    }
    case "preview":
      renderPreviewView(getRequiredElement("panel-preview"), callbacks);
      break;
    case "settings":
      renderSettingsView(getRequiredElement("panel-settings"), callbacks);
      break;
  }
}

function render(): void {
  if (appState.config) {
    applyLayoutClass(appState.config.ui_layout);
  }
  renderTopHeader();
  renderGlobalNotices();
  renderTabs();
  renderActiveScreen();
  syncPreviewClock();
}

const navigationController = createNavigationController({
  getCurrentTab: () => appState.activeTab,
  setCurrentTab: (tab) => {
    appState.activeTab = tab;
  },
  isEditorDirty,
  confirmUnsaved: () => confirmUnsavedEditorNavigation(callbacks),
  onNavigateComplete: () => {
    render();
    getRequiredElement("screen-viewport").scrollTop = 0;
  },
  onNavigateCancelled: (currentTab) => {
    const activeEl = document.activeElement;
    if (activeEl instanceof HTMLElement && activeEl.id.startsWith("tab-")) {
      getRequiredElement(`tab-${currentTab}`).focus();
    }
  },
});

async function navigateTo(targetTab: NavTabId): Promise<void> {
  await navigationController.navigateTo(targetTab);
}

async function reloadAll(): Promise<void> {
  appState.loading = true;
  appState.loadError = null;
  appState.bannerError = null;
  render();

  try {
    const [scaffold, startupNotice, config, presenceStatus] = await Promise.all([
      didiApi.getScaffoldStatus(),
      didiApi.getStartupNotice(),
      didiApi.getAppConfig(),
      didiApi.getPresenceStatus(),
    ]);
    appState.scaffold = scaffold;
    appState.startupNotice = startupNotice;
    appState.config = config;
    appState.presenceStatus = presenceStatus;
    syncEditorSelection(config.active_profile_id);
  } catch (err) {
    appState.loadError = formatCommandError(
      err,
      "Unable to load initial configuration from the DiDi host process.",
    );
  } finally {
    appState.loading = false;
    render();
  }
}

const callbacks: AppCallbacks = {
  render,
  navigateTo,
  reloadAll,
};

function setupNavigationAndHeader(): void {
  for (const tabId of TAB_ORDER) {
    const btn = getRequiredElement(`tab-${tabId}`);
    btn.addEventListener("click", () => {
      void navigateTo(tabId);
    });
    btn.addEventListener("keydown", (event) => {
      const currentIdx = TAB_ORDER.indexOf(tabId);
      let nextIdx: number | null = null;
      if (event.key === "ArrowRight") {
        nextIdx = (currentIdx + 1) % TAB_ORDER.length;
      } else if (event.key === "ArrowLeft") {
        nextIdx = (currentIdx - 1 + TAB_ORDER.length) % TAB_ORDER.length;
      } else if (event.key === "Home") {
        nextIdx = 0;
      } else if (event.key === "End") {
        nextIdx = TAB_ORDER.length - 1;
      }
      if (nextIdx !== null) {
        event.preventDefault();
        const targetId = TAB_ORDER[nextIdx];
        const targetEl = getRequiredElement(`tab-${targetId}`);
        targetEl.focus();
        void navigateTo(targetId);
      }
    });
  }

  const headerToggleBtn = getRequiredElement<HTMLButtonElement>("header-toggle-btn");
  headerToggleBtn.addEventListener("click", () => {
    if (!appState.config) return;
    void togglePresenceEnabled(!appState.config.presence_enabled, render);
  });

  window.addEventListener("resize", () => {
    if (appState.activeTab === "editor" && appState.config?.ui_layout === "expanded") {
      renderTabs();
      const dockedPreview = getRequiredElement("panel-editor-docked-preview");
      if (!dockedPreview.hidden) {
        renderPreviewView(dockedPreview, callbacks, { docked: true });
      }
      syncPreviewClock();
    }
  });
}

async function init(): Promise<void> {
  setupNavigationAndHeader();

  try {
    unlistenPresence = await didiApi.subscribePresenceStatus(
      (status) => {
        const prevStatus = appState.presenceStatus;
        const prevState = prevStatus?.state;
        appState.presenceStatus = status;
        if (status.state !== "disabled" || !status.last_error) {
          appState.dismissedClearWarning = null;
        }
        if (!appState.busy) {
          const transitionToast = resolvePresenceStatusTransitionToast(
            prevStatus,
            status,
            appState.awaitingPauseCompletion,
          );
          if (status.state === "disabled" || status.state === "published") {
            appState.awaitingPauseCompletion = false;
          }
          if (transitionToast) {
            showToast(transitionToast.message, transitionToast.tone);
          }
        } else if (status.state === "disabled" || status.state === "published") {
          appState.awaitingPauseCompletion = false;
        }
        if (
          prevState !== status.state &&
          (prevState === "disabled" || status.state === "disabled") &&
          appState.config
        ) {
          void didiApi
            .getAppConfig()
            .then((latest) => {
              if (appState.config) {
                appState.config = {
                  ...appState.config,
                  presence_enabled: latest.presence_enabled,
                  active_profile_id: latest.active_profile_id,
                  launch_at_login: latest.launch_at_login,
                };
                renderTopHeader();
                renderGlobalNotices();
                if (appState.activeTab === "dashboard") {
                  renderDashboardView(getRequiredElement("panel-dashboard"), callbacks);
                }
              }
            })
            .catch(() => {
              // Ignore transient fetch failure; header already updated from event.
            });
        }
        renderTopHeader();
        renderGlobalNotices();
        if (appState.activeTab === "dashboard") {
          renderDashboardView(getRequiredElement("panel-dashboard"), callbacks);
        } else if (appState.activeTab === "preview" && appState.previewMode === "live") {
          renderPreviewView(getRequiredElement("panel-preview"), callbacks);
        }
        syncPreviewClock();
      },
      () => {
        appState.bannerError = "Received an unexpected status update from the host process.";
        renderGlobalNotices();
      },
    );
  } catch {
    // Event bridge attaches only while window exists; status remains queryable via commands.
  }

  try {
    unlistenAutostart = await didiApi.subscribeAutostartChanged(
      (payload) => {
        if (appState.config) {
          appState.config = {
            ...appState.config,
            launch_at_login: payload.launch_at_login,
          };
          const launchCheckbox = document.getElementById("pref-launch-login");
          if (launchCheckbox instanceof HTMLInputElement) {
            launchCheckbox.checked = payload.launch_at_login;
          }
          if (appState.activeTab === "dashboard") {
            renderDashboardView(getRequiredElement("panel-dashboard"), callbacks);
          }
        }
      },
      () => {
        appState.bannerError = "Received an unexpected preference update from the host process.";
        renderGlobalNotices();
      },
    );
  } catch {
    // Event bridge attaches only while window exists; preferences remain queryable via commands.
  }

  window.addEventListener("beforeunload", () => {
    stopPreviewClock();
    if (unlistenPresence) {
      unlistenPresence();
      unlistenPresence = null;
    }
    if (unlistenAutostart) {
      unlistenAutostart();
      unlistenAutostart = null;
    }
  });

  await reloadAll();
}

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", () => {
    void init();
  });
} else {
  void init();
}
