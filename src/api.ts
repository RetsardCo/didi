import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  isAutostartPreferenceChangedEvent,
  isPresenceServiceStatus,
} from "./event_validation";
import type {
  AppConfigV1,
  AutostartPreferenceChangedEvent,
  ExportOutcomeDto,
  ExternalDocsTarget,
  ImportConflictStrategy,
  ImportPreviewSessionDto,
  PreferencesPatch,
  PresenceServiceStatus,
  ProfileV1,
  ScaffoldStatusDto,
  StartupNoticeDto,
} from "./types";

const PRESENCE_STATUS_EVENT = "presence://status";
const STARTUP_PREFERENCE_EVENT = "config://autostart-changed";

export { isAutostartPreferenceChangedEvent, isPresenceServiceStatus };

const LOCAL_PATH_PATTERN = /([a-zA-Z]:\\|\\\\[^\\]+\\|\/Users\/|\/home\/)/i;

/**
 * Extracts a safe user-facing error message from a rejected Tauri invoke call,
 * stripping accidental filesystem paths or stack traces if any ever appear.
 */
export function formatCommandError(err: unknown, fallback: string): string {
  const raw = typeof err === "string" ? err : err instanceof Error ? err.message : "";
  const trimmed = raw.trim();
  if (trimmed.length === 0 || LOCAL_PATH_PATTERN.test(trimmed)) {
    return fallback;
  }
  return trimmed;
}

export const didiApi = {
  getScaffoldStatus(): Promise<ScaffoldStatusDto> {
    return invoke<ScaffoldStatusDto>("get_scaffold_status");
  },

  getPresenceStatus(): Promise<PresenceServiceStatus> {
    return invoke<PresenceServiceStatus>("get_presence_status");
  },

  getStartupNotice(): Promise<StartupNoticeDto> {
    return invoke<StartupNoticeDto>("get_startup_notice");
  },

  getAppConfig(): Promise<AppConfigV1> {
    return invoke<AppConfigV1>("get_app_config");
  },

  saveProfile(profile: ProfileV1): Promise<AppConfigV1> {
    return invoke<AppConfigV1>("save_profile", { profile });
  },

  deleteProfile(profileId: string): Promise<AppConfigV1> {
    return invoke<AppConfigV1>("delete_profile", { profileId });
  },

  setActiveProfile(profileId: string): Promise<AppConfigV1> {
    return invoke<AppConfigV1>("set_active_profile", { profileId });
  },

  setDefaultApplicationId(rawAppId: string | null): Promise<AppConfigV1> {
    return invoke<AppConfigV1>("set_default_application_id", { rawAppId });
  },

  setPresenceEnabled(enabled: boolean): Promise<AppConfigV1> {
    return invoke<AppConfigV1>("set_presence_enabled", { enabled });
  },

  retryPresence(): Promise<void> {
    return invoke<void>("retry_presence");
  },

  updatePreferences(patch: PreferencesPatch): Promise<AppConfigV1> {
    return invoke<AppConfigV1>("update_preferences", { patch });
  },

  pickAndPreviewProfileImport(): Promise<ImportPreviewSessionDto | null> {
    return invoke<ImportPreviewSessionDto | null>("pick_and_preview_profile_import");
  },

  confirmProfileImport(
    importToken: string,
    strategy: ImportConflictStrategy,
  ): Promise<AppConfigV1> {
    return invoke<AppConfigV1>("confirm_profile_import", {
      importToken,
      strategy,
    });
  },

  exportProfilesToFile(profileIds: string[] | null): Promise<ExportOutcomeDto> {
    return invoke<ExportOutcomeDto>("export_profiles_to_file", { profileIds });
  },

  openLogsFolder(): Promise<void> {
    return invoke<void>("open_logs_folder");
  },

  openExternalDocsUrl(target: ExternalDocsTarget): Promise<void> {
    return invoke<void>("open_external_docs_url", { target });
  },

  exitApp(): Promise<void> {
    return invoke<void>("exit_app");
  },

  subscribePresenceStatus(
    onStatus: (status: PresenceServiceStatus) => void,
    onMalformed?: () => void,
  ): Promise<UnlistenFn> {
    return listen<unknown>(PRESENCE_STATUS_EVENT, (event) => {
      if (isPresenceServiceStatus(event.payload)) {
        onStatus(event.payload);
      } else if (onMalformed) {
        onMalformed();
      }
    });
  },

  subscribeAutostartChanged(
    onChanged: (payload: AutostartPreferenceChangedEvent) => void,
    onMalformed?: () => void,
  ): Promise<UnlistenFn> {
    return listen<unknown>(STARTUP_PREFERENCE_EVENT, (event) => {
      if (isAutostartPreferenceChangedEvent(event.payload)) {
        onChanged(event.payload);
      } else if (onMalformed) {
        onMalformed();
      }
    });
  },
};
