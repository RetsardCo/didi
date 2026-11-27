import type { PresenceServiceStatus } from "./types.ts";

export interface PauseToastFeedback {
  message: string;
  tone: "neutral" | "warning";
}

/**
 * Resolves the cross-tab `#global-notices` warning message when Discord activity
 * clearing could not be confirmed on Pause (`GAP-02`).
 */
export function resolvePausedClearWarning(
  status: PresenceServiceStatus | null,
  dismissedWarning: string | null,
): string | null {
  if (!status || status.state !== "disabled" || !status.last_error) {
    return null;
  }
  if (dismissedWarning === status.last_error) {
    return null;
  }
  return status.last_error;
}

/**
 * Resolves the immediate toast shown when the user toggles Presence via the UI,
 * avoiding claiming `"Presence paused."` if clearing failed or is still in flight.
 */
export function resolveTogglePresenceToast(
  nextEnabled: boolean,
  status: PresenceServiceStatus | null,
): PauseToastFeedback {
  if (nextEnabled) {
    return { message: "Presence resumed.", tone: "neutral" };
  }
  if (status?.state === "disabled") {
    if (status.last_error) {
      return { message: status.last_error, tone: "warning" };
    }
    return { message: "Presence paused.", tone: "neutral" };
  }
  return { message: "Pausing presence...", tone: "neutral" };
}

/**
 * Resolves an event-driven toast when `presence://status` reports transition into
 * `"disabled"` (completing an async UI Pause or a Tray Pause).
 */
export function resolvePresenceStatusTransitionToast(
  prevStatus: PresenceServiceStatus | null,
  nextStatus: PresenceServiceStatus,
  awaitingPauseCompletion: boolean,
): PauseToastFeedback | null {
  if (nextStatus.state !== "disabled") {
    return null;
  }

  const enteredDisabled = prevStatus?.state !== "disabled";
  const errorAppeared =
    nextStatus.last_error !== null && nextStatus.last_error !== prevStatus?.last_error;

  if (nextStatus.last_error && (enteredDisabled || errorAppeared || awaitingPauseCompletion)) {
    return { message: nextStatus.last_error, tone: "warning" };
  }

  if (awaitingPauseCompletion) {
    return { message: "Presence paused.", tone: "neutral" };
  }

  return null;
}
