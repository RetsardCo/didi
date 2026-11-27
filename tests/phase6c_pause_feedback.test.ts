import assert from "node:assert/strict";
import test from "node:test";
import { isPresenceServiceStatus } from "../src/event_validation.ts";
import {
  resolvePausedClearWarning,
  resolvePresenceStatusTransitionToast,
  resolveTogglePresenceToast,
} from "../src/presence_feedback.ts";
import type { PresenceServiceStatus, PresenceState } from "../src/types.ts";

function makeStatus(
  state: PresenceState,
  lastError: string | null = null,
): PresenceServiceStatus {
  return {
    state,
    status_label: state === "disabled" ? "Disabled" : state === "published" ? "Published" : "Error",
    masked_app_id: "****1234",
    is_connected: state === "published",
    active_activity: null,
    pending_activity: null,
    pipe_slot: state === "published" ? 0 : null,
    protocol_version: state === "published" ? 1 : null,
    consecutive_failures: 0,
    last_error: lastError,
  };
}

test("GAP-02: resolvePausedClearWarning surfaces warning only when disabled with error and clears on dismiss or resume", () => {
  const warningMsg =
    "Discord did not respond in time to confirm clearing your activity. Your previous status may remain visible until Discord refreshes.";

  // Disabled with unconfirmed clear shows banner
  assert.equal(resolvePausedClearWarning(makeStatus("disabled", warningMsg), null), warningMsg);

  // Dismissed banner stays hidden for same warning text
  assert.equal(resolvePausedClearWarning(makeStatus("disabled", warningMsg), warningMsg), null);

  // New/different warning text re-surfaces banner even if an older message was dismissed
  const otherMsg = "The connection to Discord ended before activity clearing was confirmed.";
  assert.equal(resolvePausedClearWarning(makeStatus("disabled", otherMsg), warningMsg), otherMsg);

  // Clean disabled or resumed/published states collapse banner (null -> hidden)
  assert.equal(resolvePausedClearWarning(makeStatus("disabled", null), null), null);
  assert.equal(resolvePausedClearWarning(makeStatus("published", null), null), null);
  assert.equal(resolvePausedClearWarning(makeStatus("error", "Some error"), null), null);
  assert.equal(resolvePausedClearWarning(null, null), null);
});

test("GAP-02: resolveTogglePresenceToast avoids claiming clean pause when clear failed or is pending", () => {
  const warningMsg = "The connection to Discord ended before activity clearing was confirmed.";

  assert.deepEqual(resolveTogglePresenceToast(true, makeStatus("waiting_for_discord")), {
    message: "Presence resumed.",
    tone: "neutral",
  });

  assert.deepEqual(resolveTogglePresenceToast(false, makeStatus("disabled", null)), {
    message: "Presence paused.",
    tone: "neutral",
  });

  assert.deepEqual(resolveTogglePresenceToast(false, makeStatus("disabled", warningMsg)), {
    message: warningMsg,
    tone: "warning",
  });

  assert.deepEqual(resolveTogglePresenceToast(false, makeStatus("published", null)), {
    message: "Pausing presence...",
    tone: "neutral",
  });
});

test("GAP-02: resolvePresenceStatusTransitionToast emits final toast on async or tray pause completion", () => {
  const warningMsg = "Discord rejected the activity clear request. Your previous status may still be visible in Discord.";

  // Async UI Pause completing cleanly -> neutral "Presence paused."
  assert.deepEqual(
    resolvePresenceStatusTransitionToast(
      makeStatus("published", null),
      makeStatus("disabled", null),
      true,
    ),
    { message: "Presence paused.", tone: "neutral" },
  );

  // Tray or async UI Pause completing with clear failure -> warning toast
  assert.deepEqual(
    resolvePresenceStatusTransitionToast(
      makeStatus("published", null),
      makeStatus("disabled", warningMsg),
      false,
    ),
    { message: warningMsg, tone: "warning" },
  );

  // Duplicate disabled status emission with same error does not re-toast
  assert.equal(
    resolvePresenceStatusTransitionToast(
      makeStatus("disabled", warningMsg),
      makeStatus("disabled", warningMsg),
      false,
    ),
    null,
  );

  // Transitioning to published does not emit pause toast
  assert.equal(
    resolvePresenceStatusTransitionToast(
      makeStatus("disabled", null),
      makeStatus("published", null),
      false,
    ),
    null,
  );
});

test("GAP-02: isPresenceServiceStatus validates disabled status carrying clear-failure last_error", () => {
  assert.equal(
    isPresenceServiceStatus(
      makeStatus(
        "disabled",
        "Discord did not respond in time to confirm clearing your activity. Your previous status may remain visible until Discord refreshes.",
      ),
    ),
    true,
  );
});
