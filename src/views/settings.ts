import { didiApi, formatCommandError } from "../api";
import { clearChildren, createButton, el, openModalDialog, showToast } from "../dom";
import { appState, canMutateConfig } from "../state";
import type { PreferencesPatch, UiLayout } from "../types";
import { maskApplicationId, validateApplicationId } from "../validation";
import {
  applyLayoutClass,
  runExportFlow,
  runImportFlow,
  type AppCallbacks,
} from "./shared";

export function renderSettingsView(container: HTMLElement, callbacks: AppCallbacks): void {
  clearChildren(container);

  if (appState.loading || !appState.config) {
    container.appendChild(
      el("section", { className: "card" }, [
        el("p", { className: "helper-text", text: "Loading application settings..." }),
      ]),
    );
    return;
  }

  const config = appState.config;
  const editable = canMutateConfig() && !appState.busy;

  // 1. Default Application ID (BYO-AppID)
  const appIdInput = el("input", {
    id: "settings-default-appid",
    className: "input mono",
    attrs: {
      type: appState.revealDefaultAppId ? "text" : "password",
      placeholder: "Enter 17–20 digit Discord Application ID",
      autocomplete: "off",
      "aria-describedby": "settings-appid-error",
    },
  }) as HTMLInputElement;
  appIdInput.value = config.default_application_id ?? "";
  appIdInput.disabled = !editable;

  const appIdErr = el("p", {
    id: "settings-appid-error",
    className: "field-error",
  });
  appIdErr.hidden = true;

  const saveAppIdBtn = createButton(
    "Save Application ID",
    "btn btn-primary",
    async () => {
      if (!canMutateConfig() || appState.busy) return;
      const raw = appIdInput.value.trim();
      if (raw.length > 0) {
        const validationErr = validateApplicationId(appIdInput.value);
        if (validationErr) {
          appIdInput.setAttribute("aria-invalid", "true");
          appIdErr.textContent = validationErr;
          appIdErr.hidden = false;
          return;
        }
      }
      appIdInput.removeAttribute("aria-invalid");
      appIdErr.hidden = true;

      appState.busy = true;
      appState.bannerError = null;
      callbacks.render();
      try {
        appState.config = await didiApi.setDefaultApplicationId(raw.length > 0 ? raw : null);
        appState.presenceStatus = await didiApi.getPresenceStatus();
        showToast(
          raw.length > 0
            ? `Saved Application ID (${maskApplicationId(raw)}).`
            : "Cleared default Application ID.",
          "success",
        );
      } catch (err) {
        appState.bannerError = formatCommandError(err, "Unable to update Application ID.");
      } finally {
        appState.busy = false;
        callbacks.render();
      }
    },
    { disabled: !editable },
  );

  const revealBtn = createButton(
    appState.revealDefaultAppId ? "Hide" : "Reveal",
    "btn btn-secondary btn-sm",
    () => {
      appState.revealDefaultAppId = !appState.revealDefaultAppId;
      appIdInput.type = appState.revealDefaultAppId ? "text" : "password";
      revealBtn.textContent = appState.revealDefaultAppId ? "Hide" : "Reveal";
    },
  );

  container.appendChild(
    el("section", { className: "card" }, [
      el("div", { className: "card-header" }, [
        el("h2", { className: "section-title", text: "Discord Application ID (BYO-AppID)" }),
        el("span", {
          className: `status-pill ${
            config.default_application_id ? "status-success" : "status-warning"
          }`,
          text: config.default_application_id
            ? `Configured: ${maskApplicationId(config.default_application_id)}`
            : "Not Configured",
        }),
      ]),
      el("div", { className: "field-group" }, [
        el("label", {
          className: "field-label",
          attrs: { for: "settings-default-appid" },
          text: "Default Application ID",
        }),
        el("div", { className: "input-with-action" }, [appIdInput, revealBtn]),
        el("p", {
          className: "helper-text",
          text: "Stored locally in %APPDATA%\\DiDi\\config.json and excluded from profile exports and logs.",
        }),
        appIdErr,
      ]),
      el("div", { className: "btn-row" }, [
        saveAppIdBtn,
        createButton(
          "Clear",
          "btn btn-secondary",
          async () => {
            if (!canMutateConfig() || appState.busy || !config.default_application_id) return;
            appState.busy = true;
            appState.bannerError = null;
            callbacks.render();
            try {
              appState.config = await didiApi.setDefaultApplicationId(null);
              appState.presenceStatus = await didiApi.getPresenceStatus();
              showToast("Cleared default Application ID.", "neutral");
            } catch (err) {
              appState.bannerError = formatCommandError(err, "Unable to clear Application ID.");
            } finally {
              appState.busy = false;
              callbacks.render();
            }
          },
          { disabled: !editable || !config.default_application_id },
        ),
        createButton("Discord Developer Portal", "btn btn-secondary", () => {
          void didiApi.openExternalDocsUrl("developer_portal").catch((err) => {
            appState.bannerError = formatCommandError(err, "Unable to open browser.");
            callbacks.render();
          });
        }),
      ]),
    ]),
  );

  // 2. Windows Behavior & Layout
  async function patchPreference(patch: PreferencesPatch, label: string): Promise<void> {
    if (!canMutateConfig() || appState.busy) return;
    appState.busy = true;
    appState.bannerError = null;
    callbacks.render();
    try {
      const updated = await didiApi.updatePreferences(patch);
      appState.config = updated;
      applyLayoutClass(updated.ui_layout);
      showToast(`${label} updated.`, "success");
    } catch (err) {
      appState.bannerError = formatCommandError(err, "Failed to update preference.");
    } finally {
      appState.busy = false;
      callbacks.render();
    }
  }

  function makePreferenceToggle(opts: {
    id: string;
    title: string;
    description: string;
    checked: boolean;
    onChange: (next: boolean) => void;
  }): HTMLElement {
    const checkbox = el("input", {
      id: opts.id,
      className: "checkbox-input",
      attrs: { type: "checkbox" },
    }) as HTMLInputElement;
    checkbox.checked = opts.checked;
    checkbox.disabled = !editable;
    checkbox.addEventListener("change", () => {
      opts.onChange(checkbox.checked);
    });

    return el("div", { className: "toggle-row" }, [
      el("div", { className: "toggle-meta" }, [
        el("label", { className: "toggle-title", attrs: { for: opts.id }, text: opts.title }),
        el("span", { className: "helper-text", text: opts.description }),
      ]),
      checkbox,
    ]);
  }

  const layoutSelect = el("select", {
    id: "settings-ui-layout",
    className: "select",
  }) as HTMLSelectElement;
  const layouts: Array<{ value: UiLayout; label: string }> = [
    { value: "compact", label: "Compact (480 × 600 default)" },
    { value: "expanded", label: "Expanded (760 × 620 split preview)" },
  ];
  for (const item of layouts) {
    const opt = el("option", { text: item.label, attrs: { value: item.value } }) as HTMLOptionElement;
    if (item.value === config.ui_layout) opt.selected = true;
    layoutSelect.appendChild(opt);
  }
  layoutSelect.disabled = !editable;
  layoutSelect.addEventListener("change", () => {
    const next = layoutSelect.value as UiLayout;
    void patchPreference({ ui_layout: next }, "Window layout");
  });

  container.appendChild(
    el("section", { className: "card" }, [
      el("h2", { className: "section-title", text: "Windows Integration & Layout" }),
      makePreferenceToggle({
        id: "pref-launch-login",
        title: "Launch at Windows Startup",
        description: "Registers per-user HKCU Run entry with --minimized (no elevated service).",
        checked: config.launch_at_login,
        onChange: (next) => {
          void patchPreference({ launch_at_login: next }, "Windows Startup setting");
        },
      }),
      makePreferenceToggle({
        id: "pref-start-minimized",
        title: "Start Minimized to System Tray",
        description: "Keep the settings window closed on startup until opened from the tray.",
        checked: config.start_minimized,
        onChange: (next) => {
          void patchPreference({ start_minimized: next }, "Start minimized setting");
        },
      }),
      makePreferenceToggle({
        id: "pref-close-to-tray",
        title: "Close Window to System Tray",
        description: "Destroys the editor UI on close to save RAM while keeping Discord RPC active.",
        checked: config.close_to_tray,
        onChange: (next) => {
          void patchPreference({ close_to_tray: next }, "Close-to-tray setting");
        },
      }),
      el("div", { className: "field-group" }, [
        el("label", {
          className: "field-label",
          attrs: { for: "settings-ui-layout" },
          text: "Window Layout",
        }),
        layoutSelect,
      ]),
    ]),
  );

  // 3. Data Portability, Diagnostics & Links
  container.appendChild(
    el("section", { className: "card" }, [
      el("h2", { className: "section-title", text: "Diagnostics & Data" }),
      el("div", { className: "btn-row" }, [
        createButton("Open Logs Folder", "btn btn-secondary", () => {
          void didiApi.openLogsFolder().catch((err) => {
            appState.bannerError = formatCommandError(err, "Unable to open logs folder.");
            callbacks.render();
          });
        }),
        createButton(
          "Import Profiles (.json)",
          "btn btn-secondary",
          () => {
            void runImportFlow(callbacks);
          },
          { disabled: !editable },
        ),
        createButton(
          "Export All Profiles (.json)",
          "btn btn-secondary",
          () => {
            void runExportFlow(null, callbacks);
          },
          { disabled: appState.busy },
        ),
      ]),
      el("div", { className: "btn-row" }, [
        createButton("Project Documentation", "btn btn-secondary btn-sm", () => {
          void didiApi.openExternalDocsUrl("project_repository").catch((err) => {
            appState.bannerError = formatCommandError(err, "Unable to open documentation link.");
            callbacks.render();
          });
        }),
        createButton("Report an Issue", "btn btn-secondary btn-sm", () => {
          void didiApi.openExternalDocsUrl("issue_tracker").catch((err) => {
            appState.bannerError = formatCommandError(err, "Unable to open issue tracker link.");
            callbacks.render();
          });
        }),
      ]),
    ]),
  );

  // 4. About, Compliance & Application Exit
  const versionTag = appState.scaffold ? `v${appState.scaffold.version}` : "v0.1.0";
  const aboutLogo = el("img", {
    className: "about-logo",
    attrs: {
      src: "/didi-logo.png",
      alt: "DiDi mascot logo",
      width: "36",
      height: "36",
    },
  });

  container.appendChild(
    el("section", { className: "card" }, [
      el("h2", { className: "section-title", text: "About, Compliance & Lifecycle" }),
      el("div", { className: "about-brand" }, [
        aboutLogo,
        el("div", {}, [
          el("div", {
            className: "card-title",
            text: `DiDi — Discord Ditto (${versionTag})`,
          }),
          el("p", {
            className: "helper-text",
            text: "Your Activity. Your Identity.",
          }),
        ]),
      ]),
      el("p", {
        className: "helper-text",
        text: "DiDi stores settings locally under %APPDATA%\\DiDi\\config.json, uses zero cloud sync or telemetry, discards Discord user data during IPC handshake, and never scans running game processes or collects tokens.",
      }),
      el("div", { className: "btn-row" }, [
        createButton(
          "Exit DiDi Completely",
          "btn btn-danger-outline",
          async () => {
            const choice = await openModalDialog({
              title: "Exit DiDi Completely?",
              body: [
                el("p", {
                  text: "This will clear active Rich Presence on Discord, close the system tray runtime, and terminate DiDi.",
                }),
              ],
              actions: [
                { label: "Exit DiDi", className: "btn btn-danger", value: "exit" },
                { label: "Cancel", className: "btn btn-secondary", value: "cancel" },
              ],
            });
            if (choice !== "exit") return;
            try {
              await didiApi.exitApp();
            } catch (err) {
              appState.bannerError = formatCommandError(err, "Unable to trigger clean exit.");
              callbacks.render();
            }
          },
          { disabled: appState.busy },
        ),
      ]),
    ]),
  );
}
