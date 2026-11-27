# DiDi (Discord Ditto)

Local-first Windows desktop utility for configuring and publishing custom Discord Rich Presence activity without Discord user tokens, bot tokens, cloud backends, or game process scanning.

---

## Table of Contents

1. [Project Introduction](#1-project-introduction)
2. [Features](#2-features)
3. [System Requirements](#3-system-requirements)
4. [Download and Installation](#4-download-and-installation)
5. [Discord Developer Portal Setup](#5-discord-developer-portal-setup)
6. [First-Time Setup](#6-first-time-setup)
7. [Dashboard](#7-dashboard)
8. [Profiles](#8-profiles)
9. [Presence Editor](#9-presence-editor)
10. [Preview](#10-preview)
11. [Settings](#11-settings)
12. [System Tray](#12-system-tray)
13. [Pause, Resume, and Clear Activity](#13-pause-resume-and-clear-activity)
14. [Configuration and Data Storage](#14-configuration-and-data-storage)
15. [Troubleshooting](#15-troubleshooting)
16. [Privacy and Security](#16-privacy-and-security)
17. [Uninstallation](#17-uninstallation)
18. [Frequently Asked Questions](#18-frequently-asked-questions)
19. [License](#19-license)
20. [Credits and Building from Source](#20-credits-and-building-from-source)

---

## 1. Project Introduction

DiDi (Discord Ditto, version `0.1.0`) is a lightweight Windows desktop application built with Rust, Tauri 2, and TypeScript. It lets you configure what Discord displays on your profile while you are online—including the activity type, headline, details line, state line, custom artwork, timestamps, and clickable buttons.

### What Discord Rich Presence Does
Discord Rich Presence lets an application display structured activity information on your Discord user profile card and active status line. Unlike basic game detection (which only shows a program name), Rich Presence supports two text lines, tooltips, timers, large and small artwork, and interactive buttons.

### Why DiDi Exists
Many third-party status tools ask for your Discord user token, connect unauthorized self-bots to Discord's Gateway WebSocket, or scan running processes on your computer. DiDi avoids all three:

- **Bring Your Own Application ID (BYO-AppID):** You create a standard application in the official Discord Developer Portal and provide its public 17–20 digit Application ID.
- **Official Local IPC Only:** DiDi connects directly to the Discord desktop client on the same Windows session through Discord's documented named-pipe IPC interface (`\\.\pipe\discord-ipc-{0..9}`).
- **Who Should Use DiDi:** Windows users who want persistent, customizable Discord Rich Presence across multiple reusable profiles while keeping credentials, profile data, and runtime state strictly local.

---

## 2. Features

- **Local Named-Pipe Discord IPC:** Automatically scans Windows named pipes `\\.\pipe\discord-ipc-0` through `discord-ipc-9`, handles version `1` `HANDSHAKE`/`READY` negotiation, replies to `PING` keepalive frames with `PONG`, and closes connections cleanly.
- **Multi-Profile Management (Up to 50 Profiles):** Create, edit, activate, duplicate, delete, import, and export named Rich Presence profiles stored locally.
- **Per-Profile or Global Application ID:** Configure one default Discord Application ID in Settings and optionally override it on any individual profile.
- **Full Activity Editor with Live Validation:** Edit activity type, activity name override, details, state, clickable URLs, artwork keys or external HTTPS image URLs, tooltips, session-elapsed or countdown timers, and up to 2 clickable buttons, with live character counters and inline field validation.
- **Approximate Local Preview:** Inspect how an activity will read in **Draft Preview** or **Live Published** mode, or side by side in **Expanded** (`760 × 620`) window layout, without making external image requests.
- **Coalesced Debounce & Rate Limiting:** Buffers rapid edits with a `500 ms` trailing debounce ("latest wins") and enforces a client-side ceiling of `4` `SET_ACTIVITY` updates per rolling `20 s` window (keeping `1` slot reserved under Discord's `5 / 20 s` limit for Pause or Exit), plus automatic honor of Discord `retry_after` cooldowns.
- **Reconnection & Error Classification:** Uses bounded exponential backoff (`1 s`, `2 s`, `4 s`, `8 s`, `15 s`, `30 s` with `±20%` jitter) for transient disconnects, while halting retries immediately on non-retryable errors such as an invalid Application ID (code `4000`).
- **System Tray & On-Demand Window Lifecycle:** Runs in the Windows notification area. Closing the settings window with **Close Window to System Tray** enabled destroys the WebView2 instance to release memory while keeping Discord Rich Presence running in Rust.
- **Windows Startup Integration:** Opt-in per-user Windows sign-in startup via `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` passing `--minimized`, reconciled against the Windows Registry on startup.
- **Pause, Resume, and Verified Clear:** Pausing persists `presence_enabled: false`, cancels pending timers, sends `CLEAR_ACTIVITY` (`SET_ACTIVITY` with `activity: null`) within a `2 s` timeout, disconnects the pipe, and warns clearly if Discord did not confirm clearing.
- **Crash-Safe Atomic Storage & Recovery:** Writes `%APPDATA%\DiDi\config.json` atomically via temp file + `flush` + `sync_all`, maintains `config.json.bak`, quarantines corrupted configs (up to 3 `.corrupt.*` files), and locks editing non-destructively if a newer schema version is detected.
- **Credential-Free `.json` Portability:** Export and import profile bundles using native Windows file dialogs with a two-step preview modal (`Keep Both`, `Overwrite`, or `Skip`). Application IDs are never exported or imported.

---

## 3. System Requirements

### Runtime Requirements (End Users)
- **Operating System:** Windows 10 x64 (Build `19041` or newer) or Windows 11 x64.
- **Architecture:** 64-bit Windows (`x86_64`).
- **Discord Client:** Discord Desktop client for Windows running under the same Windows user session (Rich Presence over local named pipes requires the desktop app; Discord in a web browser does not expose local IPC pipes).
- **WebView2 Runtime:** Microsoft Edge WebView2 Runtime (pre-installed on Windows 11 and modern Windows 10 systems; the NSIS installer automatically downloads Microsoft's bootstrapper silently if missing).

Hardware requirements are not fixed beyond those of Windows 10/11, WebView2, and Discord Desktop.

### Development Prerequisites (Building from Source Only)
- **Rust Toolchain:** Rust `1.99.1` (`x86_64-pc-windows-msvc`, `rustfmt`, `clippy`, configured in `rust-toolchain.toml`).
- **C++ Build Tools:** Microsoft Visual Studio C++ Build Tools with Windows 10/11 SDK.
- **Node.js & npm:** Node.js `22.18+` or `24 LTS` and npm `10+` (Node `22.18+` / `24` is required to run unflagged `.ts` frontend unit tests via `npm run test:frontend`).

---

## 4. Download and Installation

DiDi is distributed on GitHub Releases (`https://github.com/RetsardCo/didi/releases`) in two Windows `x64` formats:

| Distribution Format | Filename | Install Location | Privileges Required | Best For |
| :--- | :--- | :--- | :--- | :--- |
| **NSIS Installer (Primary)** | `DiDi_0.1.0_x64-setup.exe` | `%LOCALAPPDATA%\DiDi\` | Current user (no admin UAC) | Standard Windows installation with shortcuts and Apps & Features uninstall |
| **Portable Executable** | `didi.exe` | Any folder you choose | Current user (no install) | Running directly without shortcuts or installer registry entries |

Both builds share the same configuration directory (`%APPDATA%\DiDi\`). Do not run the installed binary and portable binary simultaneously; DiDi enforces a single-instance guard.

### Option A: NSIS Installer (`DiDi_0.1.0_x64-setup.exe`)
1. Download `DiDi_0.1.0_x64-setup.exe` from the release page.
2. Run the installer. No administrator elevation is requested.
3. Follow the English installer wizard:
   - Installs `didi.exe` and `uninstall.exe` to `%LOCALAPPDATA%\DiDi\`.
   - Creates a Start Menu shortcut (`DiDi`) and, when selected in the wizard (or in silent `/S` mode), a Desktop shortcut (`DiDi`).
   - Registers one per-user uninstall entry under `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\DiDi`.
   - Downloads the WebView2 bootstrapper silently only if WebView2 Runtime is not installed on the system.

#### Upgrading an Existing Install
- Run a newer `DiDi_<version>_x64-setup.exe` installer (or silent `/UPDATE`).
- DiDi is configured with `allowDowngrades: false`, preventing older installers from silently overwriting a newer installation.
- Upgrades replace the binary while preserving `%APPDATA%\DiDi\config.json`, `config.json.bak`, existing profiles, and `HKCU\...\Run` startup registration.

### Option B: Portable Executable (`didi.exe`)
1. Download `didi.exe` from the release page.
2. Place `didi.exe` in a dedicated folder (for example, `C:\Tools\DiDi\didi.exe`).
3. Double-click `didi.exe` to start.

### First Launch & Windows SmartScreen Note
- On first launch without `--minimized`, DiDi initializes `%APPDATA%\DiDi\config.json` (if missing), adds the DiDi mascot icon to the Windows system tray, and opens the **DiDi — Discord Ditto** window on the **Dashboard** tab.
- Current `0.1.0` releases are **unsigned** (`Authenticode: NotSigned`). When launching a newly downloaded executable or installer from the internet, Windows Defender SmartScreen may show a "Windows protected your PC" warning. To verify integrity before running, compare the file's SHA-256 checksum against the release checksums in PowerShell:

```powershell
Get-FileHash .\DiDi_0.1.0_x64-setup.exe -Algorithm SHA256
```

If the hash matches the published release checksum, choose **More info** -> **Run anyway**.

---

## 5. Discord Developer Portal Setup

Before DiDi can publish Rich Presence, you need your own Discord Application ID from the official Discord Developer Portal. You do not need a Discord Bot or a Discord user token.

### Step 1 — Open the Discord Developer Portal
1. Open `https://discord.com/developers/applications` in your browser (or click **Discord Developer Portal** in DiDi's **Settings** tab).
2. Sign in with your Discord account if prompted.

### Step 2 — Create an Application
1. Click **New Application**.
2. Enter an application name (for example, `Coding Session`, `Music Player`, or `Custom Status`), accept Discord's developer terms if prompted, and click **Create**.
   - Discord displays this application name as the default headline of your Rich Presence activity (for example, **Playing \<Application Name\>**) unless you specify **Activity Name Override** inside DiDi.
   - This is a standard Discord Application record, **not** a Discord Bot. Do not open the Bot tab or generate a bot token.

### Step 3 — Copy the Application ID
1. Open the **General Information** page for your new application.
2. Locate **Application ID** (a 17–20 digit numeric identifier) and click **Copy**.
3. Switch to DiDi, open the **Settings** tab, paste the number into **Default Application ID**, and click **Save Application ID**.
   - **Security note:** An Application ID is a public numeric snowflake, not a bot token or account credential. Never obtain, paste, or share your personal Discord account token.

### Step 4 — Configure Rich Presence Art Assets (Optional)
If you want custom images saved inside Discord instead of (or alongside) external image URLs:
1. In the Developer Portal sidebar for your application, expand **Rich Presence** and select **Art Assets**.
2. Click **Add Image(s)** and upload up to `300` custom images.
   - Discord strongly recommends `1024 × 1024` pixel artwork for crisp display on high-DPI screens.
3. Give each uploaded image an asset key name (lowercase letters, digits, `-`, or `_`).
   - Discord **automatically converts uploaded asset keys to lowercase** (`MyHero` becomes `myhero`). In DiDi, enter that exact lowercase key (`1..=300` chars of `a-z`, `0-9`, `-`, `_`; DiDi also auto-lowercases and trims Portal Asset Keys when normalizing).
4. Alternatively, Discord supports external `https://` image URLs directly (including GIF, animated WebP, and AVIF formats), which DiDi accepts up to `300` characters under **External HTTPS Image URL**.

### Step 5 — Save and Verify
1. Click **Save Changes** in the Developer Portal if prompted after adding assets or editing application details. (Newly uploaded assets in Discord's CDN can occasionally take a few minutes to propagate.)
2. Return to DiDi, open the **Editor** tab, set **Large Image** or **Small Image** **Source** to **Developer Portal Asset Key**, type the exact key name, and click **Save & Publish**.

---

## 6. First-Time Setup

Follow these exact steps after installing or launching DiDi:

1. **Launch DiDi:** Open **DiDi** from the Start Menu, Desktop shortcut, or portable `didi.exe`.
2. **Start Discord Desktop:** Ensure the Discord Desktop app for Windows is running and signed in.
3. **Create a Discord Application:** Follow [Section 5](#5-discord-developer-portal-setup) to create an application and copy its 17–20 digit **Application ID**.
4. **Configure Your Application ID in DiDi:**
   - Open the **Settings** tab.
   - Paste your Application ID into **Default Application ID**.
   - Click **Save Application ID**. The status pill updates to **Configured: \*\*\*\*XXXX**.
5. **Select or Create a Profile:**
   - Stay on the starter `Default Profile`, or open the **Profiles** tab -> **+ New Profile**, enter a **Profile Name** (for example, `Focus Mode`), check the **Profile Slug ID**, and click **Create Profile**.
6. **Edit Activity Information:**
   - In the **Editor** tab, pick an **Activity Type** (`Playing (0)`, `Listening to (2)`, `Watching (3)`, or `Competing in (5)`).
   - Fill in **Details (Line 1)** and/or **State (Line 2)** (`2–128` characters each).
   - Optionally set **Timestamps** (`None`, `Session Elapsed`, or `Countdown Until Target Date/Time`).
7. **Configure Images (Optional):**
   - Under **Artwork** -> **Large Image** or **Small Image**, choose **Developer Portal Asset Key** or **External HTTPS Image URL** and fill in optional hover tooltip text (`2–128` chars).
8. **Preview the Activity:**
   - Open the **Preview** tab (or switch **Settings -> Window Layout** to **Expanded (760 × 620 split preview)** to see **Draft Preview** alongside the **Editor**).
9. **Save & Publish:**
   - Ensure the profile is active (otherwise activate it on **Dashboard** or **Profiles**), then click **Save & Publish** (or **Save Profile**) in the sticky editor bar.
10. **Confirm Presence Enabled:**
    - On the **Dashboard** tab (or top header), confirm the button reads **Pause** (meaning presence is enabled; if it reads **Resume**, click **Resume**). Wait for the top status pill to show **● Published**.
11. **Verify in Discord:**
    - Check your Discord user profile popout / status in the Discord Desktop client.
    - If connected as **● Published** in DiDi (`Named Pipe: discord-ipc-0 (v1)`) but hidden on your Discord profile, open Discord **User Settings** -> **Activity Settings** -> **Activity Privacy** and confirm **Share my activity** (or per-server **Privacy Settings -> Activity Status**) is enabled.

---

## 7. Dashboard

The **Dashboard** tab monitors Discord IPC health and provides quick profile switching and lifecycle controls.

### Top Bar (Visible Across All Tabs)
- **Brand Header:** Shows the DiDi mascot icon, `DiDi`, version badge (`v0.1.0`), and active profile subtitle (`Active: <Profile Name>`).
- **Live Status Pill (`role="status"`):** Displays the coordinator's real state:
  - `○ Starting` — Backend initialization.
  - `⏸ Paused` — Presence disabled (`Disabled`); IPC disconnected.
  - `○ Waiting for Discord` — Presence enabled, scanning pipes or waiting between reconnects.
  - `◐ Connecting` — Named pipe opened; waiting for `READY` or `SET_ACTIVITY` ACK.
  - `● Published` — Local Discord client acknowledged `SET_ACTIVITY`.
  - `↻ Reconnecting` — Retrying automatically during the first 2 disconnects after a published session.
  - `✕ Action Required` — Non-retryable error (such as invalid Application ID or unrecoverable future config schema).
  - `■ Exiting` — Shutdown sequence in progress.
- **Quick Toggle Button (`Pause` / `Resume`):** Toggles presence immediately without leaving your current tab. Disabled only while busy or when configuration editing is locked.

### System Notices Stack (`#global-notices`)
Displays cross-tab alerts above the tabs when needed:
- **Future Schema Lock (Danger):** Shown when `config.json` has `schema_version > 1`. Editing is blocked to protect newer data.
- **Backup Recovery / Corrupt Reset (Warning):** Shown on startup if `config.json` was corrupt and either recovered from `config.json.bak` or reset to defaults (names the quarantined file).
- **Unconfirmed Clear Warning (Warning):** Shown when paused if `CLEAR_ACTIVITY` timed out, failed, or disconnected before Discord acknowledged clearing (dismissible).
- **Command Errors (Danger):** Displays sanitized action errors (never exposes local OS paths or raw RPC peer messages).

### Missing Application ID Callout
Appears above **Discord Runtime Status** whenever neither the active profile nor **Settings -> Default Application ID** has a configured Application ID (`▲ App ID Missing`), with a **Configure in Settings** shortcut button.

### Discord Runtime Status Card
- **Presence Enabled:** `Enabled` or `Paused`.
- **Discord State:** Current state label (`Published`, `Disabled`, `Waiting for Discord`, `Connecting`, `Reconnecting`, `Action Required`, `Starting`, `Exiting`).
- **Application ID:** Masked last 4 digits (`****1234 (Default)` or `****1234 (Profile override)`), or `Not configured`.
- **Named Pipe:** Active IPC slot and protocol version (`discord-ipc-0 (v1)`), or `Not attached`.
- **Application Identity:** `DiDi — Discord Ditto (0.1.0)`.
- **Contextual Helper Note:** Clarifies when activity is acknowledged over IPC (and that Discord's Activity Privacy controls final visibility) or when DiDi is awaiting `SET_ACTIVITY` ACK before marking `Published`.
- **Action Buttons:** **Pause Presence** / **Resume Presence**, plus **Retry Now** whenever the state is `Waiting for Discord`, `Reconnecting`, or `Action Required`.

### Active Profile & Quick Switch Card
- **Activity Type Chip:** Displays the verb for the active profile (`Playing`, `Listening to`, `Watching`, `Competing in`).
- **Selected Profile:** Dropdown listing all saved profiles; switching immediately saves `active_profile_id` and syncs the new activity/App ID with `PresenceService`. (Prompts first if the **Editor** has unsaved edits.)
- **Summary Rows:** Shows **Activity Name** (`(Developer Portal App Name)` if no override), **Details**, and **State**.
- **Shortcut Buttons:** **Edit Active Profile**, **View Preview**, and **Manage Profiles**.

### Startup & Tray Summary Card
Displays read-only summary chips (**Windows Startup: On/Off**, **Close to Tray: On/Off**, **Layout: Compact/Expanded**) and a **Settings** shortcut button.

---

## 8. Profiles

The **Profiles** tab manages up to **50** saved Rich Presence profiles (`X / 50`).

### Toolbar Actions
- **+ New Profile:** Opens the **Create New Profile** modal (disabled when 50 profiles exist or editing is locked):
  - **Profile Name:** Required, `1–64` characters, single-line, no control characters.
  - **Profile Slug ID:** Required, `1–64` characters of lowercase `a–z`, `0–9`, `-`, or `_`. Automatically generated from **Profile Name** unless edited manually.
  - On creation, DiDi saves the profile and opens it directly in the **Editor** tab.
- **Import (.json):** Opens a native Windows file picker filtered to `.json` (`<= 256 KiB` limit), parses the bundle (`bundle_version: 1`), and opens the **Review Profile Import** dialog:
  - Lists **New Profiles** and **Conflicting Profiles**.
  - Lets you choose a conflict strategy when matching IDs exist:
    - **Keep Both (Rename imported duplicates):** Appends `-2`, `-3`, etc. to conflicting IDs (order-independent) and ` (Imported)` to names within the 64-char limit.
    - **Overwrite existing profiles with same ID**
    - **Skip conflicting profiles (import new only)**
  - Warns if the import would exceed the 50-profile limit. Confirms via a single-use server-side token. Never modifies Application IDs.
- **Export All (.json):** Opens a native Windows save dialog (`didi-profiles.json`) and exports all profiles as a credential-free `ExportBundleV1` JSON file (all `application_id_override` values, `default_application_id`, preferences, and local paths are excluded).

### Per-Profile Card Actions
Each saved profile shows its name, slug ID, activity verb chip, masked `Override: ****XXXX` chip (if an Application ID override is configured), details/state summary, and action buttons:
- **● Active** badge or **Activate** button (sets active profile and updates Discord presence).
- **Edit:** Loads the profile into the **Editor** tab (prompts if another profile has unsaved changes).
- **Duplicate:** Clones the profile with a unique `-copy` slug ID and ` (Copy)` name suffix (clamped cleanly to 64 Unicode codepoints), then selects it in the editor. Disabled at 50 profiles.
- **Export:** Exports only that single profile to `.json`.
- **Delete:** Prompts **Delete Profile?** confirmation before removal. If the deleted profile was active, DiDi switches active selection to the first remaining profile and syncs presence. Disabled on the last remaining profile (`>= 1` profile is always required).

---

## 9. Presence Editor

The **Editor** tab modifies a selected profile draft (`ProfileV1`) with live inline validation, character counters, and a sticky save bar.

Fields marked **`D-016`** in the UI (`Activity Name Override`, `Status Display`, `Details Clickable URL`, `State Clickable URL`, `Large/Small Clickable Image URL`, and `Action Buttons`) are supported by DiDi's validated wire model (`WireActivity`), while actual client display depends on Discord desktop client support and your Developer Portal application configuration.

### Profile Identity Card

| Field | Required | Validation & Normalization | Discord Display |
| :--- | :--- | :--- | :--- |
| **Editing Profile** | Yes | Select any saved profile (prompts if current draft has unsaved changes) | Selects which local profile you are editing |
| **Profile Name** | Yes | `1–64` chars after trim; single-line; no control characters (`\n`, `\r`, `\t`, etc.) | Local DiDi label only (not sent to Discord) |
| **Application ID Override** | No | Empty/blank uses Settings default; otherwise `17–20` ASCII digits, non-zero, no control chars; masked input with **Reveal**/**Hide** & **Clear** | Selects the Discord Developer Portal application used for this profile |

### Activity Core Card

| Field | Required | Values / Validation | Discord Display |
| :--- | :--- | :--- | :--- |
| **Activity Type** | Yes | `Playing (0)`, `Listening to (2)`, `Watching (3)`, `Competing in (5)` | Verb prefix displayed on Discord ("Playing...", "Listening to...", etc.) |
| **Status Display (`D-016`)** | Yes | `Application / Activity Name (0)`, `State Line (1)`, `Details Line (2)` | `status_display_type` wire field suggesting which text line Discord shows in member list status |
| **Activity Name Override (`D-016`)** | No | Empty = null (uses Portal App Name); otherwise `2–128` chars, single-line, no control chars | Optional `name` override sent in `SET_ACTIVITY` |

### Details & State Card

| Field | Required | Validation | Discord Display |
| :--- | :--- | :--- | :--- |
| **Details (Line 1)** | No | Empty = null; otherwise `2–128` chars trimmed, single-line, no control chars | Primary description line beneath the activity title |
| **Details Clickable URL (`D-016`)** | No | Empty = null; strict public `https://` URL, `<= 256` chars, no whitespace/control chars/`\`/credentials/private or local hosts | Makes Line 1 clickable in Discord clients that support `details_url` |
| **State (Line 2)** | No | Empty = null; otherwise `2–128` chars trimmed, single-line, no control chars | Secondary status line below Details |
| **State Clickable URL (`D-016`)** | No | Empty = null; strict public `https://` URL, `<= 256` chars, same SSRF/host rules | Makes Line 2 clickable in Discord clients that support `state_url` |

### Artwork Card (Large Image & Small Image)
Repeated identically for **Large Image** (`large_*`) and **Small Image** (`small_*`):

| Field | Required | Validation | Discord Display |
| :--- | :--- | :--- | :--- |
| **Source** | Yes | `None`, `Developer Portal Asset Key`, or `External HTTPS Image URL` | Chooses how image slot is populated |
| **Portal Asset Key** *(when chosen)* | Required for mode | `1–300` chars of lowercase `a–z`, `0–9`, `-`, `_`; auto-trimmed & auto-lowercased on normalize; no control chars | References an asset uploaded in Developer Portal -> **Rich Presence -> Art Assets** |
| **External Image HTTPS URL** *(when chosen)* | Required for mode | Strict public `https://` URL, `<= 300` chars, no whitespace/credentials/private/loopback hosts | Discord client fetches/proxies the external image URL |
| **Hover Tooltip Text** | No | Empty = null; otherwise `2–128` chars, single-line, no control chars | Hover tooltip on Large (`large_text`) or Small (`small_text`) image |
| **Clickable Image URL (`D-016`)** | No | Empty = null; strict public `https://` URL, `<= 256` chars, public host rules | Link opened if user clicks the image (`large_url` / `small_url`) |

### Timestamps Card

| Field | Required | Values / Validation | Discord Display |
| :--- | :--- | :--- | :--- |
| **Timestamp Mode** | Yes | `None`, `Session Elapsed`, or `Countdown Until Target Date/Time` | Controls timer line on Rich Presence |
| **Countdown Target** *(when Countdown selected)* | Required for mode | Valid local date/time (`datetime-local`), stored as UTC Unix seconds (`1..=32,503,680,000`). Expired targets (`<= now`) are automatically omitted on publish. | Displays countdown (`XX:XX left`) until target time (`end_timestamp`); `Session Elapsed` resets its `start_timestamp` on publish/reconnect/pause (`00:00 elapsed`) |

### Action Buttons Card (`D-016`, `0–2` Buttons)

| Field | Required | Validation | Discord Display |
| :--- | :--- | :--- | :--- |
| **Button Label** | Required per button | `1–32` chars trimmed, single-line, no control chars | Button caption shown on Discord profile card |
| **Button HTTPS URL** | Required per button | Non-empty strict public `https://` URL, `<= 512` chars, no credentials/whitespace/private or local hosts | URL opened when another user clicks the button |

### Sticky Save Bar & Unsaved Changes Guard
- **Indicators:** Shows `Valid` or `<N> validation issue(s)` and `Saved` or `Unsaved edits`.
- **Actions:** **Discard** reverts draft changes back to disk state; **Save & Publish** (when editing the active profile with presence enabled) or **Save Profile** normalizes fields, saves `config.json` atomically, and syncs active presence.
- **Navigation Guard:** Switching between **Editor** and **Preview** preserves your unsaved draft without interrupting you. Navigating from either **Editor** or **Preview** to **Dashboard**, **Profiles**, or **Settings** (or switching active/edited profile) while dirty prompts **Unsaved Profile Changes** (**Save Changes**, **Discard Edits**, or **Stay Here**). Modal focus traps `Tab`/`Shift+Tab`, supports `Escape`, and restores focus to the triggering element.

---

## 10. Preview

The **Preview** tab (and the docked right-hand pane in **Editor** when **Window Layout** is **Expanded** and viewport width is `>= 720 px`) renders a local approximation of a Discord Rich Presence card.

- **Draft Preview:** Reflects your current **Editor** draft (`appState.editorDraft`)—including unsaved edits—or the active profile if no draft is open. Shows `00:00 elapsed (starts upon publish)` for `Session Elapsed` and a live 1-second tick for future `Countdown` targets.
- **Live Published:** Reads the actual snapshot last acknowledged by Discord IPC (`presenceStatus.active_activity`). Shows an empty notice whenever DiDi is `Paused`, `Waiting for Discord`, `Connecting`, or not yet published. Updates elapsed and countdown timers once per second while visible.
- **Preview Limitations (Not a Live Discord Render):**
  - The banner explicitly states: *"Approximate Local Preview — Actual Discord rendering may vary. External artwork URLs are never fetched by DiDi UI."*
  - DiDi never downloads external images inside WebView2 (`connect-src 'none'` CSP); Large and Small artwork boxes show `No Art`, `Key: <asset_key>`, or `HTTPS: <hostname>` placeholder badges.
  - Because DiDi does not query Discord REST APIs, **Activity Name** defaults to `(Developer Portal App Name)` unless **Activity Name Override** is set.
  - Action buttons are disabled (`Preview buttons do not navigate`).

---

## 11. Settings

### Discord Application ID (BYO-AppID)
- Shows a status pill (`Configured: ****XXXX` or `Not Configured`).
- **Default Application ID:** Masked input with **Reveal** / **Hide** toggle. Enter your 17–20 digit numeric Application ID from the Discord Developer Portal.
- **Save Application ID:** Validates and saves atomically to `%APPDATA%\DiDi\config.json`, then reconnects `PresenceService` if active.
- **Clear:** Removes the saved default Application ID.
- **Discord Developer Portal:** Opens `https://discord.com/developers/applications` in your default browser via allowlisted URL opener.

### Windows Integration & Layout

| Setting | Storage & Mechanism | Behavior & Persistence |
| :--- | :--- | :--- |
| **Launch at Windows Startup** | Persisted in `config.json` (`launch_at_login`) and synced atomically with `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` (`--minimized`) | Automatically rolls back Registry if disk save fails. On startup, DiDi treats the Windows Registry as authoritative (so disabling DiDi in Windows Task Manager -> Startup Apps updates `config.json` to `false` without re-enabling Registry). |
| **Start Minimized to System Tray** | Persisted in `config.json` (`start_minimized`, default `true`) | Intended tray startup preference. **Current implementation note:** DiDi checks the `--minimized` launch flag (`lib.rs`); launches from Windows Startup pass `--minimized` and start hidden in the tray, whereas manual launches without `--minimized` currently open the Settings window regardless of this toggle. |
| **Close Window to System Tray** | Persisted in `config.json` (`close_to_tray`, default `true`) | When **On**, closing the window destroys the WebView2 instance to free RAM while the Rust background service and tray icon keep running. When **Off**, clicking window Close runs bounded clean shutdown and exits DiDi completely. |
| **Window Layout** | Persisted in `config.json` (`ui_layout`: `compact` or `expanded`) | Immediately resizes the window between **Compact (`480 × 600` default)** and **Expanded (`760 × 620` split preview)**. |

### Diagnostics & Data
- **Open Logs Folder:** Ensures `%APPDATA%\DiDi\logs\` exists and opens it in Windows Explorer. **Note:** DiDi `0.1.0` does not currently write rotating application log files into `logs/` (critical startup initialization errors write only to standard error if launched from a console).
- **Import Profiles (.json) / Export All Profiles (.json):** Runs the same credential-free import/export flows as the **Profiles** tab.
- **Project Documentation / Report an Issue:** Opens allowlisted repository (`https://github.com/RetsardCo/didi`) or issue tracker (`https://github.com/RetsardCo/didi/issues`) URLs in your default browser.

### About, Compliance & Lifecycle
- Displays app identity (`DiDi — Discord Ditto (v0.1.0)`), tagline (`Your Activity. Your Identity.`), and local-first privacy summary.
- **Exit DiDi Completely:** Opens an **Exit DiDi Completely?** confirmation modal (`Exit DiDi` / `Cancel`) that triggers clean shutdown (`CLEAR_ACTIVITY` + `CLOSE` within a `2.2 s` timeout) and terminates the process.

---

## 12. System Tray

DiDi runs a native Windows notification area tray icon (`didi-tray`) using the app's mascot icon:

- **Left-Click Tray Icon:** Opens the **DiDi — Discord Ditto** WebView2 window (or unminimizes and focuses it if already open).
- **Right-Click Context Menu:**
  - `Status: <state>` (disabled info item; shows `Status: Disabled (Clear Unconfirmed)` if activity clear could not be verified on Pause).
  - **Open DiDi:** Opens or focuses the application window.
  - **Pause Presence** / **Resume Presence:** Uses the exact same synchronized path as the UI toggle (`set_presence_enabled_synced`), persisting `presence_enabled` and updating open UI state immediately via `presence://status`.
  - **Launch at Windows Startup:** Checkable menu item synced with `HKCU\...\Run` and `config.json`; notifies any open **Settings** window via `config://autostart-changed` (`{ launch_at_login }`).
  - **Exit:** Initiates clean shutdown (no modal dialog from tray) and exits DiDi.

Hovering the tray icon shows a dynamic tooltip (`DiDi — <state>`, or `DiDi — Disabled (Clear Unconfirmed)`).

---

## 13. Pause, Resume, and Clear Activity

Understanding how DiDi transitions between states prevents confusion about what stays on your Discord profile:

### What Happens When You Pause
1. DiDi persists `presence_enabled: false` in `config.json`.
2. Pending trailing debounce (`500 ms`), rate-limit wait, and reconnect backoff timers are cancelled, and any buffered unsent activity is cleared.
3. If an IPC pipe is currently connected, DiDi sends `CLEAR_ACTIVITY` (`SET_ACTIVITY` with `"activity": null`) with a shared `2.0 s` write + ACK deadline.
4. Whether clearing succeeds or fails, DiDi runs best-effort `CLOSE` (`2.0 s` timeout) + reader cleanup so Pause never gets stuck in an active/reconnecting state, and moves state to `Paused` (`Disabled`).
5. **Clear Error Honesty:**
   - If clearing succeeded, DiDi shows toast `Presence paused.` and removes any prior clear warning.
   - If clearing failed or timed out (for instance, Discord closed or rate-limited mid-pause), DiDi **does not claim Discord activity was cleared**; instead, it stores a sanitized warning in `last_error`, displays a warning toast, shows a cross-tab notice banner in `#global-notices`, and labels the tray status `Disabled (Clear Unconfirmed)`.

### What Happens When You Resume
1. DiDi persists `presence_enabled: true` in `config.json`, clears `last_error`, resets reconnect backoff, and resolves the active profile + effective Application ID.
2. If an Application ID is configured, DiDi transitions to `Waiting for Discord` -> `Connecting`, opens the pipe, and publishes the active profile activity (subject to any active rate-limit cooldown remaining on the connection).
3. If no Application ID is configured, DiDi remains `Disabled` and prompts on the Dashboard.

### What Happens When Clear Activity Runs
- Users trigger clearing by clicking **Pause** / **Pause Presence**, exiting DiDi (`request_exit`), or changing the effective Application ID (which sends `CLEAR_ACTIVITY` and closes the old pipe before connecting with the new Application ID).
- If Discord rejects `CLEAR_ACTIVITY` via RPC error (`RpcRejected`, `InvalidClientId`, protocol error) while keeping the pipe open during an internal clear command, `active_activity` is retained instead of falsely claiming it cleared.

### What Happens When Discord Is Closed or Reconnects
- **Discord Closes While Running:** Once an active pipe breaks (`EOF`, broken pipe, or `Opcode::Close`), DiDi clears `connected`, emits a sanitized status error, and enters `Reconnecting` for the first 2 consecutive failures after a `Published` session before moving to `Waiting for Discord`.
- **Reconnection Schedule:** Exponential backoff delays `[1 s, 2 s, 4 s, 8 s, 15 s, 30 s]` with `±20%` pseudo-random jitter until Discord restarts and opens `\\.\pipe\discord-ipc-{0..9}`, after which DiDi automatically republishes your active profile (and resets `Session Elapsed` start time for the new session).
- **Rate Limiting Limits:** Rapid edits within 20 seconds queue behind the rolling 4-per-20s limiter ("latest wins"); if Discord returns `4002` (`RateLimited`), DiDi disconnects cleanly and honors Discord's `retry_after` duration (default `25 s` if omitted) before reconnecting.

---

## 14. Configuration and Data Storage

DiDi stores all user data locally in one directory on Windows:

```text
%APPDATA%\DiDi\
  config.json                 # Active configuration (schema_version = 1)
  config.json.bak             # Last known valid configuration backup
  config.json.corrupt.<ts>    # Quarantined damaged files (at most 3 kept)
  logs\                       # Created if you click "Open Logs Folder"
```

### `config.json` Contents (Schema Version `1`)
- `schema_version`: `1`
- `default_application_id`: Global fallback 17–20 digit Application ID (or `null`)
- `active_profile_id`: Currently selected profile slug ID
- `presence_enabled`: Boolean (`true` = active, `false` = paused)
- `launch_at_login`, `start_minimized`, `close_to_tray`: Windows preferences
- `ui_layout`: `"compact"` or `"expanded"`
- `profiles`: Array of `1–50` `ProfileV1` objects (`id`, `name`, optional `application_id_override`, `activity_type`, `status_display_type`, text fields, artwork refs, timestamps, buttons)

### Atomic Persistence & Corruption Recovery
1. **Atomic Main Write:** Every save validates `AppConfigV1` in memory, serializes JSON to `config.json.tmp`, calls `flush()` and `sync_all()`, and atomically replaces `config.json` (with bounded sharing-violation retries).
2. **Atomic Verified Backup:** Before replacing an existing `config.json`, DiDi parses the previous on-disk `config.json`. Only if it is valid does DiDi write `config.json.bak.tmp`, flush + sync, and rename to `config.json.bak`. A corrupt file never overwrites a good backup.
3. **Quarantine & Auto-Recovery on Startup:**
   - If `config.json` fails to parse or validate on launch (for example, truncated after power loss), DiDi moves it to `config.json.corrupt.<timestamp>` (retaining at most `3` quarantine files) and attempts to restore from `config.json.bak`.
   - If both `config.json` and `config.json.bak` are corrupt, DiDi quarantines them, initializes safe defaults (`presence_enabled: true`, `default` starter profile), and notifies you in the top banner.
4. **Future Schema Protection:** If `config.json` contains an integer `schema_version > 1` (written by a newer version of DiDi), DiDi refuses to overwrite, quarantine, or mutate settings and disables editing with an upgrade warning.
5. **Backup & Upgrade Safety:**
   - **To share or back up profile definitions safely:** Use **Profiles -> Export All (.json)** (strips all Application IDs).
   - **To back up your full local setup including Application IDs privately:** Copy `%APPDATA%\DiDi\config.json` to a private location (never commit it or share it publicly).
   - Both upgrades and standard uninstalls leave `%APPDATA%\DiDi\` intact.

---

## 15. Troubleshooting

### Discord Not Detected (`Waiting for Discord`)
1. **Symptoms:** Status shows `○ Waiting for Discord`, Named Pipe shows `Not attached`.
2. **Possible verified causes:** Discord Desktop is not running; Discord is running only in a web browser; Discord is running under a different Windows user/elevation context (`ERROR_ACCESS_DENIED` on pipe); all 10 named pipe slots (`discord-ipc-0..9`) report `PipeBusy` (`ERROR_PIPE_BUSY`).
3. **Diagnostic actions:**
   - Open the Discord Desktop app (not discord.com in Chrome/Edge) and wait 10 seconds, or click **Retry Now**.
   - Ensure Discord Desktop is not running elevated as Administrator while DiDi runs as standard user.
   - Close other Rich Presence utilities occupying Discord IPC pipes and click **Retry Now**.
4. **Expected result:** Status moves to `◐ Connecting` and then `● Published`, with Named Pipe displaying `discord-ipc-0 (v1)`.
5. **What to provide when reporting an issue:** Windows 10/11 build, Discord Desktop version, DiDi version (`0.1.0`), whether Discord ran elevated, and redacted runtime status fields from Dashboard.

### DiDi Not Connecting / Keeps Retrying
1. **Symptoms:** Status cycles `Connecting` -> `Waiting for Discord` or pauses in `Reconnecting`, or Dashboard shows `"Connection to Discord desktop was interrupted"` or rate-limit cooldown text.
2. **Possible verified causes:** Discord desktop restarted; handshake timed out (`5 s`); Discord returned error `4002` (`RateLimited`, triggering automatic cooldown).
3. **Diagnostic actions:**
   - Wait 30 seconds for backoff/cooldown to complete, or click **Retry Now**.
   - Confirm Discord Desktop is fully signed in (not stuck on the updater/login screen).
4. **Expected result:** Reconnect timer completes or **Retry Now** connects cleanly to `● Published`.
5. **What to provide:** DiDi status label (`Discord State`), `Named Pipe` value, whether you recently triggered multiple profile switches/edits, and redacted error banner text.

### Presence Not Appearing in Discord Despite `● Published`
1. **Symptoms:** DiDi Dashboard displays `● Published` and `discord-ipc-0 (v1)`, but friends or your profile do not show the activity.
2. **Possible verified causes:** Discord **Activity Privacy** is disabled globally or for that server; your Discord status is **Invisible**; `D-016` fields (such as custom URLs, buttons, or `name_override`) are filtered/ignored by the Discord client; Discord caches user profile view briefly.
3. **Diagnostic actions:**
   - In Discord Desktop: **User Settings -> Activity Settings -> Activity Privacy** -> verify **Share my activity** is **On**.
   - Check per-server **Privacy Settings -> Activity Status** in the server where you are testing.
   - Verify your Discord online presence is **Online**, **Idle**, or **Do Not Disturb** (not **Invisible**).
   - Test with a plain profile (`Activity Type: Playing`, `Details`, `State`, no `D-016` overrides or buttons) to isolate whether Discord rejected optional `D-016` fields.
4. **Expected result:** Friends (and your own profile card) show **Playing \<Developer Portal App Name\>** with Details and State.
5. **What to provide:** Screenshot of DiDi's Dashboard card (masking any personal details), which fields (`D-016` vs core Details/State) were configured, and your Discord Activity Privacy toggle state.

### Invalid Application ID (`✕ Action Required`)
1. **Symptoms:** Status shows `✕ Action Required` with `"Discord rejected your Application ID. Verify it in the Discord Developer Portal."`, or Editor/Settings rejects input before saving.
2. **Possible verified causes:** Entered value is not 17–20 ASCII digits, is all zeros, matches a Bot Token / Client Secret / User ID instead of an Application ID, was deleted in Discord Developer Portal, or is a newly created App ID not yet accepted.
3. **Diagnostic actions:**
   - Open `https://discord.com/developers/applications`, select your app -> **General Information**, and copy **Application ID** again.
   - Verify in **Settings -> Default Application ID** (click **Reveal** privately to double-check length `17–20` digits, then **Save Application ID**), and verify the active profile does not have a stale **Application ID Override** in **Editor**.
   - Click **Retry Now** if needed.
4. **Expected result:** Status moves out of `✕ Action Required` to `● Published`.
5. **What to provide:** Never paste your real Application ID or tokens. Mention character count/error message and whether you entered it in Settings or Profile Override.

### Missing Image Assets
1. **Symptoms:** Activity publishes text cleanly, but artwork in Discord is blank or missing, while DiDi Preview only shows `Key: ...` or `HTTPS: ...`.
2. **Possible verified causes:** Asset key in DiDi does not exactly match the asset name in Developer Portal **Rich Presence -> Art Assets**; uppercase letters or typos in key before Portal upload; recently uploaded Portal asset still propagating in Discord's CDN (can take several minutes); **External Image HTTPS URL** points to a private/local IP, non-HTTPS URL, non-image endpoint, or a server blocking Discord's media proxy.
3. **Diagnostic actions:**
   - In **Editor**, check whether **Source** is **Developer Portal Asset Key** or **External HTTPS Image URL**.
   - Confirm the lowercase asset key matches Discord Developer Portal -> **Rich Presence -> Art Assets** character-for-character and click **Save Changes** in the Developer Portal.
   - For external URLs, verify the URL opens publicly in an incognito browser as a direct image over `https://`.
   - Wait 5–10 minutes after newly uploading assets in the Developer Portal, then pause and resume DiDi (`Pause` -> wait 2 seconds -> `Resume`).
4. **Expected result:** Large and Small artwork icons appear on the Discord activity card after CDN propagation.
5. **What to provide:** Artwork source mode (`Portal Key` vs `External HTTPS`), asset dimensions/format, and whether Portal assets were uploaded less than 15 minutes prior.

### Activity Not Clearing on Pause or Exit
1. **Symptoms:** Clicking **Pause** or **Exit** leaves your old status visible in Discord, and DiDi shows a warning banner or tray text `Status: Disabled (Clear Unconfirmed)`.
2. **Possible verified causes:** `CLEAR_ACTIVITY` timed out after `2.0 s`; Discord closed or broke the IPC pipe before ACKing clear; Discord rate-limited (`4002`) the clear command; DiDi was terminated forcefully via Task Manager / process kill instead of normal Exit or Pause.
3. **Diagnostic actions:**
   - Always exit DiDi via **System Tray -> Exit** or **Settings -> Exit DiDi Completely** (which grants up to `2.2 s` for `CLEAR_ACTIVITY` + `CLOSE`), never `End Task` unless frozen.
   - If you see `Disabled (Clear Unconfirmed)`, make sure Discord is running, click **Resume**, wait for `● Published`, then click **Pause** again.
4. **Expected result:** `Presence paused.` toast appears with no warning banner, and Discord removes the activity.
5. **What to provide:** Exact warning banner text displayed by DiDi and whether Discord remained open during Pause/Exit.

### Pause / Resume Problems
1. **Symptoms:** **Pause** / **Resume** button is disabled, or Tray/UI appear out of sync.
2. **Possible verified causes:** Configuration is locked due to `UnsupportedFutureSchema` (`schema_version > 1`) or startup storage failure; another busy command (import/save) is in flight; no Application ID is configured when clicking Resume (DiDi intentionally drops back to `Disabled` if no Application ID exists).
3. **Diagnostic actions:**
   - Check `#global-notices` at the top of the window for future-schema or storage alerts.
   - Open **Settings** and confirm a valid **Default Application ID** (or per-profile override) is configured.
4. **Expected result:** Toggling Pause/Resume updates both the header pill, Dashboard rows, and Tray menu label consistently.
5. **What to provide:** Presence of any `#global-notices` banner, `Presence Enabled` / `Discord State` rows from Dashboard, and whether you clicked via Tray or UI.

### Startup Issues (Does Not Launch at Sign-In or Checkbox Resets)
1. **Symptoms:** DiDi does not start on Windows sign-in, or **Launch at Windows Startup** turns **Off** after restarting DiDi.
2. **Possible verified causes:** DiDi was disabled in Windows **Task Manager -> Startup apps** (`reconcile_autostart_on_startup` reads Registry as authoritative and syncs `config.json` to `false`); DiDi was uninstalled using NSIS (which removes `HKCU\...\Run\DiDi`) and then reinstalled; a portable `didi.exe` was moved to a different folder after enabling startup; permissions/group policy blocked `HKCU\...\Run`.
3. **Diagnostic actions:**
   - Check Windows **Task Manager -> Startup apps** and ensure **DiDi** is not disabled.
   - Open DiDi **Settings** (or right-click **Tray**) and toggle **Launch at Windows Startup** back **On**.
   - Check Windows Registry key `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` for value `DiDi` pointing to your current `didi.exe --minimized` path:
     ```powershell
     Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name DiDi -ErrorAction SilentlyContinue
     ```
4. **Expected result:** Registry entry matches `"<path-to-didi.exe>" --minimized` and **Launch at Windows Startup** remains **On** across restarts.
5. **What to provide:** Portable vs NSIS installer usage, Task Manager startup status, and redacted `HKCU\...\Run\DiDi` value (masking `<username>`).

### Configuration Errors (`RecoveredFromBackup`, `ResetToDefaultAfterCorruption`, or `UnsupportedFutureSchema`)
1. **Symptoms:** Top notice banner warns that configuration was corrupted and recovered/reset, or editing is disabled because config was created by a newer version of DiDi.
2. **Possible verified causes:** Sudden power loss or disk interruption during external edits corrupted `config.json`; you manually edited `config.json` with invalid JSON or non-standard fields; you downgraded DiDi after running a newer schema version.
3. **Diagnostic actions:**
   - Inspect `%APPDATA%\DiDi\` in Explorer. Check `.corrupt.<timestamp>` files if you need to recover custom text manually.
   - On `UnsupportedFutureSchema`, upgrade DiDi instead of editing `config.json` by hand.
   - Use **Profiles -> Export All (.json)** regularly to keep clean portable backups.
4. **Expected result:** DiDi boots safely, preserves newer schemas untouched, or restores cleanly from `config.json.bak`.
5. **What to provide:** Exact `#global-notices` banner text and `schema_version` integer (never paste your real Application ID).

### Installation or Upgrade Problems
1. **Symptoms:** `DiDi_0.1.0_x64-setup.exe` refuses to overwrite, fails mid-install/uninstall, or SmartScreen blocks execution.
2. **Possible verified causes:** `didi.exe` is still running in the system tray (or exited less than ~2 seconds earlier while WebView2/RestartManager finishes closing handles); attempting to install an older version over a newer version (`allowDowngrades: false`); antivirus/SmartScreen quarantine on an unsigned binary.
3. **Diagnostic actions:**
   - Right-click the DiDi tray icon -> **Exit**, wait 2–3 seconds, and verify zero `didi.exe` processes remain before running installer or uninstaller:
     ```powershell
     Get-Process didi -ErrorAction SilentlyContinue
     ```
   - Verify installer SHA-256 checksum matches the GitHub Release asset.
4. **Expected result:** Clean non-elevated install into `%LOCALAPPDATA%\DiDi\` and single uninstall entry under `HKCU\...\Uninstall\DiDi`.
5. **What to provide:** Windows 10/11 version, installer filename, exit code or exact dialog error, and whether an earlier `didi.exe` instance was open.

### WebView2 Problems (Blank Window or Missing Runtime)
1. **Symptoms:** Installer prompts/fails during WebView2 download, or opening the DiDi window shows a blank white/dark frame while tray runs normally.
2. **Possible verified causes:** Offline environment when installer attempts silent `downloadBootstrapper`; corrupted or missing Microsoft Edge WebView2 Evergreen Runtime on Windows 10.
3. **Diagnostic actions:**
   - Verify WebView2 Runtime presence via **Windows Settings -> Apps -> Installed apps** (`Microsoft Edge WebView2 Runtime`) or Registry:
     ```powershell
     Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' -ErrorAction SilentlyContinue | Select-Object pv
     ```
   - If missing or damaged offline, download Microsoft's official WebView2 Evergreen Runtime Standalone Installer directly from Microsoft and repair/install it, then reopen DiDi.
4. **Expected result:** `DiDi — Discord Ditto` window renders crisp Midnight UI (`Dashboard`, `Profiles`, `Editor`, `Preview`, `Settings`).
5. **What to provide:** Windows 10/11 build, WebView2 `pv` version string, and whether DiDi's tray icon and presence publishing still function.

---

## 16. Privacy and Security

DiDi is engineered around strict local-first boundaries enforced at compile time (`unsafe_code = "forbid"`, `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]`) and runtime:

- **Local-First Operation & IPC Only:** Communicates solely with the local Discord Desktop client via Windows named pipes (`\\.\pipe\discord-ipc-{0..9}`).
- **No Discord User Tokens or Bot Tokens:** DiDi never asks for, reads, transmits, or stores Discord user tokens, account passwords, MFA codes, session cookies, or bot tokens. Normal Rich Presence needs only your public Discord Application ID.
- **Zero Discord User PII Retention:** Incoming `READY` handshake payloads omit user profile fields (`user.id`, `username`, `avatar`, `discriminator`) during deserialization (`ReadyPayload`).
- **No Process Scanning:** DiDi never enumerates Windows processes, reads memory of other applications, or tracks active games.
- **Strict Content Security Policy & Least-Privilege Tauri Host:**
  - `tauri.conf.json` enforces `connect-src 'none'`, `object-src 'none'`, `frame-ancestors 'none'`, `base-uri 'none'`, and `default-src 'self'`.
  - `capabilities/default.json` limits the `"settings"` WebView window exclusively to `core:event:default`, `core:window:allow-close`, and `core:window:allow-set-focus`.
  - JavaScript DOM rendering uses pure `textContent` APIs (zero `innerHTML`).
  - Native file dialogs run entirely in Rust (`tauri-plugin-dialog`) using opaque preview tokens (`DEF-12`), preventing JavaScript from supplying or reading raw OS paths.
  - All external button/asset/activity URLs undergo SSRF/host hardening rejecting `http://`, localhost/private IPv4/IPv6 ranges (including octal/mapped/NAT64/6to4/Teredo forms), port `0`, and embedded credentials.
- **Network Behavior:** The running DiDi application sends **zero telemetry, zero analytics, zero crash reports, and zero background HTTP requests**. External URLs open in your default browser only when you explicitly click **Discord Developer Portal**, **Project Documentation**, or **Report an Issue** (strictly allowlisted via `ExternalDocsTarget`). Separately, the NSIS installer uses `downloadBootstrapper` only during installation if Microsoft Edge WebView2 Runtime is missing.
- **Browser Extension Status:** DiDi has **no browser extension** and does **not** monitor browser tabs; all features operate exclusively via local Windows UI and Discord Desktop named-pipe IPC.
- **Reporting Security Issues:** Do not post security vulnerabilities or real credentials/tokens in public GitHub issues. Use GitHub's **Security -> Advisories -> Report a vulnerability** workflow on `https://github.com/RetsardCo/didi`.

---

## 17. Uninstallation

To uninstall an NSIS-installed copy of DiDi:

1. Close DiDi cleanly first via **System Tray -> Exit** or **Settings -> Exit DiDi Completely** (recommended so Discord activity clears before files are deleted).
2. Open **Windows Settings -> Apps -> Installed apps**, search for **DiDi**, and click **Uninstall** (or run `%LOCALAPPDATA%\DiDi\uninstall.exe`).

### What Uninstallation Removes vs. Preserves
- **Removed:**
  - `%LOCALAPPDATA%\DiDi\` (`didi.exe` and `uninstall.exe`).
  - Start Menu shortcut (`%APPDATA%\Microsoft\Windows\Start Menu\Programs\DiDi.lnk`).
  - Desktop shortcut (`%USERPROFILE%\Desktop\DiDi.lnk`).
  - Uninstall registry entry (`HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\DiDi`).
  - Autostart startup entry (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run\DiDi`).
- **Preserved:**
  - `%APPDATA%\DiDi\` (`config.json`, `config.json.bak`, and any `.corrupt.*` quarantine files) is **never deleted** by uninstallation, even if the installer's optional *"Delete app data"* checkbox is selected (that Tauri checkbox only targets `$APPDATA\app.didi.desktop` / `$LOCALAPPDATA\app.didi.desktop` WebView2 cache and `HKCU\Software\DiDi Contributors`).
  - **Note on Reinstalling After Uninstall:** Because uninstallation removes `HKCU\...\Run\DiDi`, launching DiDi after a later reinstall reconciles `launch_at_login` in `config.json` to `false` until you re-enable **Launch at Windows Startup** in **Settings** or the **Tray** menu.

### Manual Complete Data Removal (Optional)
If you want to wipe all saved DiDi profiles, Application IDs, and backups after uninstalling, delete `%APPDATA%\DiDi\` manually:

```powershell
Remove-Item -LiteralPath (Join-Path $env:APPDATA 'DiDi') -Recurse -Force
```

For the **portable executable**, delete your copied `didi.exe` (and remove `%APPDATA%\DiDi\` above if desired; also turn off **Launch at Windows Startup** beforehand if enabled).

---

## 18. Frequently Asked Questions

**Do I need Discord Developer Portal?**
Yes. DiDi uses a Bring-Your-Own Application ID model and requires a 17–20 digit Application ID created at `https://discord.com/developers/applications`.

**Do I need a Discord bot?**
No. You only create a Discord Application container and copy its **Application ID** from **General Information**. Do not configure a Bot user or Bot Token.

**Do I need a Discord Nitro subscription?**
No. Standard Discord Rich Presence via local IPC with your own Application ID works on standard Discord accounts without Nitro.

**Can DiDi work without Discord desktop?**
No. DiDi communicates over Windows local named pipes (`\\.\pipe\discord-ipc-{0..9}`) exposed by the Discord Desktop client. Discord Web in a browser and mobile-only clients do not host local Windows IPC pipes.

**Does DiDi work after closing Discord?**
DiDi continues running in the Windows system tray in `Reconnecting` / `Waiting for Discord` state and automatically resumes publishing once you reopen Discord Desktop. While Discord is closed, nothing appears on your Discord profile.

**Is DiDi portable?**
Yes. Both standalone `didi.exe` and installed builds are single binaries storing configuration in `%APPDATA%\DiDi\config.json` (and profile bundles can be moved between PCs via `.json` export/import). Note that standalone `didi.exe` still reads/writes `%APPDATA%\DiDi\`, not next to the `.exe`.

**Where are my profiles stored?**
All saved profiles, backups, and settings live locally at `%APPDATA%\DiDi\config.json` (`config.json.bak` for backups).

**Does DiDi collect my activity or send telemetry?**
No. DiDi stores data exclusively on your machine, discards Discord user objects from `READY` packets, enforces `connect-src 'none'` in its WebView CSP, and sends zero telemetry or remote logs.

**Is a browser extension required?**
No. There is no DiDi browser extension and none is needed.

---

## 19. License

DiDi is licensed under the **MIT License**. See [LICENSE](LICENSE) for full license terms.

### Disclaimer
DiDi is an independent open-source project and is not affiliated with, endorsed by, or sponsored by Discord Inc. Discord and the Discord logo are registered trademarks of Discord Inc.

---

## 20. Credits and Building from Source

### Contributors & Core Technologies
- **Contributors:** DiDi Contributors (`https://github.com/RetsardCo/didi`)
- **Core Stack:** Rust (`didi`), Tauri 2 (`tauri`, `tauri-plugin-autostart`, `tauri-plugin-dialog`, `tauri-plugin-opener`, `tauri-plugin-single-instance`), Tokio, `serde` / `serde_json`, `thiserror`, TypeScript, Vite, and NSIS (`makensis`) for Windows packaging.

### Local Build & Verification Commands

```powershell
# Install frontend dependencies
npm install

# Run full verification suite (TypeScript typecheck, frontend unit tests, rustfmt, clippy, cargo tests)
npm run check

# Run application in development mode
npm run tauri -- dev

# Build production frontend assets
npm run build

# Build standalone portable executable (src-tauri\target\release\didi.exe)
npm run tauri -- build --no-bundle

# Build standalone portable executable and NSIS installer (src-tauri\target\release\bundle\nsis\DiDi_0.1.0_x64-setup.exe)
npm run tauri -- build
```
