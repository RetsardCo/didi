import type {
  AutostartPreferenceChangedEvent,
  PresenceServiceStatus,
  PresenceState,
} from "./types.ts";

const VALID_PRESENCE_STATES: ReadonlySet<PresenceState> = new Set<PresenceState>([
  "initializing",
  "disabled",
  "waiting_for_discord",
  "connecting",
  "published",
  "reconnecting",
  "error",
  "exiting",
]);

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * Validates that an incoming `presence://status` payload conforms to the expected shape
 * before updating frontend state, protecting against malformed or unexpected payloads.
 */
export function isPresenceServiceStatus(payload: unknown): payload is PresenceServiceStatus {
  if (!isRecord(payload)) {
    return false;
  }
  const state = payload["state"];
  const statusLabel = payload["status_label"];
  const isConnected = payload["is_connected"];
  const consecutiveFailures = payload["consecutive_failures"];
  const maskedAppId = payload["masked_app_id"];
  const pipeSlot = payload["pipe_slot"];
  const protocolVersion = payload["protocol_version"];
  const lastError = payload["last_error"];

  if (typeof state !== "string" || !VALID_PRESENCE_STATES.has(state as PresenceState)) {
    return false;
  }
  if (typeof statusLabel !== "string") {
    return false;
  }
  if (typeof isConnected !== "boolean") {
    return false;
  }
  if (typeof consecutiveFailures !== "number") {
    return false;
  }
  if (maskedAppId !== null && typeof maskedAppId !== "string") {
    return false;
  }
  if (pipeSlot !== null && typeof pipeSlot !== "number") {
    return false;
  }
  if (protocolVersion !== null && typeof protocolVersion !== "number") {
    return false;
  }
  if (lastError !== null && typeof lastError !== "string") {
    return false;
  }
  return true;
}

/**
 * Validates that an incoming `config://autostart-changed` event payload contains
 * strictly `{ launch_at_login: boolean }` (`FINDING-03`).
 */
export function isAutostartPreferenceChangedEvent(
  payload: unknown,
): payload is AutostartPreferenceChangedEvent {
  if (!isRecord(payload)) {
    return false;
  }
  return typeof payload["launch_at_login"] === "boolean";
}
