import type {
  AppConfigV1,
  NavTabId,
  PresenceServiceStatus,
  ProfileV1,
  ScaffoldStatusDto,
  StartupNoticeDto,
} from "./types";
import { normalizeProfileForSave } from "./validation";

export interface AppState {
  loading: boolean;
  busy: boolean;
  loadError: string | null;
  bannerError: string | null;
  activeTab: NavTabId;
  scaffold: ScaffoldStatusDto | null;
  startupNotice: StartupNoticeDto | null;
  config: AppConfigV1 | null;
  presenceStatus: PresenceServiceStatus | null;
  editorProfileId: string | null;
  editorDraft: ProfileV1 | null;
  previewMode: "draft" | "live";
  revealDefaultAppId: boolean;
  revealOverrideAppId: boolean;
  dismissedClearWarning: string | null;
  awaitingPauseCompletion: boolean;
}

export const appState: AppState = {
  loading: true,
  busy: false,
  loadError: null,
  bannerError: null,
  activeTab: "dashboard",
  scaffold: null,
  startupNotice: null,
  config: null,
  presenceStatus: null,
  editorProfileId: null,
  editorDraft: null,
  previewMode: "draft",
  revealDefaultAppId: false,
  revealOverrideAppId: false,
  dismissedClearWarning: null,
  awaitingPauseCompletion: false,
};

export function cloneProfile(profile: ProfileV1): ProfileV1 {
  return {
    ...profile,
    large_asset: { ...profile.large_asset },
    small_asset: { ...profile.small_asset },
    buttons: profile.buttons.map((b) => ({ ...b })),
  };
}

export function canMutateConfig(): boolean {
  if (appState.loading || !appState.config) {
    return false;
  }
  if (appState.startupNotice && !appState.startupNotice.can_edit_config) {
    return false;
  }
  return true;
}

export function getActiveProfile(): ProfileV1 | null {
  if (!appState.config) {
    return null;
  }
  return (
    appState.config.profiles.find((p) => p.id === appState.config?.active_profile_id) ?? null
  );
}

export function getPersistedEditorProfile(): ProfileV1 | null {
  if (!appState.config || !appState.editorProfileId) {
    return null;
  }
  return appState.config.profiles.find((p) => p.id === appState.editorProfileId) ?? null;
}

export function syncEditorSelection(profileId?: string): void {
  if (!appState.config) {
    appState.editorProfileId = null;
    appState.editorDraft = null;
    return;
  }

  const targetId = profileId ?? appState.editorProfileId ?? appState.config.active_profile_id;
  const found =
    appState.config.profiles.find((p) => p.id === targetId) ??
    appState.config.profiles.find((p) => p.id === appState.config?.active_profile_id) ??
    appState.config.profiles[0] ??
    null;

  if (found) {
    appState.editorProfileId = found.id;
    appState.editorDraft = cloneProfile(found);
  } else {
    appState.editorProfileId = null;
    appState.editorDraft = null;
  }
}

export function isEditorDirty(): boolean {
  const persisted = getPersistedEditorProfile();
  if (!persisted || !appState.editorDraft) {
    return false;
  }
  const a = JSON.stringify(normalizeProfileForSave(persisted));
  const b = JSON.stringify(normalizeProfileForSave(appState.editorDraft));
  return a !== b;
}

export function getEffectiveConfiguredAppId(profile: ProfileV1 | null): string | null {
  if (!profile) {
    return appState.config?.default_application_id ?? null;
  }
  return profile.application_id_override ?? appState.config?.default_application_id ?? null;
}
