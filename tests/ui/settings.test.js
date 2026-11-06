import assert from "node:assert/strict";
import test from "node:test";
import { DEFAULT_SETTINGS, normalizeSettings } from "../../ui/settings.js";

test("normalizeSettings returns defaults on null or invalid root", () => {
  assert.deepEqual(normalizeSettings(null), DEFAULT_SETTINGS);
  assert.deepEqual(normalizeSettings([1, 2, 3]), DEFAULT_SETTINGS);
});

test("normalizeSettings clamps autoRefreshMs and filters invalid regions/modes/cities", () => {
  const normalized = normalizeSettings({
    region: "west",
    meterMode: "hps",
    copyStyle: "full",
    alwaysOnTop: 1,
    compactOverlay: true,
    autoRefreshMs: 120,
    selectedCities: ["Caerleon", "Atlantis", "Caerleon", "Brecilien"],
  });

  assert.equal(normalized.region, "west");
  assert.equal(normalized.meterMode, "hps");
  assert.equal(normalized.copyStyle, "full");
  assert.equal(normalized.alwaysOnTop, true);
  assert.equal(normalized.compactOverlay, true);
  assert.equal(normalized.autoRefreshMs, 500);
  assert.deepEqual(normalized.selectedCities, ["Caerleon", "Brecilien"]);
});
