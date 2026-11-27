import { didiApi, formatCommandError } from "../api";
import { clearChildren, createButton, el, openModalDialog, showToast } from "../dom";
import {
  appState,
  canMutateConfig,
  cloneProfile,
  isEditorDirty,
  syncEditorSelection,
} from "../state";
import type { ProfileV1 } from "../types";
import {
  generateUniqueProfileSlug,
  MAX_PROFILE_NAME_LEN,
  MAX_PROFILES_COUNT,
  maskApplicationId,
  validateProfileId,
  validateProfileName,
} from "../validation";
import {
  activityVerb,
  confirmUnsavedEditorNavigation,
  runExportFlow,
  runImportFlow,
  type AppCallbacks,
} from "./shared";

async function openCreateProfileModal(callbacks: AppCallbacks): Promise<void> {
  if (!appState.config || !canMutateConfig()) return;
  const existingIds = appState.config.profiles.map((p) => p.id);

  let draftName = "";
  let draftId = "";
  let idManuallyEdited = false;

  const nameInput = el("input", {
    id: "new-profile-name",
    className: "input",
    attrs: {
      type: "text",
      maxlength: "64",
      placeholder: "e.g., Late Night Coding",
    },
  }) as HTMLInputElement;

  const idInput = el("input", {
    id: "new-profile-id",
    className: "input mono",
    attrs: {
      type: "text",
      maxlength: "64",
      placeholder: "late-night-coding",
    },
  }) as HTMLInputElement;

  const errorText = el("p", { className: "field-error" });
  errorText.hidden = true;

  nameInput.addEventListener("input", () => {
    draftName = nameInput.value;
    if (!idManuallyEdited) {
      draftId = generateUniqueProfileSlug(draftName, existingIds);
      idInput.value = draftId;
    }
    errorText.hidden = true;
  });

  idInput.addEventListener("input", () => {
    idManuallyEdited = true;
    draftId = idInput.value;
    errorText.hidden = true;
  });

  while (true) {
    const choice = await openModalDialog({
      title: "Create New Profile",
      body: [
        el("div", { className: "field-group" }, [
          el("label", {
            className: "field-label",
            attrs: { for: "new-profile-name" },
            text: "Profile Name",
          }),
          nameInput,
        ]),
        el("div", { className: "field-group" }, [
          el("label", {
            className: "field-label",
            attrs: { for: "new-profile-id" },
            text: "Profile Slug ID",
          }),
          idInput,
          el("p", {
            className: "helper-text",
            text: "Lowercase letters, numbers, '-' or '_', 1–64 chars.",
          }),
        ]),
        errorText,
      ],
      actions: [
        { label: "Create Profile", className: "btn btn-primary", value: "create" },
        { label: "Cancel", className: "btn btn-secondary", value: "cancel" },
      ],
    });

    if (choice !== "create") {
      return;
    }

    const nameErr = validateProfileName(draftName);
    const idErr = validateProfileId(draftId);
    const trimmedId = draftId.trim();
    const duplicate = existingIds.includes(trimmedId)
      ? `Profile ID "${trimmedId}" already exists.`
      : null;

    const firstError = nameErr ?? idErr ?? duplicate;
    if (firstError) {
      errorText.textContent = firstError;
      errorText.hidden = false;
      continue;
    }

    const newProfile: ProfileV1 = {
      id: trimmedId,
      name: draftName.trim(),
      application_id_override: null,
      activity_name_override: null,
      activity_type: "playing",
      status_display_type: "name",
      details: null,
      details_url: null,
      state: null,
      state_url: null,
      large_asset: { kind: "none" },
      large_text: null,
      large_url: null,
      small_asset: { kind: "none" },
      small_text: null,
      small_url: null,
      timestamp_mode: "none",
      custom_timestamp_unix: null,
      buttons: [],
    };

    appState.busy = true;
    appState.bannerError = null;
    callbacks.render();
    try {
      appState.config = await didiApi.saveProfile(newProfile);
      syncEditorSelection(newProfile.id);
      showToast(`Created profile "${newProfile.name}".`, "success");
      appState.busy = false;
      await callbacks.navigateTo("editor");
      return;
    } catch (err) {
      appState.bannerError = formatCommandError(err, "Unable to create profile.");
      appState.busy = false;
      callbacks.render();
      return;
    }
  }
}

export function renderProfilesView(container: HTMLElement, callbacks: AppCallbacks): void {
  clearChildren(container);

  if (appState.loading || !appState.config) {
    container.appendChild(
      el("section", { className: "card" }, [
        el("p", { className: "helper-text", text: "Loading saved profiles..." }),
      ]),
    );
    return;
  }

  const config = appState.config;
  const editable = canMutateConfig() && !appState.busy;
  const atMaxProfiles = config.profiles.length >= MAX_PROFILES_COUNT;

  const toolbarCard = el("section", { className: "card" }, [
    el("div", { className: "card-header" }, [
      el("h2", { className: "section-title", text: "Saved Profiles" }),
      el("span", {
        className: "chip mono",
        text: `${config.profiles.length} / ${MAX_PROFILES_COUNT}`,
      }),
    ]),
    el("div", { className: "btn-row" }, [
      createButton(
        "+ New Profile",
        "btn btn-primary",
        () => {
          void (async () => {
            if (isEditorDirty()) {
              const proceed = await confirmUnsavedEditorNavigation(callbacks);
              if (!proceed) return;
            }
            await openCreateProfileModal(callbacks);
          })();
        },
        {
          disabled: !editable || atMaxProfiles,
          title: atMaxProfiles ? "Maximum of 50 profiles reached." : undefined,
        },
      ),
      createButton(
        "Import (.json)",
        "btn btn-secondary",
        () => {
          void runImportFlow(callbacks);
        },
        { disabled: !editable },
      ),
      createButton(
        "Export All (.json)",
        "btn btn-secondary",
        () => {
          void runExportFlow(null, callbacks);
        },
        { disabled: appState.busy },
      ),
    ]),
  ]);
  container.appendChild(toolbarCard);

  const listWrap = el("div", { className: "profile-list" });
  const existingIds = config.profiles.map((p) => p.id);

  for (const profile of config.profiles) {
    const isActive = profile.id === config.active_profile_id;
    const isOnlyProfile = config.profiles.length <= 1;

    const metaChips = el("div", { className: "profile-item-meta" }, [
      el("span", { className: "mono", text: profile.id }),
      el("span", { className: "chip", text: activityVerb(profile.activity_type) }),
    ]);

    if (profile.application_id_override) {
      metaChips.appendChild(
        el("span", {
          className: "chip",
          text: `Override: ${maskApplicationId(profile.application_id_override)}`,
        }),
      );
    }

    const summaryText = [profile.details, profile.state]
      .filter((part): part is string => Boolean(part))
      .join(" — ");

    const item = el(
      "article",
      { className: `profile-item ${isActive ? "is-active" : ""}` },
      [
        el("div", { className: "profile-item-top" }, [
          el("div", {}, [
            el("h3", { className: "profile-item-title", text: profile.name }),
            metaChips,
          ]),
          isActive
            ? el("span", { className: "status-pill status-success", text: "● Active" })
            : createButton(
                "Activate",
                "btn btn-primary btn-sm",
                async () => {
                  if (!canMutateConfig() || appState.busy) return;
                  appState.busy = true;
                  appState.bannerError = null;
                  callbacks.render();
                  try {
                    appState.config = await didiApi.setActiveProfile(profile.id);
                    appState.presenceStatus = await didiApi.getPresenceStatus();
                    showToast(`Activated "${profile.name}".`, "success");
                  } catch (err) {
                    appState.bannerError = formatCommandError(err, "Failed to activate profile.");
                  } finally {
                    appState.busy = false;
                    callbacks.render();
                  }
                },
                { disabled: !editable },
              ),
        ]),
        el("p", {
          className: "helper-text",
          text: summaryText.length > 0 ? summaryText : "No details or state text configured.",
        }),
        el("div", { className: "btn-row" }, [
          createButton("Edit", "btn btn-secondary btn-sm", () => {
            void (async () => {
              if (appState.editorProfileId !== profile.id && isEditorDirty()) {
                const proceed = await confirmUnsavedEditorNavigation(callbacks);
                if (!proceed) return;
              }
              syncEditorSelection(profile.id);
              await callbacks.navigateTo("editor");
            })();
          }),
          createButton(
            "Duplicate",
            "btn btn-secondary btn-sm",
            async () => {
              if (!canMutateConfig() || appState.busy || atMaxProfiles) return;
              const copy = cloneProfile(profile);
              copy.id = generateUniqueProfileSlug(`${profile.id}-copy`, existingIds);
              const copySuffix = " (Copy)";
              const baseChars = Array.from(profile.name)
                .slice(0, MAX_PROFILE_NAME_LEN - copySuffix.length)
                .join("");
              copy.name = `${baseChars}${copySuffix}`;
              appState.busy = true;
              appState.bannerError = null;
              callbacks.render();
              try {
                appState.config = await didiApi.saveProfile(copy);
                syncEditorSelection(copy.id);
                showToast(`Duplicated "${profile.name}".`, "success");
              } catch (err) {
                appState.bannerError = formatCommandError(err, "Failed to duplicate profile.");
              } finally {
                appState.busy = false;
                callbacks.render();
              }
            },
            {
              disabled: !editable || atMaxProfiles,
              title: atMaxProfiles ? "Maximum of 50 profiles reached." : undefined,
            },
          ),
          createButton(
            "Export",
            "btn btn-secondary btn-sm",
            () => {
              void runExportFlow([profile.id], callbacks);
            },
            { disabled: appState.busy },
          ),
          createButton(
            "Delete",
            "btn btn-danger-outline btn-sm",
            async () => {
              if (!canMutateConfig() || appState.busy || isOnlyProfile) return;
              const choice = await openModalDialog({
                title: "Delete Profile?",
                body: [
                  el("p", {
                    text: `Delete profile "${profile.name}" (${profile.id})? This action cannot be undone.`,
                  }),
                ],
                actions: [
                  { label: "Delete Profile", className: "btn btn-danger", value: "delete" },
                  { label: "Cancel", className: "btn btn-secondary", value: "cancel" },
                ],
              });
              if (choice !== "delete") return;

              appState.busy = true;
              appState.bannerError = null;
              callbacks.render();
              try {
                appState.config = await didiApi.deleteProfile(profile.id);
                syncEditorSelection();
                appState.presenceStatus = await didiApi.getPresenceStatus();
                showToast(`Deleted "${profile.name}".`, "neutral");
              } catch (err) {
                appState.bannerError = formatCommandError(err, "Failed to delete profile.");
              } finally {
                appState.busy = false;
                callbacks.render();
              }
            },
            {
              disabled: !editable || isOnlyProfile,
              title: isOnlyProfile ? "Cannot delete the last remaining profile." : undefined,
            },
          ),
        ]),
      ],
    );

    listWrap.appendChild(item);
  }

  container.appendChild(listWrap);
}
