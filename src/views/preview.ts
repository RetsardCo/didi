import { clearChildren, el } from "../dom";
import { appState, getActiveProfile } from "../state";
import type { ArtworkRef, PresenceActivity } from "../types";
import { extractHostnameSafe, formatDurationClock } from "../validation";
import { activityVerb, type AppCallbacks } from "./shared";

function describeArtworkPlaceholder(asset: ArtworkRef): { badge: string; title: string } {
  if (asset.kind === "none") {
    return { badge: "No Art", title: "No artwork configured" };
  }
  if (asset.kind === "portal_key") {
    const key = asset.key.trim().toLowerCase() || "(empty key)";
    return { badge: `Key: ${key}`, title: `Portal Asset Key: ${key}` };
  }
  const url = asset.url.trim();
  const host = url.toLowerCase().startsWith("https://")
    ? extractHostnameSafe(url)
    : "(invalid url)";
  return { badge: `HTTPS: ${host}`, title: `External HTTPS Image (${host})` };
}

function profileToPreviewSnapshot(): PresenceActivity | null {
  const source = appState.editorDraft ?? getActiveProfile();
  if (!source) return null;
  return {
    activity_type: source.activity_type,
    status_display_type: source.status_display_type,
    name_override: source.activity_name_override,
    details: source.details,
    details_url: source.details_url,
    state: source.state,
    state_url: source.state_url,
    large_asset: source.large_asset,
    large_text: source.large_text,
    large_url: source.large_url,
    small_asset: source.small_asset,
    small_text: source.small_text,
    small_url: source.small_url,
    timestamp_mode: source.timestamp_mode,
    start_timestamp: null,
    end_timestamp:
      source.timestamp_mode === "countdown_until" ? source.custom_timestamp_unix : null,
    buttons: source.buttons,
  };
}

function formatTimestampPreview(activity: PresenceActivity, isLiveSnapshot: boolean): string | null {
  const nowSecs = Math.floor(Date.now() / 1000);
  if (activity.timestamp_mode === "none") {
    return null;
  }
  if (activity.timestamp_mode === "session_elapsed") {
    if (isLiveSnapshot && activity.start_timestamp !== null) {
      return `${formatDurationClock(nowSecs - activity.start_timestamp)} elapsed`;
    }
    return "00:00 elapsed (starts upon publish)";
  }
  if (activity.timestamp_mode === "countdown_until") {
    if (activity.end_timestamp === null) {
      return "Countdown (target date/time not set)";
    }
    const diff = activity.end_timestamp - nowSecs;
    if (diff <= 0) {
      return "Countdown expired (omitted automatically on publish)";
    }
    return `${formatDurationClock(diff)} left`;
  }
  return null;
}

export function renderPreviewView(
  container: HTMLElement,
  callbacks: AppCallbacks,
  options?: { docked?: boolean },
): void {
  clearChildren(container);

  if (appState.loading || !appState.config) {
    container.appendChild(
      el("section", { className: "card" }, [
        el("p", { className: "helper-text", text: "Loading preview..." }),
      ]),
    );
    return;
  }

  const isDocked = options?.docked === true;
  const activeMode = isDocked ? "draft" : appState.previewMode;
  const status = appState.presenceStatus;

  const isLivePublished =
    activeMode === "live" &&
    status?.state === "published" &&
    status.active_activity !== null;

  const activity =
    activeMode === "live"
      ? status?.state === "published"
        ? status.active_activity
        : null
      : profileToPreviewSnapshot();

  const headerControls: HTMLElement[] = [
    el("h2", {
      className: "section-title",
      text: isDocked ? "Draft Preview" : "Rich Presence Preview",
    }),
  ];

  if (!isDocked) {
    const modeSwitch = el("div", {
      className: "segmented",
      attrs: { role: "group", "aria-label": "Preview source mode" },
    });
    const draftBtn = el("button", {
      className: `segmented-btn ${activeMode === "draft" ? "is-active" : ""}`,
      text: "Draft Preview",
      attrs: { type: "button" },
    }) as HTMLButtonElement;
    draftBtn.addEventListener("click", () => {
      appState.previewMode = "draft";
      callbacks.render();
    });

    const liveBtn = el("button", {
      className: `segmented-btn ${activeMode === "live" ? "is-active" : ""}`,
      text: "Live Published",
      attrs: { type: "button" },
    }) as HTMLButtonElement;
    liveBtn.addEventListener("click", () => {
      appState.previewMode = "live";
      callbacks.render();
    });

    modeSwitch.appendChild(draftBtn);
    modeSwitch.appendChild(liveBtn);
    headerControls.push(modeSwitch);
  }

  const wrapperCard = el("section", { className: "card" }, [
    el("div", { className: "card-header" }, headerControls),
    el("p", {
      className: "preview-disclaimer",
      text: "Approximate Local Preview — Actual Discord rendering may vary. External artwork URLs are never fetched by DiDi UI.",
    }),
  ]);

  if (activeMode === "live" && !isLivePublished) {
    wrapperCard.appendChild(
      el("div", { className: "discord-card" }, [
        el("p", {
          className: "helper-text",
          text:
            status?.state === "published"
              ? "No published activity snapshot available."
              : `No live Discord Rich Presence published right now (current state: ${status?.status_label ?? "Unknown"}).`,
        }),
      ]),
    );
    container.appendChild(wrapperCard);
    return;
  }

  if (!activity) {
    wrapperCard.appendChild(
      el("p", { className: "helper-text", text: "Select or create a profile to preview." }),
    );
    container.appendChild(wrapperCard);
    return;
  }

  const largeInfo = describeArtworkPlaceholder(activity.large_asset);
  const smallInfo = describeArtworkPlaceholder(activity.small_asset);

  const artBox = el("div", {
    className: "discord-art-box",
    attrs: {
      title: activity.large_text
        ? `${largeInfo.title} — "${activity.large_text}"`
        : largeInfo.title,
    },
  }, [el("span", { text: largeInfo.badge })]);

  if (activity.small_asset.kind !== "none") {
    artBox.appendChild(
      el("span", {
        className: "discord-art-small",
        text: activity.small_asset.kind === "portal_key" ? "KEY" : "URL",
        attrs: {
          title: activity.small_text
            ? `${smallInfo.title} — "${activity.small_text}"`
            : smallInfo.title,
        },
      }),
    );
  }

  const resolvedTitle =
    activity.name_override?.trim() || "(Developer Portal App Name)";
  const textStack = el("div", { className: "discord-text-stack" }, [
    el("p", { className: "discord-title", text: resolvedTitle }),
  ]);

  if (activity.details) {
    const detailSuffix = activity.details_url
      ? ` [${extractHostnameSafe(activity.details_url)}]`
      : "";
    textStack.appendChild(
      el("p", { className: "discord-line", text: `${activity.details}${detailSuffix}` }),
    );
  } else {
    textStack.appendChild(
      el("p", { className: "discord-line-muted", text: "(No details text)" }),
    );
  }

  if (activity.state) {
    const stateSuffix = activity.state_url
      ? ` [${extractHostnameSafe(activity.state_url)}]`
      : "";
    textStack.appendChild(
      el("p", { className: "discord-line", text: `${activity.state}${stateSuffix}` }),
    );
  }

  const timestampLine = formatTimestampPreview(activity, isLivePublished);
  if (timestampLine) {
    textStack.appendChild(
      el("p", { className: "discord-line-muted mono", text: timestampLine }),
    );
  }

  if (activity.large_text || activity.small_text) {
    const tooltips = [
      activity.large_text ? `Large tip: "${activity.large_text}"` : null,
      activity.small_text ? `Small tip: "${activity.small_text}"` : null,
    ]
      .filter((t): t is string => Boolean(t))
      .join(" • ");
    textStack.appendChild(el("p", { className: "helper-text", text: tooltips }));
  }

  const cardBody = el("div", { className: "discord-card" }, [
    el("div", { className: "discord-card-header" }, [
      el("span", { text: `${activityVerb(activity.activity_type)}` }),
      el("span", {
        className: `chip ${isLivePublished ? "chip-accent" : ""}`,
        text: isLivePublished ? "Live Snapshot" : "Local Draft",
      }),
    ]),
    el("div", { className: "discord-card-body" }, [artBox, textStack]),
  ]);

  if (activity.buttons.length > 0) {
    const buttonsRow = el("div", { className: "discord-buttons" });
    for (const btn of activity.buttons) {
      const label = btn.label.trim() || "(Untitled Button)";
      const domainHint = btn.url.toLowerCase().startsWith("https://")
        ? ` (${extractHostnameSafe(btn.url)})`
        : "";
      const previewBtn = el("button", {
        className: "discord-preview-btn",
        text: `${label}${domainHint}`,
        attrs: { type: "button", disabled: "true", title: "Preview buttons do not navigate" },
      }) as HTMLButtonElement;
      buttonsRow.appendChild(previewBtn);
    }
    cardBody.appendChild(buttonsRow);
  }

  wrapperCard.appendChild(cardBody);
  container.appendChild(wrapperCard);
}
