import assert from "node:assert/strict";
import test from "node:test";
import {
  buildAlbionDataPriceUrl,
  formatDuration,
  formatExactInt,
  formatMeterClipboard,
  formatNumber,
  rankMeterPlayers,
  summarizeMarketPrices,
} from "../../ui/meter.js";

const SAMPLE_PLAYERS = [
  {
    name: "Kaelen",
    weaponUniqueName: "T8_2H_BOW_AVALON@2",
    weaponNameFr: "Brumisateur T8.2",
    damage: 2150,
    dps: 134.4,
    heal: 0,
    hps: 0,
    overhealed: 0,
    overhealPercent: 0,
    takenDamage: 120,
  },
  {
    name: "Aurelia",
    weaponUniqueName: "T8_2H_INFERNOSTAFF_MORGANA@2",
    weaponNameFr: "Bâton ardent T8.2",
    damage: 5390,
    dps: 336.9,
    heal: 0,
    hps: 0,
    overhealed: 0,
    overhealPercent: 0,
    takenDamage: 640,
  },
  {
    name: "Seraphina",
    weaponUniqueName: "T8_MAIN_HOLYSTAFF_AVALON@2",
    weaponNameFr: "Sanctificateur T8.2",
    damage: 0,
    dps: 0,
    heal: 1420,
    hps: 88.8,
    overhealed: 520,
    overhealPercent: 26.8,
    takenDamage: 0,
  },
];

test("formatNumber and formatDuration format compact French metrics", () => {
  assert.equal(formatNumber(5390), "5390");
  assert.equal(formatNumber(18450), "18,5 k");
  assert.equal(formatNumber(2450000), "2,45 M");
  assert.equal(formatExactInt(445000), "445 000");
  assert.equal(formatDuration(16), "16s");
  assert.equal(formatDuration(138), "02m 18s");
  assert.equal(formatDuration(3725), "1h 02m 05s");
});

test("rankMeterPlayers orders by active mode and computes relative bar and share percentages", () => {
  const byDamage = rankMeterPlayers(SAMPLE_PLAYERS, "damage");
  assert.equal(byDamage[0].name, "Aurelia");
  assert.equal(byDamage[0].rank, 1);
  assert.equal(byDamage[0].barPercent, 100);
  assert.ok(byDamage[0].sharePercent > 70);
  assert.equal(byDamage[1].name, "Kaelen");

  const byHeal = rankMeterPlayers(SAMPLE_PLAYERS, "heal");
  assert.equal(byHeal[0].name, "Seraphina");
  assert.equal(byHeal[0].barPercent, 100);
  assert.ok(byHeal[0].secondaryLabel.includes("OH 27%"));
});

test("formatMeterClipboard supports compact and full party chat formats", () => {
  const compact = formatMeterClipboard(SAMPLE_PLAYERS, "damage", "compact", "Brumes T8", 16);
  assert.ok(compact.startsWith("[Albion Stats • Dégâts • Brumes T8 • 16s]"));
  assert.ok(compact.includes("1. Aurelia"));
  assert.ok(compact.includes("2. Kaelen"));

  const full = formatMeterClipboard(SAMPLE_PLAYERS, "heal", "full", "Brumes T8", 16);
  assert.ok(full.includes("Seraphina [Sanctificateur T8.2]"));
});

test("buildAlbionDataPriceUrl and summarizeMarketPrices compute arbitrage spreads", () => {
  const url = buildAlbionDataPriceUrl("europe", ["T7_2H_CLEAVER_HELL@1"], ["Caerleon", "Thetford"], [2, 3]);
  assert.equal(
    url,
    "https://europe.albion-online-data.com/api/v2/stats/prices/T7_2H_CLEAVER_HELL@1.json?locations=Caerleon%2CThetford&qualities=2,3",
  );

  const summary = summarizeMarketPrices([
    {
      item_id: "T7_2H_CLEAVER_HELL@1",
      city: "Thetford",
      quality: 2,
      sell_price_min: 410000,
      buy_price_max: 390000,
    },
    {
      item_id: "T7_2H_CLEAVER_HELL@1",
      city: "Caerleon",
      quality: 3,
      sell_price_min: 465000,
      buy_price_max: 445000,
    },
  ]);
  assert.equal(summary.length, 1);
  assert.equal(summary[0].bestSellPrice, 410000);
  assert.equal(summary[0].bestSellCity, "Thetford");
  assert.equal(summary[0].bestBuyOrder, 445000);
  assert.equal(summary[0].bestBuyCity, "Caerleon");
  assert.equal(summary[0].spreadSilver, 35000);
});
