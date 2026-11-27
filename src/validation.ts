import type { ActivityButton, ArtworkRef, ProfileV1 } from "./types";

export const MAX_PROFILES_COUNT = 50;
export const MAX_PROFILE_ID_LEN = 64;
export const MAX_PROFILE_NAME_LEN = 64;
export const MIN_TEXT_LEN = 2;
export const MAX_TEXT_LEN = 128;
export const MAX_URL_LEN = 256;
export const MAX_BUTTON_URL_LEN = 512;
export const MAX_BUTTON_LABEL_LEN = 32;
export const MAX_PORTAL_KEY_LEN = 300;
export const MAX_BUTTONS_COUNT = 2;
export const MAX_UNIX_TIMESTAMP_SECS = 32_503_680_000;

export function hasControlChars(value: string): boolean {
  for (let i = 0; i < value.length; i++) {
    const code = value.charCodeAt(i);
    if (code <= 0x1f || (code >= 0x7f && code <= 0x9f)) {
      return true;
    }
  }
  return false;
}

export function normalizeOptionalText(raw: string): string | null {
  const trimmed = raw.trim();
  return trimmed.length === 0 ? null : trimmed;
}

export function maskApplicationId(raw: string | null | undefined): string {
  if (!raw) {
    return "Not configured";
  }
  const trimmed = raw.trim();
  if (trimmed.length === 0) {
    return "Not configured";
  }
  const suffix = trimmed.slice(Math.max(0, trimmed.length - 4));
  return `****${suffix}`;
}

export function validateApplicationId(raw: string): string | null {
  if (hasControlChars(raw)) {
    return "Application ID must not contain control characters.";
  }
  const trimmed = raw.trim();
  if (trimmed.length === 0) {
    return "Application ID cannot be empty.";
  }
  if (trimmed.length < 17 || trimmed.length > 20 || !/^\d+$/.test(trimmed)) {
    return "Application ID must be 17 to 20 ASCII digits.";
  }
  if (/^0+$/.test(trimmed)) {
    return "Application ID must be a non-zero Discord snowflake.";
  }
  return null;
}

export function validateProfileId(id: string): string | null {
  if (hasControlChars(id)) {
    return "Profile ID must not contain control characters.";
  }
  const trimmed = id.trim();
  if (trimmed.length === 0 || trimmed.length > MAX_PROFILE_ID_LEN) {
    return `Profile ID must be 1 to ${MAX_PROFILE_ID_LEN} characters.`;
  }
  if (!/^[a-z0-9_-]+$/.test(trimmed)) {
    return "Profile ID may use only lowercase letters, numbers, '-' or '_'.";
  }
  return null;
}

export function validateProfileName(name: string): string | null {
  if (hasControlChars(name)) {
    return "Profile name must not contain control characters or line breaks.";
  }
  const trimmed = name.trim();
  const len = Array.from(trimmed).length;
  if (len === 0) {
    return "Profile name is required.";
  }
  if (len > MAX_PROFILE_NAME_LEN) {
    return `Profile name must be ${MAX_PROFILE_NAME_LEN} characters or fewer.`;
  }
  return null;
}

export function validateOptionalTextField(label: string, raw: string): string | null {
  if (hasControlChars(raw)) {
    return `${label} must be single-line with no control characters.`;
  }
  const trimmed = raw.trim();
  if (trimmed.length === 0) {
    return null;
  }
  const len = Array.from(trimmed).length;
  if (len < MIN_TEXT_LEN || len > MAX_TEXT_LEN) {
    return `${label} must be between ${MIN_TEXT_LEN} and ${MAX_TEXT_LEN} characters.`;
  }
  return null;
}

function isValidPort(portStr: string): boolean {
  if (portStr.length === 0 || !/^\d+$/.test(portStr)) {
    return false;
  }
  const port = Number(portStr);
  return Number.isInteger(port) && port >= 1 && port <= 65535;
}

function isDisallowedIpv4Octets(a: number, b: number, c: number, _d: number): boolean {
  if (a === 0) return true; // 0.0.0.0/8
  if (a === 127) return true; // 127.0.0.0/8
  if (a === 10) return true; // 10.0.0.0/8
  if (a === 100 && b >= 64 && b <= 127) return true; // 100.64.0.0/10
  if (a === 172 && b >= 16 && b <= 31) return true; // 172.16.0.0/12
  if (a === 192 && b === 0 && c === 0) return true; // 192.0.0.0/24
  if (a === 192 && b === 0 && c === 2) return true; // 192.0.2.0/24
  if (a === 192 && b === 168) return true; // 192.168.0.0/16
  if (a === 169 && b === 254) return true; // 169.254.0.0/16
  if (a === 198 && b === 51 && c === 100) return true; // 198.51.100.0/24
  if (a === 203 && b === 0 && c === 113) return true; // 203.0.113.0/24
  if (a >= 224) return true; // multicast & reserved
  return false;
}

function tryParseIpv4(host: string): [number, number, number, number] | null {
  const parts = host.split(".");
  if (parts.length !== 4) {
    return null;
  }
  const nums: number[] = [];
  for (const part of parts) {
    if (!/^\d{1,3}$/.test(part)) {
      return null;
    }
    if (part.length > 1 && part.startsWith("0")) {
      return [-1, -1, -1, -1]; // Signal invalid leading-zero octal form
    }
    const n = Number(part);
    if (n > 255) {
      return null;
    }
    nums.push(n);
  }
  return [nums[0], nums[1], nums[2], nums[3]];
}

function tryParseIpv6Segments(host: string): number[] | null {
  if (!host.includes(":")) {
    return null;
  }
  let working = host;
  // Handle embedded IPv4 suffix like ::ffff:127.0.0.1
  const lastColon = working.lastIndexOf(":");
  const maybeIpv4 = working.slice(lastColon + 1);
  if (maybeIpv4.includes(".")) {
    const octets = tryParseIpv4(maybeIpv4);
    if (!octets || octets[0] < 0) {
      return null;
    }
    const hi = ((octets[0] << 8) | octets[1]).toString(16);
    const lo = ((octets[2] << 8) | octets[3]).toString(16);
    working = `${working.slice(0, lastColon + 1)}${hi}:${lo}`;
  }

  const doubleColonCount = (working.match(/::/g) ?? []).length;
  if (doubleColonCount > 1) {
    return null;
  }
  const [leftRaw, rightRaw] = doubleColonCount === 1 ? working.split("::") : [working, ""];
  const left = leftRaw ? leftRaw.split(":") : [];
  const right = doubleColonCount === 1 && rightRaw ? rightRaw.split(":") : [];

  const totalExplicit = left.length + right.length;
  if (doubleColonCount === 0 && totalExplicit !== 8) {
    return null;
  }
  if (doubleColonCount === 1 && totalExplicit >= 8) {
    return null;
  }

  const parseGroup = (g: string): number | null => {
    if (!/^[0-9a-fA-F]{1,4}$/.test(g)) return null;
    return Number.parseInt(g, 16);
  };

  const segments: number[] = [];
  for (const g of left) {
    const val = parseGroup(g);
    if (val === null) return null;
    segments.push(val);
  }
  if (doubleColonCount === 1) {
    const missing = 8 - totalExplicit;
    for (let i = 0; i < missing; i++) {
      segments.push(0);
    }
  }
  for (const g of right) {
    const val = parseGroup(g);
    if (val === null) return null;
    segments.push(val);
  }
  return segments.length === 8 ? segments : null;
}

function isDisallowedIpv6Segments(s: number[]): boolean {
  const isAllZero = s.every((v) => v === 0);
  if (isAllZero) return true; // ::
  if (s.slice(0, 7).every((v) => v === 0) && s[7] === 1) return true; // ::1
  if ((s[0] & 0xff00) === 0xff00) return true; // ff00::/8 multicast
  if ((s[0] & 0xffc0) === 0xfe80) return true; // fe80::/10 link-local
  if ((s[0] & 0xfe00) === 0xfc00) return true; // fc00::/7 unique local
  if (s[0] === 0x2001 && s[1] === 0x0db8) return true; // 2001:db8::/32 doc
  if (s[0] === 0x0100 && s[1] === 0 && s[2] === 0 && s[3] === 0) return true; // 100::/64 discard

  // IPv4-mapped / IPv4-compatible
  if (s[0] === 0 && s[1] === 0 && s[2] === 0 && s[3] === 0 && s[4] === 0 && (s[5] === 0 || s[5] === 0xffff)) {
    if (isDisallowedIpv4Octets(s[6] >> 8, s[6] & 0xff, s[7] >> 8, s[7] & 0xff)) {
      return true;
    }
  }
  // NAT64 local-use & well-known
  if (s[0] === 0x0064 && s[1] === 0xff9b && s[2] === 0x0001) return true;
  if (s[0] === 0x0064 && s[1] === 0xff9b && s[2] === 0 && s[3] === 0 && s[4] === 0 && s[5] === 0) {
    if (isDisallowedIpv4Octets(s[6] >> 8, s[6] & 0xff, s[7] >> 8, s[7] & 0xff)) {
      return true;
    }
  }
  // 6to4
  if (s[0] === 0x2002) {
    if (isDisallowedIpv4Octets(s[1] >> 8, s[1] & 0xff, s[2] >> 8, s[2] & 0xff)) {
      return true;
    }
  }
  // Teredo
  if (s[0] === 0x2001 && s[1] === 0x0000) return true;
  return false;
}

export function validateHttpsUrl(label: string, raw: string, maxLen: number): string | null {
  if (raw.length === 0) {
    return null;
  }
  if (hasControlChars(raw) || /\s/.test(raw)) {
    return `${label} must not contain spaces or control characters.`;
  }
  const len = Array.from(raw).length;
  if (len > maxLen) {
    return `${label} must be ${maxLen} characters or fewer.`;
  }
  if (!raw.toLowerCase().startsWith("https://")) {
    return `${label} must start with https://.`;
  }
  if (raw.includes("\\")) {
    return `${label} must not contain backslashes.`;
  }

  const remainder = raw.slice(8);
  const authEndMatch = remainder.search(/[/?#]/);
  const authority = authEndMatch === -1 ? remainder : remainder.slice(0, authEndMatch);

  if (authority.includes("@")) {
    return `${label} must not embed username or password credentials.`;
  }

  let hostPart = authority;
  if (authority.startsWith("[")) {
    const bracketEnd = authority.indexOf("]");
    if (bracketEnd === -1) {
      return `${label} has an invalid IPv6 host.`;
    }
    const afterBracket = authority.slice(bracketEnd + 1);
    if (afterBracket.length > 0) {
      if (!afterBracket.startsWith(":") || !isValidPort(afterBracket.slice(1))) {
        return `${label} has an invalid port.`;
      }
    }
    hostPart = authority.slice(1, bracketEnd);
  } else {
    const colonIdx = authority.indexOf(":");
    if (colonIdx !== -1) {
      const portStr = authority.slice(colonIdx + 1);
      if (!isValidPort(portStr)) {
        return `${label} has an invalid port.`;
      }
      hostPart = authority.slice(0, colonIdx);
    }
  }

  const hostLower = hostPart.toLowerCase();
  if (hostLower.length === 0) {
    return `${label} must include a public host name.`;
  }

  const ipv4 = tryParseIpv4(hostLower);
  if (ipv4) {
    if (ipv4[0] < 0 || isDisallowedIpv4Octets(ipv4[0], ipv4[1], ipv4[2], ipv4[3])) {
      return `${label} cannot target private, loopback, or local IPv4 addresses.`;
    }
    return null;
  }

  if (authority.startsWith("[")) {
    const ipv6 = tryParseIpv6Segments(hostLower);
    if (!ipv6 || isDisallowedIpv6Segments(ipv6)) {
      return `${label} cannot target private, loopback, or invalid IPv6 addresses.`;
    }
    return null;
  }

  if (
    hostLower === "localhost" ||
    hostLower.endsWith(".localhost") ||
    hostLower.endsWith(".local") ||
    hostLower.endsWith(".internal") ||
    hostLower.endsWith(".lan")
  ) {
    return `${label} cannot target local or internal hosts.`;
  }

  const labels = hostLower.split(".");
  if (labels.length < 2 || labels.some((part) => part.length === 0)) {
    return `${label} must use a public domain name (e.g., example.com).`;
  }
  const tld = labels[labels.length - 1];
  if (/^\d+$/.test(tld)) {
    return `${label} has an invalid numeric top-level domain.`;
  }
  for (const part of labels) {
    if (part.startsWith("-") || part.endsWith("-") || !/^[a-z0-9-]+$/.test(part)) {
      return `${label} contains invalid hostname characters.`;
    }
  }

  return null;
}

export function validatePortalKey(raw: string): string | null {
  if (hasControlChars(raw)) {
    return "Asset key must not contain control characters.";
  }
  const trimmed = raw.trim().toLowerCase();
  if (trimmed.length === 0 || trimmed.length > MAX_PORTAL_KEY_LEN) {
    return `Asset key must be 1 to ${MAX_PORTAL_KEY_LEN} characters.`;
  }
  if (!/^[a-z0-9_-]+$/.test(trimmed)) {
    return "Asset key may use only letters, numbers, '-' or '_' (auto-lowercased).";
  }
  return null;
}

export function validateArtworkRef(label: string, asset: ArtworkRef): string | null {
  if (asset.kind === "none") {
    return null;
  }
  if (asset.kind === "portal_key") {
    return validatePortalKey(asset.key);
  }
  if (asset.url.trim().length === 0) {
    return `${label} image URL cannot be empty.`;
  }
  return validateHttpsUrl(`${label} image URL`, asset.url, MAX_PORTAL_KEY_LEN);
}

export function validateButton(button: ActivityButton, index: number): {
  labelError: string | null;
  urlError: string | null;
} {
  let labelError: string | null = null;
  if (hasControlChars(button.label)) {
    labelError = `Button ${index + 1} label must not contain control characters.`;
  } else {
    const trimmed = button.label.trim();
    const len = Array.from(trimmed).length;
    if (len === 0) {
      labelError = `Button ${index + 1} label is required.`;
    } else if (len > MAX_BUTTON_LABEL_LEN) {
      labelError = `Button ${index + 1} label must be ${MAX_BUTTON_LABEL_LEN} characters or fewer.`;
    }
  }

  let urlError: string | null = null;
  if (button.url.trim().length === 0) {
    urlError = `Button ${index + 1} URL is required.`;
  } else {
    urlError = validateHttpsUrl(`Button ${index + 1} URL`, button.url, MAX_BUTTON_URL_LEN);
  }

  return { labelError, urlError };
}

export interface ProfileValidationMap {
  name?: string;
  application_id_override?: string;
  activity_name_override?: string;
  details?: string;
  details_url?: string;
  state?: string;
  state_url?: string;
  large_asset?: string;
  large_text?: string;
  large_url?: string;
  small_asset?: string;
  small_text?: string;
  small_url?: string;
  custom_timestamp_unix?: string;
  buttons: Array<{ label?: string; url?: string }>;
}

export function validateProfileDraft(draft: ProfileV1): {
  valid: boolean;
  errors: ProfileValidationMap;
} {
  const errors: ProfileValidationMap = { buttons: [] };
  let valid = true;

  const setErr = (key: Exclude<keyof ProfileValidationMap, "buttons">, msg: string | null) => {
    if (msg) {
      errors[key] = msg;
      valid = false;
    }
  };

  setErr("name", validateProfileName(draft.name));

  if (draft.application_id_override !== null && draft.application_id_override.trim().length > 0) {
    setErr("application_id_override", validateApplicationId(draft.application_id_override));
  } else if (draft.application_id_override !== null && hasControlChars(draft.application_id_override)) {
    setErr("application_id_override", "Application ID must not contain control characters.");
  }

  if (draft.activity_name_override !== null) {
    setErr(
      "activity_name_override",
      validateOptionalTextField("Activity Name Override", draft.activity_name_override),
    );
  }
  if (draft.details !== null) {
    setErr("details", validateOptionalTextField("Details", draft.details));
  }
  if (draft.details_url !== null) {
    setErr("details_url", validateHttpsUrl("Details URL", draft.details_url, MAX_URL_LEN));
  }
  if (draft.state !== null) {
    setErr("state", validateOptionalTextField("State", draft.state));
  }
  if (draft.state_url !== null) {
    setErr("state_url", validateHttpsUrl("State URL", draft.state_url, MAX_URL_LEN));
  }

  setErr("large_asset", validateArtworkRef("Large Asset", draft.large_asset));
  if (draft.large_text !== null) {
    setErr("large_text", validateOptionalTextField("Large Image Tooltip", draft.large_text));
  }
  if (draft.large_url !== null) {
    setErr("large_url", validateHttpsUrl("Large Image URL", draft.large_url, MAX_URL_LEN));
  }

  setErr("small_asset", validateArtworkRef("Small Asset", draft.small_asset));
  if (draft.small_text !== null) {
    setErr("small_text", validateOptionalTextField("Small Image Tooltip", draft.small_text));
  }
  if (draft.small_url !== null) {
    setErr("small_url", validateHttpsUrl("Small Image URL", draft.small_url, MAX_URL_LEN));
  }

  if (draft.timestamp_mode === "countdown_until") {
    if (draft.custom_timestamp_unix === null) {
      setErr("custom_timestamp_unix", "Countdown mode requires a target date and time.");
    } else if (
      !Number.isInteger(draft.custom_timestamp_unix) ||
      draft.custom_timestamp_unix <= 0 ||
      draft.custom_timestamp_unix > MAX_UNIX_TIMESTAMP_SECS
    ) {
      setErr(
        "custom_timestamp_unix",
        "Countdown timestamp must be a valid Unix timestamp in seconds.",
      );
    }
  } else if (draft.custom_timestamp_unix !== null) {
    if (
      !Number.isInteger(draft.custom_timestamp_unix) ||
      draft.custom_timestamp_unix <= 0 ||
      draft.custom_timestamp_unix > MAX_UNIX_TIMESTAMP_SECS
    ) {
      setErr("custom_timestamp_unix", "Custom timestamp must be valid Unix seconds.");
    }
  }

  for (let i = 0; i < draft.buttons.length; i++) {
    const res = validateButton(draft.buttons[i], i);
    const entry: { label?: string; url?: string } = {};
    if (res.labelError) {
      entry.label = res.labelError;
      valid = false;
    }
    if (res.urlError) {
      entry.url = res.urlError;
      valid = false;
    }
    errors.buttons.push(entry);
  }

  return { valid, errors };
}

export function normalizeProfileForSave(draft: ProfileV1): ProfileV1 {
  const normalizeArtwork = (asset: ArtworkRef): ArtworkRef => {
    if (asset.kind === "portal_key") {
      return { kind: "portal_key", key: asset.key.trim().toLowerCase() };
    }
    if (asset.kind === "external_url") {
      return { kind: "external_url", url: asset.url.trim() };
    }
    return { kind: "none" };
  };

  return {
    id: draft.id.trim(),
    name: draft.name.trim(),
    application_id_override: normalizeOptionalText(draft.application_id_override ?? ""),
    activity_name_override: normalizeOptionalText(draft.activity_name_override ?? ""),
    activity_type: draft.activity_type,
    status_display_type: draft.status_display_type,
    details: normalizeOptionalText(draft.details ?? ""),
    details_url: normalizeOptionalText(draft.details_url ?? ""),
    state: normalizeOptionalText(draft.state ?? ""),
    state_url: normalizeOptionalText(draft.state_url ?? ""),
    large_asset: normalizeArtwork(draft.large_asset),
    large_text: normalizeOptionalText(draft.large_text ?? ""),
    large_url: normalizeOptionalText(draft.large_url ?? ""),
    small_asset: normalizeArtwork(draft.small_asset),
    small_text: normalizeOptionalText(draft.small_text ?? ""),
    small_url: normalizeOptionalText(draft.small_url ?? ""),
    timestamp_mode: draft.timestamp_mode,
    custom_timestamp_unix:
      draft.timestamp_mode === "countdown_until" ? draft.custom_timestamp_unix : null,
    buttons: draft.buttons.map((b) => ({
      label: b.label.trim(),
      url: b.url.trim(),
    })),
  };
}

export function generateUniqueProfileSlug(baseName: string, existingIds: readonly string[]): string {
  const used = new Set(existingIds);
  const rawSlug = baseName
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 56);
  const base = rawSlug.length > 0 ? rawSlug : "profile";
  if (!used.has(base)) {
    return base;
  }
  for (let i = 2; i < 1000; i++) {
    const suffix = `-${i}`;
    const candidate = `${base.slice(0, MAX_PROFILE_ID_LEN - suffix.length)}${suffix}`;
    if (!used.has(candidate)) {
      return candidate;
    }
  }
  return `profile-${Date.now().toString(36)}`;
}

export function extractHostnameSafe(httpsUrl: string): string {
  const withoutScheme = httpsUrl.slice(8);
  const end = withoutScheme.search(/[/?#]/);
  return end === -1 ? withoutScheme : withoutScheme.slice(0, end);
}

export function formatDurationClock(totalSeconds: number): string {
  const clamped = Math.max(0, Math.floor(totalSeconds));
  const hours = Math.floor(clamped / 3600);
  const minutes = Math.floor((clamped % 3600) / 60);
  const seconds = clamped % 60;
  const mm = String(minutes).padStart(2, "0");
  const ss = String(seconds).padStart(2, "0");
  if (hours > 0) {
    return `${String(hours).padStart(2, "0")}:${mm}:${ss}`;
  }
  return `${mm}:${ss}`;
}
