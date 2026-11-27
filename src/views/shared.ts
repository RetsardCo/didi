import { didiApi, formatCommandError } from "../api";
import { el, openModalDialog, showToast } from "../dom";
import { resolveTogglePresenceToast } from "../presence_feedback";
import {
  appState,
  canMutateConfig,
  cloneProfile,
  getPersistedEditorProfile,
  isEditorDirty,
  syncEditorSelection,
} from "../state";
import type {
  ActivityType,
  ImportConflictStrategy,
  NavTabId,
  PresenceState,
  UiLayout,
} from "../types";
import { normalizeProfileForSave, validateProfileDraft } from "../validation";

export interface AppCallbacks {
  render: () => void;
  navigateTo: (tab: NavTabId) => Promise<void>;
  reloadAll: () => Promise<void>;
}

export function getPresenceBadgeSpec(state: PresenceState): {
  symbol: string;
  label: string;
  toneClass: string;
} {
  switch (state) {
    case "published":
      return { symbol: "●", label: "Published", toneClass: "status-success" };
    case "waiting_for_discord":
      return { symbol: "○", label: "Waiting for Discord", toneClass: "status-warning" };
    case "connecting":
      return { symbol: "◐", label: "Connecting", toneClass: "status-warning" };
    case "reconnecting":
      return { symbol: "↻", label: "Reconnecting", toneClass: "status-warning" };
    case "error":
      return { symbol: "✕", label: "Action Required", toneClass: "status-danger" };
    case "disabled":
      return { symbol: "⏸", label: "Paused", toneClass: "status-neutral" };
    case "exiting":
      return { symbol: "■", label: "Exiting", toneClass: "status-neutral" };
    case "initializing":
    default:
      return { symbol: "○", label: "Starting", toneClass: "status-neutral" };
  }
}

export function activityVerb(type: ActivityType): string {
  switch (type) {
    case "playing":
      return "Playing";
    case "listening":
      return "Listening to";
    case "watching":
      return "Watching";
    case "competing":
      return "Competing in";
  }
}

export function applyLayoutClass(layout: UiLayout): void {
  const shell = document.getElementById("app-shell");
  if (!shell) return;
  shell.classList.remove("layout-compact", "layout-expanded");
  shell.classList.add(layout === "expanded" ? "layout-expanded" : "layout-compact");
}

/**
 * Shared Pause/Resume toggle handler used by both the top header button and Dashboard
 * so clear-failure warnings and pending Pause toasts stay consistent.
 */
export async function togglePresenceEnabled(
  nextEnabled: boolean,
  render: () => void,
): Promise<void> {
  if (!canMutateConfig() || appState.busy) return;
  appState.busy = true;
  appState.bannerError = null;
  if (!nextEnabled) {
    appState.dismissedClearWarning = null;
  }
  render();
  try {
    appState.config = await didiApi.setPresenceEnabled(nextEnabled);
    appState.presenceStatus = await didiApi.getPresenceStatus();
    appState.awaitingPauseCompletion =
      !nextEnabled && appState.presenceStatus.state !== "disabled";
    const toast = resolveTogglePresenceToast(nextEnabled, appState.presenceStatus);
    showToast(toast.message, toast.tone);
  } catch (err) {
    appState.awaitingPauseCompletion = false;
    appState.bannerError = formatCommandError(err, "Unable to toggle presence state.");
  } finally {
    appState.busy = false;
    render();
  }
}

export async function runImportFlow(callbacks: AppCallbacks): Promise<void> {
  if (!canMutateConfig() || appState.busy) {
    return;
  }

  appState.busy = true;
  appState.bannerError = null;
  callbacks.render();

  let session = null;
  try {
    session = await didiApi.pickAndPreviewProfileImport();
  } catch (err) {
    appState.bannerError = formatCommandError(err, "Unable to read the selected profile bundle.");
    appState.busy = false;
    callbacks.render();
    return;
  }

  appState.busy = false;
  callbacks.render();

  if (!session) {
    return;
  }

  const { preview, import_token } = session;
  let chosenStrategy: ImportConflictStrategy = "keep_both";

  const bodyNodes: HTMLElement[] = [
    el("p", {
      className: "helper-text",
      text: `Bundle v${preview.bundle_version} contains ${preview.total_profiles} profile(s): ${preview.new_profiles.length} new, ${preview.conflicting_profiles.length} conflicting.`,
    }),
    el("p", {
      className: "helper-text",
      text: "Security guarantee: Discord Application IDs are never included in exports or modified by imports.",
    }),
  ];

  if (preview.new_profiles.length > 0) {
    const newList = el("div", { className: "field-group" }, [
      el("span", { className: "field-label", text: "New Profiles" }),
      ...preview.new_profiles.map((p) =>
        el("div", {
          className: "helper-text",
          text: `• ${p.name} (${p.id})`,
        }),
      ),
    ]);
    bodyNodes.push(newList);
  }

  if (preview.conflicting_profiles.length > 0) {
    const conflictList = el("div", { className: "field-group" }, [
      el("span", { className: "field-label", text: "Conflicting Profiles" }),
      ...preview.conflicting_profiles.map((c) =>
        el("div", {
          className: "helper-text",
          text: `• "${c.imported_name}" (${c.imported_id}) matches existing "${c.existing_name}"`,
        }),
      ),
    ]);
    bodyNodes.push(conflictList);

    const strategyWrap = el("div", { className: "field-group" }, [
      el("label", { className: "field-label", attrs: { for: "import-strategy-select" }, text: "Conflict Resolution" }),
    ]);
    const select = el("select", {
      id: "import-strategy-select",
      className: "select",
    }) as HTMLSelectElement;

    const strategies: Array<{ value: ImportConflictStrategy; label: string }> = [
      { value: "keep_both", label: "Keep Both (Rename imported duplicates)" },
      { value: "overwrite", label: "Overwrite existing profiles with same ID" },
      { value: "skip", label: "Skip conflicting profiles (import new only)" },
    ];
    for (const s of strategies) {
      const opt = el("option", { text: s.label, attrs: { value: s.value } }) as HTMLOptionElement;
      if (s.value === chosenStrategy) {
        opt.selected = true;
      }
      select.appendChild(opt);
    }
    select.addEventListener("change", () => {
      chosenStrategy = select.value as ImportConflictStrategy;
    });
    strategyWrap.appendChild(select);
    bodyNodes.push(strategyWrap);
  }

  if (preview.would_exceed_max_profiles || preview.would_exceed_max_profiles_keep_both) {
    bodyNodes.push(
      el("p", {
        className: "field-error",
        text: preview.would_exceed_max_profiles
          ? "Warning: Adding these profiles would exceed the 50-profile limit."
          : "Warning: Selecting Keep Both would exceed the 50-profile limit. Choose Skip or Overwrite instead.",
      }),
    );
  }

  const choice = await openModalDialog({
    title: "Review Profile Import",
    body: bodyNodes,
    actions: [
      { label: "Confirm Import", className: "btn btn-primary", value: "confirm" },
      { label: "Cancel", className: "btn btn-secondary", value: "cancel" },
    ],
  });

  if (choice !== "confirm") {
    return;
  }

  appState.busy = true;
  appState.bannerError = null;
  callbacks.render();

  try {
    const updated = await didiApi.confirmProfileImport(import_token, chosenStrategy);
    const keepDirty = isEditorDirty();
    appState.config = updated;
    if (!keepDirty) {
      syncEditorSelection(appState.editorProfileId ?? updated.active_profile_id);
    }
    const status = await didiApi.getPresenceStatus();
    appState.presenceStatus = status;
    showToast("Profiles imported cleanly.", "success");
  } catch (err) {
    appState.bannerError = formatCommandError(err, "Failed to apply profile import.");
  } finally {
    appState.busy = false;
    callbacks.render();
  }
}

export async function runExportFlow(
  profileIds: string[] | null,
  callbacks: AppCallbacks,
): Promise<void> {
  if (appState.busy || !appState.config) {
    return;
  }
  appState.busy = true;
  appState.bannerError = null;
  callbacks.render();

  try {
    const result = await didiApi.exportProfilesToFile(profileIds);
    if (result.exported) {
      showToast(
        profileIds === null ? "Exported all profiles (.json)." : "Exported profile (.json).",
        "success",
      );
    }
  } catch (err) {
    appState.bannerError = formatCommandError(err, "Unable to export profiles.");
  } finally {
    appState.busy = false;
    callbacks.render();
  }
}

/**
 * Prompts when the Activity Editor has unsaved modifications before switching tabs/profiles.
 * Returns `true` if navigation/switch may proceed, `false` if user cancels.
 */
export async function confirmUnsavedEditorNavigation(callbacks: AppCallbacks): Promise<boolean> {
  const draft = appState.editorDraft;
  const persisted = getPersistedEditorProfile();
  if (!draft || !persisted) {
    return true;
  }

  const profileName = persisted.name;
  const validation = validateProfileDraft(draft);

  const actions: Array<{ label: string; className: string; value: string }> = [];
  if (canMutateConfig() && validation.valid) {
    actions.push({ label: "Save Changes", className: "btn btn-primary", value: "save" });
  }
  actions.push({ label: "Discard Edits", className: "btn btn-danger-outline", value: "discard" });
  actions.push({ label: "Stay Here", className: "btn btn-secondary", value: "stay" });

  const body: HTMLElement[] = [
    el("p", {
      text: `You have unsaved changes in "${profileName}".`,
    }),
  ];

  if (!validation.valid) {
    body.push(
      el("p", {
        className: "field-error",
        text: "Current edits have validation errors and cannot be saved until fixed.",
      }),
    );
  }

  const choice = await openModalDialog({
    title: "Unsaved Profile Changes",
    body,
    actions,
  });

  if (choice === "save") {
    try {
      const normalized = normalizeProfileForSave(draft);
      const updated = await didiApi.saveProfile(normalized);
      appState.config = updated;
      const saved = updated.profiles.find((p) => p.id === normalized.id);
      if (saved) {
        appState.editorDraft = cloneProfile(saved);
      }
      appState.presenceStatus = await didiApi.getPresenceStatus();
      showToast("Profile saved.", "success");
      return true;
    } catch (err) {
      appState.bannerError = formatCommandError(err, "Failed to save profile.");
      callbacks.render();
      return false;
    }
  }

  if (choice === "discard") {
    appState.editorDraft = cloneProfile(persisted);
    return true;
  }

  return false;
}
