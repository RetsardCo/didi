export type PresenceState =
  | "initializing"
  | "disabled"
  | "waiting_for_discord"
  | "connecting"
  | "published"
  | "reconnecting"
  | "error"
  | "exiting";

export type ActivityType = "playing" | "listening" | "watching" | "competing";

export type StatusDisplayType = "name" | "state" | "details";

export type TimestampMode = "none" | "session_elapsed" | "countdown_until";

export type UiLayout = "compact" | "expanded";

export type ImportConflictStrategy = "overwrite" | "keep_both" | "skip";

export type ExternalDocsTarget =
  | "developer_portal"
  | "project_repository"
  | "issue_tracker";

export type StartupNoticeKind =
  | "normal"
  | "created_default"
  | "recovered_from_backup"
  | "reset_to_default_after_corruption"
  | "unsupported_future_schema"
  | "storage_load_failed";

export type ArtworkRef =
  | { kind: "none" }
  | { kind: "portal_key"; key: string }
  | { kind: "external_url"; url: string };

export interface ActivityButton {
  label: string;
  url: string;
}

export interface PresenceActivity {
  activity_type: ActivityType;
  status_display_type: StatusDisplayType | null;
  name_override: string | null;
  details: string | null;
  details_url: string | null;
  state: string | null;
  state_url: string | null;
  large_asset: ArtworkRef;
  large_text: string | null;
  large_url: string | null;
  small_asset: ArtworkRef;
  small_text: string | null;
  small_url: string | null;
  timestamp_mode: TimestampMode;
  start_timestamp: number | null;
  end_timestamp: number | null;
  buttons: ActivityButton[];
}

export interface PresenceServiceStatus {
  state: PresenceState;
  status_label: string;
  masked_app_id: string | null;
  is_connected: boolean;
  active_activity: PresenceActivity | null;
  pending_activity: PresenceActivity | null;
  pipe_slot: number | null;
  protocol_version: number | null;
  consecutive_failures: number;
  last_error: string | null;
}

export interface ScaffoldStatusDto {
  product_name: string;
  expanded_name: string;
  version: string;
  stage: string;
  presence_state: string;
  adapter_kind: string;
  live_ipc_enabled: boolean;
}

export interface StartupNoticeDto {
  kind: StartupNoticeKind;
  message: string | null;
  can_edit_config: boolean;
  found_schema_version: number | null;
  supported_schema_version: number;
  quarantined_filename: string | null;
}

export interface ProfileV1 {
  id: string;
  name: string;
  application_id_override: string | null;
  activity_name_override: string | null;
  activity_type: ActivityType;
  status_display_type: StatusDisplayType;
  details: string | null;
  details_url: string | null;
  state: string | null;
  state_url: string | null;
  large_asset: ArtworkRef;
  large_text: string | null;
  large_url: string | null;
  small_asset: ArtworkRef;
  small_text: string | null;
  small_url: string | null;
  timestamp_mode: TimestampMode;
  custom_timestamp_unix: number | null;
  buttons: ActivityButton[];
}

export interface AppConfigV1 {
  schema_version: number;
  default_application_id: string | null;
  active_profile_id: string;
  presence_enabled: boolean;
  launch_at_login: boolean;
  start_minimized: boolean;
  close_to_tray: boolean;
  ui_layout: UiLayout;
  profiles: ProfileV1[];
}

export interface PreferencesPatch {
  launch_at_login?: boolean;
  start_minimized?: boolean;
  close_to_tray?: boolean;
  ui_layout?: UiLayout;
}

export interface ProfileSummary {
  id: string;
  name: string;
  activity_type: ActivityType;
  details: string | null;
  state: string | null;
}

export interface ProfileConflict {
  existing_id: string;
  existing_name: string;
  imported_id: string;
  imported_name: string;
}

export interface ImportPreview {
  bundle_version: number;
  total_profiles: number;
  new_profiles: ProfileSummary[];
  conflicting_profiles: ProfileConflict[];
  would_exceed_max_profiles: boolean;
  would_exceed_max_profiles_keep_both: boolean;
}

export interface ImportPreviewSessionDto {
  import_token: string;
  preview: ImportPreview;
}

export interface ExportOutcomeDto {
  exported: boolean;
}

export interface AutostartPreferenceChangedEvent {
  launch_at_login: boolean;
}

export type NavTabId = "dashboard" | "profiles" | "editor" | "preview" | "settings";
