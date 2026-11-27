import { didiApi, formatCommandError } from "../api";
import { clearChildren, createButton, el, showToast } from "../dom";
import {
  appState,
  canMutateConfig,
  getActiveProfile,
  getEffectiveConfiguredAppId,
  isEditorDirty,
  syncEditorSelection,
} from "../state";
import { maskApplicationId } from "../validation";
import {
  activityVerb,
  confirmUnsavedEditorNavigation,
  getPresenceBadgeSpec,
  togglePresenceEnabled,
  type AppCallbacks,
} from "./shared";

export function renderDashboardView(container: HTMLElement, callbacks: AppCallbacks): void {
  clearChildren(container);

  if (appState.loading) {
    container.appendChild(
      el("section", { className: "card" }, [
        el("h2", { className: "section-title", text: "Connection & Presence" }),
        el("p", { className: "helper-text", text: "Loading configuration and Discord coordinator status..." }),
      ]),
    );
    return;
  }

  if (appState.loadError || !appState.config || !appState.presenceStatus) {
    container.appendChild(
      el("section", { className: "card" }, [
        el("h2", { className: "section-title", text: "Host Connection Error" }),
        el("p", {
          className: "field-error",
          text: appState.loadError ?? "Unable to load runtime state from the DiDi host.",
        }),
        el("div", { className: "btn-row" }, [
          createButton("Retry Loading", "btn btn-primary", () => {
            void callbacks.reloadAll();
          }),
        ]),
      ]),
    );
    return;
  }

  const config = appState.config;
  const status = appState.presenceStatus;
  const activeProfile = getActiveProfile();
  const effectiveAppId = getEffectiveConfiguredAppId(activeProfile);
  const badge = getPresenceBadgeSpec(status.state);
  const editable = canMutateConfig() && !appState.busy;

  // Missing Application ID callout
  if (!effectiveAppId) {
    container.appendChild(
      el("section", { className: "card" }, [
        el("div", { className: "card-header" }, [
          el("h2", { className: "card-title", text: "Setup Required: Discord Application ID" }),
          el("span", { className: "status-pill status-warning", text: "▲ App ID Missing" }),
        ]),
        el("p", {
          className: "helper-text",
          text: "DiDi uses Bring-Your-Own Application ID. Create or select an Application ID in the Discord Developer Portal and enter it in Settings before publishing Rich Presence.",
        }),
        el("div", { className: "btn-row" }, [
          createButton("Configure in Settings", "btn btn-primary", () => {
            void callbacks.navigateTo("settings");
          }),
        ]),
      ]),
    );
  }

  // Connection & Runtime Card
  const appIdSummary = (() => {
    if (status.masked_app_id) {
      return activeProfile?.application_id_override
        ? `${status.masked_app_id} (Profile override)`
        : `${status.masked_app_id} (Default)`;
    }
    if (effectiveAppId) {
      return activeProfile?.application_id_override
        ? `${maskApplicationId(effectiveAppId)} (Profile override)`
        : `${maskApplicationId(effectiveAppId)} (Default)`;
    }
    return "Not configured";
  })();

  const kvRows: HTMLElement[] = [
    el("div", { className: "kv-row" }, [
      el("dt", { text: "Presence Enabled" }),
      el("dd", { text: config.presence_enabled ? "Enabled" : "Paused" }),
    ]),
    el("div", { className: "kv-row" }, [
      el("dt", { text: "Discord State" }),
      el("dd", { text: status.status_label }),
    ]),
    el("div", { className: "kv-row" }, [
      el("dt", { text: "Application ID" }),
      el("dd", { className: "mono", text: appIdSummary }),
    ]),
    el("div", { className: "kv-row" }, [
      el("dt", { text: "Named Pipe" }),
      el("dd", {
        className: "mono",
        text:
          status.pipe_slot !== null
            ? `discord-ipc-${status.pipe_slot} (v${status.protocol_version ?? "?"})`
            : "Not attached",
      }),
    ]),
    el("div", { className: "kv-row" }, [
      el("dt", { text: "Application Identity" }),
      el("dd", {
        text: appState.scaffold
          ? `${appState.scaffold.product_name} — ${appState.scaffold.expanded_name} (${appState.scaffold.version})`
          : "DiDi — Discord Ditto",
      }),
    ]),
  ];

  const runtimeCard = el("section", { className: "card" }, [
    el("div", { className: "card-header" }, [
      el("h2", { className: "section-title", text: "Discord Runtime Status" }),
      el("span", {
        className: `status-pill ${badge.toneClass}`,
        text: `${badge.symbol} ${badge.label}`,
      }),
    ]),
    el("dl", { className: "kv-list" }, kvRows),
  ]);

  if (status.last_error) {
    runtimeCard.appendChild(
      el("p", {
        className:
          status.state === "disabled"
            ? "notice-banner notice-banner-warning"
            : "field-error",
        attrs: { role: status.state === "disabled" ? "status" : "alert" },
        text: status.last_error,
      }),
    );
  }

  if (status.state === "published") {
    runtimeCard.appendChild(
      el("p", {
        className: "helper-text",
        text: "Activity acknowledged by local Discord IPC. Profile visibility also depends on Discord's Activity Privacy settings.",
      }),
    );
  } else if (status.state === "connecting") {
    runtimeCard.appendChild(
      el("p", {
        className: "helper-text",
        text: "IPC pipe connected; awaiting SET_ACTIVITY acknowledgement before marking Published.",
      }),
    );
  }

  const runtimeActions = el("div", { className: "btn-row" }, [
    createButton(
      config.presence_enabled ? "Pause Presence" : "Resume Presence",
      config.presence_enabled ? "btn btn-secondary" : "btn btn-primary",
      () => {
        void togglePresenceEnabled(!config.presence_enabled, callbacks.render);
      },
      { disabled: !editable },
    ),
  ]);

  if (
    status.state === "waiting_for_discord" ||
    status.state === "reconnecting" ||
    status.state === "error"
  ) {
    runtimeActions.appendChild(
      createButton(
        "Retry Now",
        "btn btn-secondary",
        async () => {
          if (!canMutateConfig() || appState.busy) return;
          appState.busy = true;
          appState.bannerError = null;
          callbacks.render();
          try {
            await didiApi.retryPresence();
            appState.presenceStatus = await didiApi.getPresenceStatus();
          } catch (err) {
            appState.bannerError = formatCommandError(err, "Retry request failed.");
          } finally {
            appState.busy = false;
            callbacks.render();
          }
        },
        { disabled: !editable },
      ),
    );
  }

  runtimeCard.appendChild(runtimeActions);
  container.appendChild(runtimeCard);

  // Active Profile Card & Quick Switcher
  const profileSelect = el("select", {
    id: "dashboard-profile-select",
    className: "select",
    attrs: { "aria-label": "Active profile" },
  }) as HTMLSelectElement;

  for (const profile of config.profiles) {
    const opt = el("option", {
      text: profile.name,
      attrs: { value: profile.id },
    }) as HTMLOptionElement;
    if (profile.id === config.active_profile_id) {
      opt.selected = true;
    }
    profileSelect.appendChild(opt);
  }
  profileSelect.disabled = !editable;
  profileSelect.addEventListener("change", () => {
    void (async () => {
      const targetId = profileSelect.value;
      if (targetId === config.active_profile_id) return;
      if (isEditorDirty()) {
        const proceed = await confirmUnsavedEditorNavigation(callbacks);
        if (!proceed) {
          profileSelect.value = config.active_profile_id;
          return;
        }
      }
      appState.busy = true;
      appState.bannerError = null;
      callbacks.render();
      try {
        appState.config = await didiApi.setActiveProfile(targetId);
        syncEditorSelection(targetId);
        appState.presenceStatus = await didiApi.getPresenceStatus();
        showToast("Switched active profile.", "success");
      } catch (err) {
        appState.bannerError = formatCommandError(err, "Unable to switch active profile.");
      } finally {
        appState.busy = false;
        callbacks.render();
      }
    })();
  });

  const activeDetails =
    activeProfile?.details ?? "(No details line)";
  const activeState =
    activeProfile?.state ?? "(No state line)";

  const activityCard = el("section", { className: "card" }, [
    el("div", { className: "card-header" }, [
      el("h2", { className: "section-title", text: "Active Profile & Quick Switch" }),
      el("span", {
        className: "chip",
        text: activeProfile ? activityVerb(activeProfile.activity_type) : "None",
      }),
    ]),
    el("div", { className: "field-group" }, [
      el("label", {
        className: "field-label",
        attrs: { for: "dashboard-profile-select" },
        text: "Selected Profile",
      }),
      profileSelect,
    ]),
    el("dl", { className: "kv-list" }, [
      el("div", { className: "kv-row" }, [
        el("dt", { text: "Activity Name" }),
        el("dd", {
          text: activeProfile?.activity_name_override ?? "(Developer Portal App Name)",
        }),
      ]),
      el("div", { className: "kv-row" }, [
        el("dt", { text: "Details" }),
        el("dd", { text: activeDetails }),
      ]),
      el("div", { className: "kv-row" }, [
        el("dt", { text: "State" }),
        el("dd", { text: activeState }),
      ]),
    ]),
    el("div", { className: "btn-row" }, [
      createButton(
        "Edit Active Profile",
        "btn btn-primary",
        () => {
          syncEditorSelection(config.active_profile_id);
          void callbacks.navigateTo("editor");
        },
        { disabled: !activeProfile },
      ),
      createButton("View Preview", "btn btn-secondary", () => {
        void callbacks.navigateTo("preview");
      }),
      createButton("Manage Profiles", "btn btn-secondary", () => {
        void callbacks.navigateTo("profiles");
      }),
    ]),
  ]);
  container.appendChild(activityCard);

  // Quick Preferences Summary
  container.appendChild(
    el("section", { className: "card" }, [
      el("div", { className: "card-header" }, [
        el("h2", { className: "section-title", text: "Startup & Tray Summary" }),
        createButton("Settings", "btn btn-secondary btn-sm", () => {
          void callbacks.navigateTo("settings");
        }),
      ]),
      el("div", { className: "btn-row" }, [
        el("span", {
          className: `chip ${config.launch_at_login ? "chip-accent" : ""}`,
          text: `Windows Startup: ${config.launch_at_login ? "On" : "Off"}`,
        }),
        el("span", {
          className: `chip ${config.close_to_tray ? "chip-accent" : ""}`,
          text: `Close to Tray: ${config.close_to_tray ? "On" : "Off"}`,
        }),
        el("span", {
          className: "chip",
          text: `Layout: ${config.ui_layout === "expanded" ? "Expanded" : "Compact"}`,
        }),
      ]),
    ]),
  );
}
