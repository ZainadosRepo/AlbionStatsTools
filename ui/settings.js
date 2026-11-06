// Settings schema and normalization for Albion Stats.

import { METER_MODES } from "./meter.js";

export const VALID_REGIONS = ["europe", "west", "east"];
export const VALID_COPY_STYLES = ["compact", "full"];
export const DEFAULT_CITIES = [
  "Caerleon",
  "Thetford",
  "Martlock",
  "Lymhurst",
  "Bridgewatch",
  "Fort Sterling",
  "Brecilien",
];

export const DEFAULT_SETTINGS = Object.freeze({
  region: "europe",
  meterMode: "damage",
  copyStyle: "compact",
  alwaysOnTop: false,
  compactOverlay: false,
  autoRefreshMs: 1000,
  selectedCities: [...DEFAULT_CITIES],
});

export const normalizeSettings = (raw) => {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
    return {
      ...DEFAULT_SETTINGS,
      selectedCities: [...DEFAULT_CITIES],
    };
  }

  const region = VALID_REGIONS.includes(raw.region)
    ? raw.region
    : DEFAULT_SETTINGS.region;
  const meterMode = METER_MODES.includes(raw.meterMode)
    ? raw.meterMode
    : DEFAULT_SETTINGS.meterMode;
  const copyStyle = VALID_COPY_STYLES.includes(raw.copyStyle)
    ? raw.copyStyle
    : DEFAULT_SETTINGS.copyStyle;
  const alwaysOnTop = Boolean(raw.alwaysOnTop);
  const compactOverlay = Boolean(raw.compactOverlay);

  const parsedRefresh = Number(raw.autoRefreshMs);
  const autoRefreshMs = Number.isFinite(parsedRefresh)
    ? Math.max(500, Math.min(5000, Math.round(parsedRefresh)))
    : DEFAULT_SETTINGS.autoRefreshMs;

  const selectedCities = Array.isArray(raw.selectedCities)
    ? raw.selectedCities
        .filter((c) => typeof c === "string" && DEFAULT_CITIES.includes(c))
        .filter((c, i, arr) => arr.indexOf(c) === i)
    : [...DEFAULT_CITIES];

  return {
    region,
    meterMode,
    copyStyle,
    alwaysOnTop,
    compactOverlay,
    autoRefreshMs,
    selectedCities: selectedCities.length > 0 ? selectedCities : [...DEFAULT_CITIES],
  };
};
