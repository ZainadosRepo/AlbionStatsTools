import assert from "node:assert/strict";
import test from "node:test";
import {
  buildAlbionDataPriceUrl,
  buildAlbionItemIconCandidates,
  buildAlbionItemIconUrl,
  buildAlbionSpellIconCandidates,
  buildAlbionSpellIconUrl,
  collectSessionPriceableItems,
  computeRecipeMaterialCosts,
  computeSessionMarketValue,
  estimateCraftingDefaults,
  formatDuration,
  formatExactInt,
  formatMeterClipboard,
  formatNumber,
  getCityBadgeInfo,
  parseItemTierAndEnchantment,
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

test("buildAlbionItemIconUrl builds official render URLs with literal @ and fallback candidates", () => {
  assert.equal(
    buildAlbionItemIconUrl("T8_2H_BOW_AVALON@2", 48),
    "https://render.albiononline.com/v1/item/T8_2H_BOW_AVALON@2.png?size=48",
  );
  const candidates = buildAlbionItemIconCandidates("T8_2H_BOW_AVALON@2", 128);
  assert.equal(candidates[0], "https://render.albiononline.com/v1/item/T8_2H_BOW_AVALON@2.png?size=128");
  assert.ok(candidates.includes("https://render.albiononline.com/v1/item/T8_2H_BOW_AVALON.png"));
  assert.equal(buildAlbionItemIconUrl("UNARMED"), null);
  assert.equal(buildAlbionItemIconUrl("SILVER"), null);
  assert.equal(buildAlbionItemIconUrl("ITEM_9999"), null);
  assert.equal(buildAlbionItemIconUrl(""), null);
});

test("buildAlbionSpellIconUrl and buildAlbionSpellIconCandidates prioritize uisprite and locale=en", () => {
  assert.equal(
    buildAlbionSpellIconUrl("HOLYSTAFF_HEAL_BURST", 32),
    "https://render.albiononline.com/v1/spell/HOLYSTAFF_HEAL_BURST.png?size=32",
  );
  assert.equal(buildAlbionSpellIconUrl("AUTO_ATTACK"), null);
  assert.equal(buildAlbionSpellIconUrl("SPELL_4521"), null);
  assert.equal(buildAlbionSpellIconUrl(""), null);
  assert.equal(buildAlbionSpellIconUrl(null), null);

  const spellUrls = buildAlbionSpellIconCandidates(
    {
      uniqueName: "CHAIN_LIGHTNING_CAPE",
      nameEn: "Chain Lightning (Thetford)",
      uiSprite: "CAPE_THETFORD_LIGHTNING",
    },
    64,
  );
  assert.equal(spellUrls[0], "https://render.albiononline.com/v1/spell/CAPE_THETFORD_LIGHTNING.png?size=64");
  assert.ok(spellUrls.includes("https://render.albiononline.com/v1/spell/Chain%20Lightning.png?locale=en&size=64"));
  assert.deepEqual(buildAlbionSpellIconCandidates({ uniqueName: "AUTO_ATTACK", nameEn: "Auto Attack" }), []);
});

test("estimateCraftingDefaults separates artefact items from standard items", () => {
  const artefact = estimateCraftingDefaults("T8_MAIN_NATURESTAFF_KEEPER@1", 1000000, 2560);
  assert.equal(artefact.isArtefact, true);
  assert.ok(artefact.artefactCost > 0);
  assert.ok(artefact.returnableCost > 0);

  const standard = estimateCraftingDefaults("T8_BAG", 200000, 2400);
  assert.equal(standard.isArtefact, false);
  assert.equal(standard.artefactCost, 0);
  assert.ok(standard.returnableCost > 0);
});

test("computeRecipeMaterialCosts computes exact live returnable and artefact costs per crafted unit", () => {
  const bowRecipe = {
    itemUniqueName: "T8_2H_BOW_KEEPER@2",
    amountCrafted: 1,
    craftingFocus: 49260,
    itemValue: 32768,
    ingredients: [
      { uniqueName: "T8_PLANKS_LEVEL2@2", nameFr: "Planches d'épicéa rares", count: 32, isReturnable: true },
      { uniqueName: "T8_ARTEFACT_2H_BOW_KEEPER", nameFr: "Os sculpté du sage", count: 1, isReturnable: false },
    ],
  };
  const bowEval = computeRecipeMaterialCosts(bowRecipe, [
    { itemId: "T8_PLANKS_LEVEL2@2", bestSellPrice: 245000, bestSellCity: "Bridgewatch", bestBuyOrder: 199000, bestBuyCity: "Fort Sterling" },
    { itemId: "T8_ARTEFACT_2H_BOW_KEEPER", bestSellPrice: 600000, bestSellCity: "Fort Sterling", bestBuyOrder: 0, bestBuyCity: "-" },
  ]);
  assert.equal(bowEval.returnablePerUnit, 32 * 245000);
  assert.equal(bowEval.nonReturnablePerUnit, 600000);
  assert.equal(bowEval.pricedCount, 2);
  assert.equal(bowEval.missingCount, 0);

  const stewRecipe = {
    itemUniqueName: "T8_MEAL_STEW@1",
    amountCrafted: 10,
    craftingFocus: 752,
    itemValue: 576,
    ingredients: [
      { uniqueName: "T8_PUMPKIN", nameFr: "Citrouille", count: 36, isReturnable: true },
      { uniqueName: "T1_FISHSAUCE_LEVEL1", nameFr: "Sauce de poisson basique", count: 90, isReturnable: false },
    ],
  };
  const stewEval = computeRecipeMaterialCosts(stewRecipe, [
    { itemId: "T8_PUMPKIN", bestSellPrice: 500, bestSellCity: "Caerleon", bestBuyOrder: 400, bestBuyCity: "Caerleon" },
    { itemId: "T1_FISHSAUCE_LEVEL1", bestSellPrice: 4000, bestSellCity: "Lymhurst", bestBuyOrder: 3600, bestBuyCity: "Lymhurst" },
  ]);
  assert.equal(stewEval.returnablePerUnit, (36 * 500) / 10);
  assert.equal(stewEval.nonReturnablePerUnit, (90 * 4000) / 10);
});

test("collectSessionPriceableItems aggregates loot and gathering quantities", () => {
  const { items, qtyByItem } = collectSessionPriceableItems({
    lootLog: [
      { uniqueName: "T8_BAG_INSIGHT", quantity: 1 },
      { uniqueName: "SILVER", quantity: 12000 },
      { uniqueName: "ITEM_4242", quantity: 3 },
      { uniqueName: "T8_BAG_INSIGHT", quantity: 2 },
    ],
    gatheringLog: [
      { uniqueName: "T6_WOOD_LEVEL1@1", totalAmount: 14 },
      { uniqueName: "T6_WOOD_LEVEL1@1", totalAmount: 6 },
      { uniqueName: "T5_ORE", totalAmount: 0 },
    ],
  });
  assert.deepEqual(items, ["T8_BAG_INSIGHT", "T6_WOOD_LEVEL1@1"]);
  assert.equal(qtyByItem.T8_BAG_INSIGHT, 3);
  assert.equal(qtyByItem["T6_WOOD_LEVEL1@1"], 20);
  assert.deepEqual(collectSessionPriceableItems(null), { items: [], qtyByItem: {} });

  const value = computeSessionMarketValue(
    [
      { itemId: "T8_BAG_INSIGHT", bestSellPrice: 100000, bestBuyOrder: 90000 },
      { itemId: "T6_WOOD_LEVEL1@1", bestSellPrice: 0, bestBuyOrder: 500 },
      { itemId: "T4_UNKNOWN", bestSellPrice: 999, bestBuyOrder: 0 },
    ],
    qtyByItem,
  );
  assert.equal(value.total, 310000);
  assert.equal(value.pricedItems, 2);
});

test("parseItemTierAndEnchantment and getCityBadgeInfo provide accurate Albion UI metadata", () => {
  assert.deepEqual(parseItemTierAndEnchantment("T7_2H_CLEAVER_HELL@1"), { tier: 7, enchantment: 1, label: "T7.1" });
  assert.deepEqual(parseItemTierAndEnchantment("T8_BAG_INSIGHT"), { tier: 8, enchantment: 0, label: "T8" });
  assert.deepEqual(parseItemTierAndEnchantment("NON_TIER_ITEM"), { tier: null, enchantment: 0, label: "" });

  const martlock = getCityBadgeInfo("Martlock");
  assert.equal(martlock.key, "martlock");
  assert.equal(martlock.icon, "🛡️");
  assert.equal(martlock.label, "Martlock");

  const fallback = getCityBadgeInfo("Unknown Realm");
  assert.equal(fallback.key, "default");
  assert.equal(fallback.label, "Unknown Realm");
  assert.equal(getCityBadgeInfo("-"), null);
});
