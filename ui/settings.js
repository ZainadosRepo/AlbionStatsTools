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

export const DEFAULT_TRACKED_ITEMS = [
  "T7_2H_CLEAVER_HELL@1",
  "T8_MAIN_HOLYSTAFF_AVALON@2",
  "T8_2H_BOW_AVALON@2",
  "T8_2H_INFERNOSTAFF_MORGANA@2",
  "T8_BAG_INSIGHT",
];

export const DEFAULT_SETTINGS = Object.freeze({
  region: "europe",
  meterMode: "damage",
  copyStyle: "compact",
  compactMode: false,
  autoRefreshMs: 1000,
  selectedCities: [...DEFAULT_CITIES],
  trackedItems: [...DEFAULT_TRACKED_ITEMS],
});

export const normalizeSettings = (raw) => {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
    return {
      ...DEFAULT_SETTINGS,
      selectedCities: [...DEFAULT_CITIES],
      trackedItems: [...DEFAULT_TRACKED_ITEMS],
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
  const compactMode = Boolean(raw.compactMode ?? raw.compactOverlay);

  const parsedRefresh = raw.autoRefreshMs != null ? Number(raw.autoRefreshMs) : NaN;
  const autoRefreshMs = Number.isFinite(parsedRefresh)
    ? Math.max(500, Math.min(5000, Math.round(parsedRefresh)))
    : DEFAULT_SETTINGS.autoRefreshMs;

  const selectedCities = Array.isArray(raw.selectedCities)
    ? raw.selectedCities
        .filter((c) => typeof c === "string" && DEFAULT_CITIES.includes(c))
        .filter((c, i, arr) => arr.indexOf(c) === i)
    : [...DEFAULT_CITIES];

  const trackedItems = Array.isArray(raw.trackedItems)
    ? raw.trackedItems
        .filter((it) => typeof it === "string")
        .map((it) => it.trim().toUpperCase())
        .filter((it) => it.length > 0 && /^[A-Z0-9_@]+$/.test(it))
        .filter((it, i, arr) => arr.indexOf(it) === i)
        .slice(0, 40)
    : [...DEFAULT_TRACKED_ITEMS];

  return {
    region,
    meterMode,
    copyStyle,
    compactMode,
    autoRefreshMs,
    selectedCities: selectedCities.length > 0 ? selectedCities : [...DEFAULT_CITIES],
    trackedItems: trackedItems.length > 0 ? trackedItems : [...DEFAULT_TRACKED_ITEMS],
  };
};
