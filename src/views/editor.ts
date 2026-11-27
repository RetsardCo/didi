import { didiApi, formatCommandError } from "../api";
import { clearChildren, createButton, el, showToast } from "../dom";
import {
  appState,
  canMutateConfig,
  cloneProfile,
  isEditorDirty,
  syncEditorSelection,
} from "../state";
import type {
  ActivityType,
  ArtworkRef,
  StatusDisplayType,
  TimestampMode,
} from "../types";
import {
  MAX_BUTTON_LABEL_LEN,
  MAX_BUTTON_URL_LEN,
  MAX_BUTTONS_COUNT,
  MAX_PORTAL_KEY_LEN,
  MAX_PROFILE_NAME_LEN,
  MAX_TEXT_LEN,
  MAX_URL_LEN,
  normalizeOptionalText,
  normalizeProfileForSave,
  validateProfileDraft,
  type ProfileValidationMap,
} from "../validation";
import { confirmUnsavedEditorNavigation, type AppCallbacks } from "./shared";

function toDatetimeLocalValue(unixSecs: number | null): string {
  if (unixSecs === null || !Number.isFinite(unixSecs) || unixSecs <= 0) {
    return "";
  }
  const d = new Date(unixSecs * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(
    d.getHours(),
  )}:${pad(d.getMinutes())}`;
}

function fromDatetimeLocalValue(val: string): number | null {
  if (!val) return null;
  const ms = Date.parse(val);
  if (Number.isNaN(ms) || ms <= 0) return null;
  return Math.floor(ms / 1000);
}

export interface EditorCallbacks extends AppCallbacks {
  onDraftUpdated: () => void;
}

export function renderEditorView(container: HTMLElement, callbacks: EditorCallbacks): void {
  clearChildren(container);

  if (appState.loading || !appState.config) {
    container.appendChild(
      el("section", { className: "card" }, [
        el("p", { className: "helper-text", text: "Loading Activity Editor..." }),
      ]),
    );
    return;
  }

  if (!appState.editorDraft) {
    syncEditorSelection();
  }

  const draft = appState.editorDraft;
  const config = appState.config;
  if (!draft) {
    container.appendChild(
      el("section", { className: "card" }, [
        el("p", { className: "helper-text", text: "No profile selected." }),
      ]),
    );
    return;
  }

  const activeDraft = draft;
  const editable = canMutateConfig() && !appState.busy;
  const isEditingActive = draft.id === config.active_profile_id;
  const publishable = isEditingActive && config.presence_enabled;

  // Dynamic validation & status nodes updated in-place without destroying focus
  const errorRefs = new Map<string, { input: HTMLElement; err: HTMLParagraphElement }>();
  const counterRefs = new Map<string, { counter: HTMLSpanElement; max: number }>();

  const validationSummaryEl = el("span", { className: "chip" });
  const dirtyBadgeEl = el("span", { className: "chip" });
  const discardBtn = createButton("Discard", "btn btn-secondary", () => {
    syncEditorSelection(draft.id);
    renderEditorView(container, callbacks);
    callbacks.onDraftUpdated();
  });
  const saveBtn = createButton(
    publishable ? "Save & Publish" : "Save Profile",
    "btn btn-primary",
    () => {
      void handleSave();
    },
  );

  function refreshValidationAndBar(): boolean {
    if (!appState.editorDraft) return false;
    const result = validateProfileDraft(appState.editorDraft);

    const setFieldError = (key: string, message: string | undefined) => {
      const ref = errorRefs.get(key);
      if (!ref) return;
      if (message) {
        ref.input.setAttribute("aria-invalid", "true");
        ref.err.textContent = message;
        ref.err.hidden = false;
      } else {
        ref.input.removeAttribute("aria-invalid");
        ref.err.textContent = "";
        ref.err.hidden = true;
      }
    };

    setFieldError("name", result.errors.name);
    setFieldError("application_id_override", result.errors.application_id_override);
    setFieldError("activity_name_override", result.errors.activity_name_override);
    setFieldError("details", result.errors.details);
    setFieldError("details_url", result.errors.details_url);
    setFieldError("state", result.errors.state);
    setFieldError("state_url", result.errors.state_url);
    setFieldError("large_asset", result.errors.large_asset);
    setFieldError("large_text", result.errors.large_text);
    setFieldError("large_url", result.errors.large_url);
    setFieldError("small_asset", result.errors.small_asset);
    setFieldError("small_text", result.errors.small_text);
    setFieldError("small_url", result.errors.small_url);
    setFieldError("custom_timestamp_unix", result.errors.custom_timestamp_unix);

    result.errors.buttons.forEach((bErr, idx) => {
      setFieldError(`button_label_${idx}`, bErr.label);
      setFieldError(`button_url_${idx}`, bErr.url);
    });

    const dirty = isEditorDirty();
    dirtyBadgeEl.textContent = dirty ? "Unsaved edits" : "Saved";
    dirtyBadgeEl.className = dirty ? "chip chip-accent" : "chip";

    const errCount = countErrors(result.errors);
    if (result.valid) {
      validationSummaryEl.textContent = "Valid";
      validationSummaryEl.className = "status-pill status-success";
    } else {
      validationSummaryEl.textContent = `${errCount} validation issue${errCount === 1 ? "" : "s"}`;
      validationSummaryEl.className = "status-pill status-danger";
    }

    discardBtn.disabled = !dirty || appState.busy;
    saveBtn.disabled = !editable || !result.valid;
    return result.valid;
  }

  function updateCounter(key: string, value: string): void {
    const ref = counterRefs.get(key);
    if (!ref) return;
    const len = Array.from(value.trim()).length;
    ref.counter.textContent = `${len} / ${ref.max}`;
    if (len > ref.max) {
      ref.counter.classList.add("is-over");
    } else {
      ref.counter.classList.remove("is-over");
    }
  }

  function createTextField(opts: {
    key: string;
    label: string;
    value: string;
    maxLen?: number;
    placeholder?: string;
    experimental?: boolean;
    mono?: boolean;
    disabled?: boolean;
    helper?: string;
    onInput: (val: string) => void;
  }): HTMLElement {
    const inputId = `editor-field-${opts.key}`;
    const errId = `editor-err-${opts.key}`;

    const labelEl = el("label", {
      className: "field-label",
      attrs: { for: inputId },
      text: opts.label,
    });
    if (opts.experimental) {
      labelEl.appendChild(
        el("span", { className: "experimental-tag", text: "D-016" }),
      );
    }

    const labelRow = el("div", { className: "field-label-row" }, [labelEl]);
    if (opts.maxLen !== undefined) {
      const counter = el("span", {
        className: "char-counter",
        text: `${Array.from(opts.value.trim()).length} / ${opts.maxLen}`,
      });
      counterRefs.set(opts.key, { counter, max: opts.maxLen });
      labelRow.appendChild(counter);
    }

    const input = el("input", {
      id: inputId,
      className: `input${opts.mono ? " mono" : ""}`,
      attrs: {
        type: "text",
        "aria-describedby": errId,
        placeholder: opts.placeholder ?? "",
      },
    }) as HTMLInputElement;
    input.value = opts.value;
    input.disabled = opts.disabled ?? !editable;

    const errEl = el("p", { id: errId, className: "field-error" });
    errEl.hidden = true;
    errorRefs.set(opts.key, { input, err: errEl });

    input.addEventListener("input", () => {
      opts.onInput(input.value);
      updateCounter(opts.key, input.value);
      refreshValidationAndBar();
      callbacks.onDraftUpdated();
    });

    const group = el("div", { className: "field-group" }, [labelRow, input]);
    if (opts.helper) {
      group.appendChild(el("p", { className: "helper-text", text: opts.helper }));
    }
    group.appendChild(errEl);
    return group;
  }

  async function handleSave(): Promise<void> {
    if (!appState.editorDraft || !canMutateConfig() || appState.busy) return;
    if (!refreshValidationAndBar()) return;

    appState.busy = true;
    appState.bannerError = null;
    saveBtn.disabled = true;
    callbacks.render();

    try {
      const normalized = normalizeProfileForSave(appState.editorDraft);
      const updated = await didiApi.saveProfile(normalized);
      appState.config = updated;
      const saved = updated.profiles.find((p) => p.id === normalized.id);
      if (saved) {
        appState.editorDraft = cloneProfile(saved);
      }
      appState.presenceStatus = await didiApi.getPresenceStatus();
      showToast(
        publishable
          ? `Saved "${normalized.name}" and queued Rich Presence update.`
          : `Saved profile "${normalized.name}".`,
        "success",
      );
    } catch (err) {
      appState.bannerError = formatCommandError(err, "Unable to save profile.");
    } finally {
      appState.busy = false;
      callbacks.render();
    }
  }

  // Profile Switcher header inside Editor
  const profilePicker = el("select", {
    id: "editor-profile-picker",
    className: "select",
    attrs: { "aria-label": "Profile to edit" },
  }) as HTMLSelectElement;
  for (const p of config.profiles) {
    const opt = el("option", {
      text: p.id === config.active_profile_id ? `${p.name} (Active)` : p.name,
      attrs: { value: p.id },
    }) as HTMLOptionElement;
    if (p.id === draft.id) opt.selected = true;
    profilePicker.appendChild(opt);
  }
  profilePicker.addEventListener("change", () => {
    void (async () => {
      const nextId = profilePicker.value;
      if (nextId === draft.id) return;
      if (isEditorDirty()) {
        const proceed = await confirmUnsavedEditorNavigation(callbacks);
        if (!proceed) {
          profilePicker.value = draft.id;
          return;
        }
      }
      syncEditorSelection(nextId);
      callbacks.render();
    })();
  });

  // 1. Identity Card
  const overrideInputId = "editor-field-application_id_override";
  const overrideErrId = "editor-err-application_id_override";
  const overrideInput = el("input", {
    id: overrideInputId,
    className: "input mono",
    attrs: {
      type: appState.revealOverrideAppId ? "text" : "password",
      placeholder: "Optional (uses Settings default if blank)",
      autocomplete: "off",
      "aria-describedby": overrideErrId,
    },
  }) as HTMLInputElement;
  overrideInput.value = draft.application_id_override ?? "";
  overrideInput.disabled = !editable;
  const overrideErr = el("p", { id: overrideErrId, className: "field-error" });
  overrideErr.hidden = true;
  errorRefs.set("application_id_override", { input: overrideInput, err: overrideErr });

  overrideInput.addEventListener("input", () => {
    draft.application_id_override = normalizeOptionalText(overrideInput.value);
    refreshValidationAndBar();
    callbacks.onDraftUpdated();
  });

  const revealOverrideBtn = createButton(
    appState.revealOverrideAppId ? "Hide" : "Reveal",
    "btn btn-secondary btn-sm",
    () => {
      appState.revealOverrideAppId = !appState.revealOverrideAppId;
      overrideInput.type = appState.revealOverrideAppId ? "text" : "password";
      revealOverrideBtn.textContent = appState.revealOverrideAppId ? "Hide" : "Reveal";
    },
  );

  const identityCard = el("section", { className: "card" }, [
    el("div", { className: "card-header" }, [
      el("h2", { className: "section-title", text: "Profile Identity" }),
      el("span", { className: "mono helper-text", text: `ID: ${draft.id}` }),
    ]),
    el("div", { className: "field-group" }, [
      el("label", {
        className: "field-label",
        attrs: { for: "editor-profile-picker" },
        text: "Editing Profile",
      }),
      profilePicker,
    ]),
    createTextField({
      key: "name",
      label: "Profile Name",
      value: draft.name,
      maxLen: MAX_PROFILE_NAME_LEN,
      placeholder: "Required (1–64 chars)",
      onInput: (val) => {
        draft.name = val;
      },
    }),
    el("div", { className: "field-group" }, [
      el("label", {
        className: "field-label",
        attrs: { for: overrideInputId },
        text: "Application ID Override (Optional)",
      }),
      el("div", { className: "input-with-action" }, [
        overrideInput,
        revealOverrideBtn,
        createButton(
          "Clear",
          "btn btn-secondary btn-sm",
          () => {
            overrideInput.value = "";
            draft.application_id_override = null;
            refreshValidationAndBar();
            callbacks.onDraftUpdated();
          },
          { disabled: !editable },
        ),
      ]),
      el("p", {
        className: "helper-text",
        text: "17–20 digit Discord snowflake. Leave empty to use the Default Application ID in Settings.",
      }),
      overrideErr,
    ]),
  ]);
  container.appendChild(identityCard);

  // 2. Activity Core + D-016 notice
  const actTypeSelect = el("select", {
    id: "editor-activity-type",
    className: "select",
  }) as HTMLSelectElement;
  const actTypes: Array<{ value: ActivityType; label: string }> = [
    { value: "playing", label: "Playing (0)" },
    { value: "listening", label: "Listening to (2)" },
    { value: "watching", label: "Watching (3)" },
    { value: "competing", label: "Competing in (5)" },
  ];
  for (const t of actTypes) {
    const opt = el("option", { text: t.label, attrs: { value: t.value } }) as HTMLOptionElement;
    if (t.value === draft.activity_type) opt.selected = true;
    actTypeSelect.appendChild(opt);
  }
  actTypeSelect.disabled = !editable;
  actTypeSelect.addEventListener("change", () => {
    draft.activity_type = actTypeSelect.value as ActivityType;
    refreshValidationAndBar();
    callbacks.onDraftUpdated();
  });

  const statusDisplaySelect = el("select", {
    id: "editor-status-display",
    className: "select",
  }) as HTMLSelectElement;
  const statusDisplayOpts: Array<{ value: StatusDisplayType; label: string }> = [
    { value: "name", label: "Application / Activity Name (0)" },
    { value: "state", label: "State Line (1)" },
    { value: "details", label: "Details Line (2)" },
  ];
  for (const s of statusDisplayOpts) {
    const opt = el("option", { text: s.label, attrs: { value: s.value } }) as HTMLOptionElement;
    if (s.value === draft.status_display_type) opt.selected = true;
    statusDisplaySelect.appendChild(opt);
  }
  statusDisplaySelect.disabled = !editable;
  statusDisplaySelect.addEventListener("change", () => {
    draft.status_display_type = statusDisplaySelect.value as StatusDisplayType;
    refreshValidationAndBar();
    callbacks.onDraftUpdated();
  });

  const coreCard = el("section", { className: "card" }, [
    el("h2", { className: "section-title", text: "Activity Core" }),
    el("div", { className: "field-row-2" }, [
      el("div", { className: "field-group" }, [
        el("label", {
          className: "field-label",
          attrs: { for: "editor-activity-type" },
          text: "Activity Type",
        }),
        actTypeSelect,
      ]),
      el("div", { className: "field-group" }, [
        el("label", {
          className: "field-label",
          attrs: { for: "editor-status-display" },
        }, [
          document.createTextNode("Status Display "),
          el("span", { className: "experimental-tag", text: "D-016" }),
        ]),
        statusDisplaySelect,
      ]),
    ]),
    createTextField({
      key: "activity_name_override",
      label: "Activity Name Override",
      value: draft.activity_name_override ?? "",
      maxLen: MAX_TEXT_LEN,
      experimental: true,
      placeholder: "Optional (2–128 chars; defaults to Portal App Name)",
      onInput: (val) => {
        draft.activity_name_override = val.length === 0 ? null : val;
      },
    }),
    el("p", {
      className: "helper-text",
      text: "Fields tagged D-016 (name override, status display, clickable URLs, buttons) are supported in DiDi's wire model and pending live verification against a user-supplied Application ID.",
    }),
  ]);
  container.appendChild(coreCard);

  // 3. Text Lines & Clickable URLs
  container.appendChild(
    el("section", { className: "card" }, [
      el("h2", { className: "section-title", text: "Details & State" }),
      createTextField({
        key: "details",
        label: "Details (Line 1)",
        value: draft.details ?? "",
        maxLen: MAX_TEXT_LEN,
        placeholder: "e.g., Working on DiDi UI (2–128 chars)",
        onInput: (val) => {
          draft.details = val.length === 0 ? null : val;
        },
      }),
      createTextField({
        key: "details_url",
        label: "Details Clickable URL",
        value: draft.details_url ?? "",
        maxLen: MAX_URL_LEN,
        experimental: true,
        mono: true,
        placeholder: "https://example.com (optional)",
        onInput: (val) => {
          draft.details_url = val.length === 0 ? null : val;
        },
      }),
      createTextField({
        key: "state",
        label: "State (Line 2)",
        value: draft.state ?? "",
        maxLen: MAX_TEXT_LEN,
        placeholder: "e.g., Local-first Windows utility (2–128 chars)",
        onInput: (val) => {
          draft.state = val.length === 0 ? null : val;
        },
      }),
      createTextField({
        key: "state_url",
        label: "State Clickable URL",
        value: draft.state_url ?? "",
        maxLen: MAX_URL_LEN,
        experimental: true,
        mono: true,
        placeholder: "https://example.com (optional)",
        onInput: (val) => {
          draft.state_url = val.length === 0 ? null : val;
        },
      }),
    ]),
  );

  // 4. Artwork Section Helper
  function buildArtworkSection(slot: "large" | "small"): HTMLElement {
    const isLarge = slot === "large";
    const assetKey = isLarge ? "large_asset" : "small_asset";
    const textKey = isLarge ? "large_text" : "small_text";
    const urlKey = isLarge ? "large_url" : "small_url";
    const currentAsset = isLarge ? activeDraft.large_asset : activeDraft.small_asset;

    const kindSelectId = `editor-${slot}-kind`;
    const kindSelect = el("select", {
      id: kindSelectId,
      className: "select",
    }) as HTMLSelectElement;
    const kinds: Array<{ value: ArtworkRef["kind"]; label: string }> = [
      { value: "none", label: "None" },
      { value: "portal_key", label: "Developer Portal Asset Key" },
      { value: "external_url", label: "External HTTPS Image URL" },
    ];
    for (const k of kinds) {
      const opt = el("option", { text: k.label, attrs: { value: k.value } }) as HTMLOptionElement;
      if (k.value === currentAsset.kind) opt.selected = true;
      kindSelect.appendChild(opt);
    }
    kindSelect.disabled = !editable;

    const valueWrap = el("div", { className: "field-group" });
    const renderAssetValueInput = (asset: ArtworkRef) => {
      clearChildren(valueWrap);
      errorRefs.delete(assetKey);
      counterRefs.delete(assetKey);

      if (asset.kind === "none") {
        return;
      }

      const valueField = createTextField({
        key: assetKey,
        label: asset.kind === "portal_key" ? "Portal Asset Key" : "External Image HTTPS URL",
        value: asset.kind === "portal_key" ? asset.key : asset.url,
        maxLen: MAX_PORTAL_KEY_LEN,
        mono: true,
        placeholder:
          asset.kind === "portal_key"
            ? "my_asset_key (auto-lowercased)"
            : "https://cdn.example.com/icon.png",
        helper:
          asset.kind === "portal_key"
            ? "Must match a Rich Presence Art Asset uploaded in Discord Developer Portal."
            : "Must use public https:// host (never fetched directly by DiDi UI).",
        onInput: (val) => {
          const updated: ArtworkRef =
            asset.kind === "portal_key"
              ? { kind: "portal_key", key: val }
              : { kind: "external_url", url: val };
          if (isLarge) {
            activeDraft.large_asset = updated;
          } else {
            activeDraft.small_asset = updated;
          }
        },
      });
      valueWrap.appendChild(valueField);
    };

    kindSelect.addEventListener("change", () => {
      const kind = kindSelect.value as ArtworkRef["kind"];
      const nextAsset: ArtworkRef =
        kind === "portal_key"
          ? { kind: "portal_key", key: "" }
          : kind === "external_url"
            ? { kind: "external_url", url: "" }
            : { kind: "none" };
      if (isLarge) {
        activeDraft.large_asset = nextAsset;
      } else {
        activeDraft.small_asset = nextAsset;
      }
      renderAssetValueInput(nextAsset);
      refreshValidationAndBar();
      callbacks.onDraftUpdated();
    });

    renderAssetValueInput(currentAsset);

    return el("div", { className: "button-editor-item" }, [
      el("h3", {
        className: "card-title",
        text: isLarge ? "Large Image" : "Small Image",
      }),
      el("div", { className: "field-group" }, [
        el("label", {
          className: "field-label",
          attrs: { for: kindSelectId },
          text: "Source",
        }),
        kindSelect,
      ]),
      valueWrap,
      createTextField({
        key: textKey,
        label: "Hover Tooltip Text",
        value: (isLarge ? activeDraft.large_text : activeDraft.small_text) ?? "",
        maxLen: MAX_TEXT_LEN,
        placeholder: "Optional tooltip (2–128 chars)",
        onInput: (val) => {
          const next = val.length === 0 ? null : val;
          if (isLarge) activeDraft.large_text = next;
          else activeDraft.small_text = next;
        },
      }),
      createTextField({
        key: urlKey,
        label: "Clickable Image URL",
        value: (isLarge ? activeDraft.large_url : activeDraft.small_url) ?? "",
        maxLen: MAX_URL_LEN,
        experimental: true,
        mono: true,
        placeholder: "https://example.com (optional)",
        onInput: (val) => {
          const next = val.length === 0 ? null : val;
          if (isLarge) activeDraft.large_url = next;
          else activeDraft.small_url = next;
        },
      }),
    ]);
  }

  container.appendChild(
    el("section", { className: "card" }, [
      el("h2", { className: "section-title", text: "Artwork" }),
      buildArtworkSection("large"),
      buildArtworkSection("small"),
    ]),
  );

  // 5. Timestamps
  const tsModeSelect = el("select", {
    id: "editor-timestamp-mode",
    className: "select",
  }) as HTMLSelectElement;
  const tsModes: Array<{ value: TimestampMode; label: string }> = [
    { value: "none", label: "None (Recommended default)" },
    { value: "session_elapsed", label: "Session Elapsed (Count-up while published)" },
    { value: "countdown_until", label: "Countdown Until Target Date/Time" },
  ];
  for (const m of tsModes) {
    const opt = el("option", { text: m.label, attrs: { value: m.value } }) as HTMLOptionElement;
    if (m.value === draft.timestamp_mode) opt.selected = true;
    tsModeSelect.appendChild(opt);
  }
  tsModeSelect.disabled = !editable;

  const countdownWrap = el("div", { className: "field-group" });
  const countdownInput = el("input", {
    id: "editor-countdown-datetime",
    className: "input mono",
    attrs: {
      type: "datetime-local",
      "aria-describedby": "editor-err-custom_timestamp_unix",
    },
  }) as HTMLInputElement;
  countdownInput.value = toDatetimeLocalValue(draft.custom_timestamp_unix);
  countdownInput.disabled = !editable;

  const countdownErr = el("p", {
    id: "editor-err-custom_timestamp_unix",
    className: "field-error",
  });
  countdownErr.hidden = true;
  errorRefs.set("custom_timestamp_unix", { input: countdownInput, err: countdownErr });

  countdownInput.addEventListener("input", () => {
    draft.custom_timestamp_unix = fromDatetimeLocalValue(countdownInput.value);
    refreshValidationAndBar();
    callbacks.onDraftUpdated();
  });

  countdownWrap.appendChild(
    el("label", {
      className: "field-label",
      attrs: { for: "editor-countdown-datetime" },
      text: "Countdown Target (Local Time -> Stored as UTC Unix Seconds)",
    }),
  );
  countdownWrap.appendChild(countdownInput);
  countdownWrap.appendChild(countdownErr);
  countdownWrap.hidden = draft.timestamp_mode !== "countdown_until";

  tsModeSelect.addEventListener("change", () => {
    draft.timestamp_mode = tsModeSelect.value as TimestampMode;
    if (draft.timestamp_mode !== "countdown_until") {
      draft.custom_timestamp_unix = null;
      countdownInput.value = "";
      countdownWrap.hidden = true;
    } else {
      if (draft.custom_timestamp_unix === null) {
        const oneHourAhead = Math.floor(Date.now() / 1000) + 3600;
        draft.custom_timestamp_unix = oneHourAhead;
        countdownInput.value = toDatetimeLocalValue(oneHourAhead);
      }
      countdownWrap.hidden = false;
    }
    refreshValidationAndBar();
    callbacks.onDraftUpdated();
  });

  container.appendChild(
    el("section", { className: "card" }, [
      el("h2", { className: "section-title", text: "Timestamps" }),
      el("div", { className: "field-group" }, [
        el("label", {
          className: "field-label",
          attrs: { for: "editor-timestamp-mode" },
          text: "Timestamp Mode",
        }),
        tsModeSelect,
      ]),
      countdownWrap,
    ]),
  );

  // 6. Action Buttons (0..=2)
  const buttonsWrap = el("div", { className: "field-grid" });
  const addBtnButton = createButton("+ Add Button", "btn btn-secondary btn-sm", () => {
    if (draft.buttons.length >= MAX_BUTTONS_COUNT) return;
    draft.buttons.push({ label: "", url: "" });
    renderButtonsList();
    refreshValidationAndBar();
    callbacks.onDraftUpdated();
  });

  function renderButtonsList(): void {
    clearChildren(buttonsWrap);
    for (let i = 0; i < MAX_BUTTONS_COUNT; i++) {
      errorRefs.delete(`button_label_${i}`);
      errorRefs.delete(`button_url_${i}`);
      counterRefs.delete(`button_label_${i}`);
      counterRefs.delete(`button_url_${i}`);
    }

    if (activeDraft.buttons.length === 0) {
      buttonsWrap.appendChild(
        el("p", { className: "helper-text", text: "No action buttons configured (max 2)." }),
      );
    }

    activeDraft.buttons.forEach((btn, idx) => {
      const box = el("div", { className: "button-editor-item" }, [
        el("div", { className: "card-header" }, [
          el("span", { className: "field-label", text: `Button ${idx + 1}` }),
          createButton(
            "Remove",
            "btn btn-danger-outline btn-sm",
            () => {
              activeDraft.buttons.splice(idx, 1);
              renderButtonsList();
              refreshValidationAndBar();
              callbacks.onDraftUpdated();
            },
            { disabled: !editable },
          ),
        ]),
        createTextField({
          key: `button_label_${idx}`,
          label: "Button Label",
          value: btn.label,
          maxLen: MAX_BUTTON_LABEL_LEN,
          placeholder: "e.g., View Project",
          onInput: (val) => {
            btn.label = val;
          },
        }),
        createTextField({
          key: `button_url_${idx}`,
          label: "Button HTTPS URL",
          value: btn.url,
          maxLen: MAX_BUTTON_URL_LEN,
          mono: true,
          placeholder: "https://example.com",
          onInput: (val) => {
            btn.url = val;
          },
        }),
      ]);
      buttonsWrap.appendChild(box);
    });

    addBtnButton.disabled = !editable || activeDraft.buttons.length >= MAX_BUTTONS_COUNT;
  }

  renderButtonsList();

  container.appendChild(
    el("section", { className: "card" }, [
      el("div", { className: "card-header" }, [
        el("h2", { className: "section-title" }, [
          document.createTextNode("Action Buttons "),
          el("span", { className: "experimental-tag", text: "D-016" }),
        ]),
        addBtnButton,
      ]),
      buttonsWrap,
    ]),
  );

  // Sticky Save / Discard bar
  container.appendChild(
    el("div", { className: "sticky-editor-bar" }, [
      el("div", { className: "btn-row" }, [validationSummaryEl, dirtyBadgeEl]),
      el("div", { className: "btn-row" }, [discardBtn, saveBtn]),
    ]),
  );

  refreshValidationAndBar();
}

function countErrors(errors: ProfileValidationMap): number {
  let count = 0;
  for (const [k, v] of Object.entries(errors)) {
    if (k === "buttons") continue;
    if (typeof v === "string") count += 1;
  }
  for (const b of errors.buttons) {
    if (b.label) count += 1;
    if (b.url) count += 1;
  }
  return count;
}
