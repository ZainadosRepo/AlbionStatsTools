// Pure formatting, meter ranking, clipboard export, and Albion Data Project price helpers.

export const METER_MODES = ["damage", "dps", "heal", "hps", "overheal", "taken"];

export const formatNumber = (value, decimals = 0) => {
  const num = Number(value);
  if (!Number.isFinite(num)) return "0";
  const abs = Math.abs(num);
  if (abs >= 1_000_000) {
    return `${(Math.round((num / 1_000_000) * 100) / 100).toFixed(2).replace(".", ",")} M`;
  }
  if (abs >= 10_000) {
    return `${(Math.round((num / 1_000) * 10) / 10).toFixed(1).replace(".", ",")} k`;
  }
  return num.toFixed(decimals).replace(".", ",");
};

export const formatExactInt = (value) => {
  const num = Math.round(Number(value) || 0);
  return num.toString().replace(/\B(?=(\d{3})+(?!\d))/g, " ");
};

export const formatDuration = (seconds) => {
  const total = Math.max(0, Math.floor(Number(seconds) || 0));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  if (h > 0) {
    return `${h}h ${String(m).padStart(2, "0")}m ${String(s).padStart(2, "0")}s`;
  }
  if (m > 0) {
    return `${String(m).padStart(2, "0")}m ${String(s).padStart(2, "0")}s`;
  }
  return `${s}s`;
};

const metricValueForMode = (player, mode) => {
  switch (mode) {
    case "dps":
      return Number(player.dps) || 0;
    case "heal":
      return Number(player.heal) || 0;
    case "hps":
      return Number(player.hps) || 0;
    case "overheal":
      return Number(player.overhealed) || 0;
    case "taken":
      return Number(player.takenDamage) || 0;
    case "damage":
    default:
      return Number(player.damage) || 0;
  }
};

export const rankMeterPlayers = (players, mode = "damage") => {
  if (!Array.isArray(players) || players.length === 0) return [];
  const activeMode = METER_MODES.includes(mode) ? mode : "damage";

  const cloned = players.map((p) => {
    const val = metricValueForMode(p, activeMode);
    return { ...p, _sortVal: val };
  });

  cloned.sort((a, b) => b._sortVal - a._sortVal || (b.damage || 0) - (a.damage || 0));
  const topVal = Math.max(1, cloned[0]._sortVal || 0);
  const sumVal = cloned.reduce((acc, p) => acc + p._sortVal, 0);

  return cloned.map((p, index) => {
    const barPercent = p._sortVal > 0 ? Math.max(3, Math.min(100, (p._sortVal / topVal) * 100)) : 0;
    const sharePercent = sumVal > 0 ? (p._sortVal / sumVal) * 100 : 0;

    let primaryLabel = "";
    let secondaryLabel = "";
    switch (activeMode) {
      case "dps":
        primaryLabel = `${formatNumber(p.dps, 1)} /s`;
        secondaryLabel = `${formatNumber(p.damage)} total (${sharePercent.toFixed(1)}%)`;
        break;
      case "heal":
        primaryLabel = formatNumber(p.heal);
        secondaryLabel = `${formatNumber(p.hps, 1)} HPS • OH ${(p.overhealPercent || 0).toFixed(0)}% (${sharePercent.toFixed(1)}%)`;
        break;
      case "hps":
        primaryLabel = `${formatNumber(p.hps, 1)} /s`;
        secondaryLabel = `${formatNumber(p.heal)} soins • OH ${(p.overhealPercent || 0).toFixed(0)}%`;
        break;
      case "overheal":
        primaryLabel = `${formatNumber(p.overhealed)} OH`;
        secondaryLabel = `${(p.overhealPercent || 0).toFixed(1)}% des soins bruts`;
        break;
      case "taken":
        primaryLabel = formatNumber(p.takenDamage);
        secondaryLabel = `${sharePercent.toFixed(1)}% subis`;
        break;
      case "damage":
      default:
        primaryLabel = formatNumber(p.damage);
        secondaryLabel = `${formatNumber(p.dps, 1)} DPS (${sharePercent.toFixed(1)}%)`;
        break;
    }

    return {
      ...p,
      rank: index + 1,
      barPercent,
      sharePercent,
      primaryLabel,
      secondaryLabel,
    };
  });
};

export const formatMeterClipboard = (
  players,
  mode = "damage",
  style = "compact",
  mapName = "Zone",
  combatSeconds = 0,
) => {
  const ranked = rankMeterPlayers(players, mode).filter((p) => p._sortVal > 0);
  if (ranked.length === 0) {
    return "[Albion Stats] Aucune donnée de combat active.";
  }

  const modeLabel =
    mode === "heal" || mode === "hps"
      ? "Soins"
      : mode === "taken"
        ? "Subis"
        : mode === "overheal"
          ? "Overheal"
          : "Dégâts";

  if (style === "compact") {
    const parts = ranked
      .slice(0, 5)
      .map(
        (p) =>
          `${p.rank}. ${p.name} ${p.primaryLabel} (${p.sharePercent.toFixed(0)}%)`,
      );
    return `[Albion Stats • ${modeLabel} • ${mapName} • ${formatDuration(combatSeconds)}] ${parts.join(" | ")}`;
  }

  const lines = [
    `=== Albion Stats — ${modeLabel} (${mapName} • ${formatDuration(combatSeconds)}) ===`,
  ];
  for (const p of ranked) {
    lines.push(
      `${p.rank}. ${p.name} [${p.weaponNameFr || p.weaponUniqueName}] — ${p.primaryLabel} (${p.secondaryLabel})`,
    );
  }
  return lines.join("\n");
};

export const buildAlbionDataPriceUrl = (
  region = "europe",
  uniqueNames = [],
  locations = ["Caerleon", "Thetford", "Martlock", "Lymhurst", "Bridgewatch", "Fort Sterling"],
  qualities = [1, 2, 3],
) => {
  const host =
    region === "west"
      ? "https://west.albion-online-data.com"
      : region === "east"
        ? "https://east.albion-online-data.com"
        : "https://europe.albion-online-data.com";
  const items = uniqueNames.filter(Boolean).join(",");
  const locs = encodeURIComponent(locations.filter(Boolean).join(","));
  const quals = qualities.join(",");
  return `${host}/api/v2/stats/prices/${items}.json?locations=${locs}&qualities=${quals}`;
};

export const summarizeMarketPrices = (rows = []) => {
  if (!Array.isArray(rows)) return [];
  const byItem = new Map();

  for (const row of rows) {
    if (!row || !row.item_id) continue;
    const itemId = String(row.item_id);
    const city = String(row.city || "");
    const sellMin = Number(row.sell_price_min) || 0;
    const buyMax = Number(row.buy_price_max) || 0;
    const quality = Number(row.quality) || 1;

    if (!byItem.has(itemId)) {
      byItem.set(itemId, {
        itemId,
        bestSellPrice: 0,
        bestSellCity: "-",
        bestSellQuality: 1,
        bestBuyOrder: 0,
        bestBuyCity: "-",
        spreadSilver: 0,
        spreadPercent: 0,
      });
    }
    const entry = byItem.get(itemId);
    if (sellMin > 0 && (entry.bestSellPrice === 0 || sellMin < entry.bestSellPrice)) {
      entry.bestSellPrice = sellMin;
      entry.bestSellCity = city;
      entry.bestSellQuality = quality;
    }
    if (buyMax > entry.bestBuyOrder) {
      entry.bestBuyOrder = buyMax;
      entry.bestBuyCity = city;
    }
  }

  const out = [];
  for (const entry of byItem.values()) {
    if (entry.bestSellPrice > 0 && entry.bestBuyOrder > 0) {
      entry.spreadSilver = entry.bestBuyOrder - entry.bestSellPrice;
      entry.spreadPercent = (entry.spreadSilver / entry.bestSellPrice) * 100;
    }
    out.push(entry);
  }
  return out;
};

export const buildAlbionItemIconUrl = (uniqueName, size = 128) => {
  const clean = String(uniqueName ?? "").trim();
  if (!clean || clean === "UNARMED" || clean === "SILVER" || clean.startsWith("ITEM_")) {
    return null;
  }
  const px = Math.max(16, Math.min(217, Math.round(Number(size) || 128)));
  const encodedId = encodeURIComponent(clean).replace(/%40/g, "@");
  return `https://render.albiononline.com/v1/item/${encodedId}.png?size=${px}`;
};

export const buildAlbionItemIconCandidates = (uniqueName, size = 128) => {
  const clean = String(uniqueName ?? "").trim();
  if (!clean || clean === "UNARMED" || clean === "SILVER" || clean.startsWith("ITEM_")) {
    return [];
  }
  const px = Math.max(16, Math.min(217, Math.round(Number(size) || 128)));
  const encodedId = encodeURIComponent(clean).replace(/%40/g, "@");
  const urls = [
    `https://render.albiononline.com/v1/item/${encodedId}.png?size=${px}`,
    `https://render.albiononline.com/v1/item/${encodedId}.png`,
  ];
  const atIdx = clean.lastIndexOf("@");
  if (atIdx > 0) {
    const baseRaw = clean.slice(0, atIdx);
    const enchRaw = clean.slice(atIdx + 1);
    const encodedBase = encodeURIComponent(baseRaw);
    if (/^\d+$/.test(enchRaw) && Number(enchRaw) > 0) {
      urls.push(`https://render.albiononline.com/v1/item/${encodedBase}.png?enchantment=${enchRaw}&size=${px}`);
    }
    urls.push(`https://render.albiononline.com/v1/item/${encodedBase}.png`);
  }
  return [...new Set(urls)];
};

export const buildAlbionSpellIconUrl = (uniqueName, size = 64) => {
  const clean = String(uniqueName ?? "").trim();
  if (!clean || /^SPELL_\d+$/.test(clean) || clean === "UNKNOWN" || clean === "AUTO_ATTACK") {
    return null;
  }
  const px = Math.max(16, Math.min(217, Math.round(Number(size) || 64)));
  return `https://render.albiononline.com/v1/spell/${encodeURIComponent(clean)}.png?size=${px}`;
};

export const buildAlbionSpellIconCandidates = (spell, size = 64) => {
  const obj = typeof spell === "string" ? { uniqueName: spell } : spell || {};
  const uniqueName = String(obj.uniqueName ?? "").trim();
  const uiSprite = String(obj.uiSprite ?? "").trim();
  const rawNameEn = String(obj.nameEn ?? "").trim();

  if (uniqueName === "AUTO_ATTACK") {
    return [];
  }

  const px = Math.max(16, Math.min(217, Math.round(Number(size) || 64)));
  const urls = [];

  if (uiSprite && uiSprite !== "UNKNOWN") {
    const encSprite = encodeURIComponent(uiSprite);
    urls.push(`https://render.albiononline.com/v1/spell/${encSprite}.png?size=${px}`);
    urls.push(`https://render.albiononline.com/v1/spell/${encSprite}.png`);
  }

  const cleanNameEn = rawNameEn.replace(/\s*\([^)]*\)\s*$/g, "").trim();
  if (
    cleanNameEn &&
    cleanNameEn.toLowerCase() !== "auto attack" &&
    !/^spell\s*#\d+$/i.test(cleanNameEn)
  ) {
    const encEn = encodeURIComponent(cleanNameEn);
    urls.push(`https://render.albiononline.com/v1/spell/${encEn}.png?locale=en&size=${px}`);
    urls.push(`https://render.albiononline.com/v1/spell/${encEn}.png?locale=en`);
  }

  if (uniqueName && !/^SPELL_\d+$/.test(uniqueName) && uniqueName !== "UNKNOWN") {
    const encUnique = encodeURIComponent(uniqueName);
    urls.push(`https://render.albiononline.com/v1/spell/${encUnique}.png?size=${px}`);
    urls.push(`https://render.albiononline.com/v1/spell/${encUnique}.png`);
  }

  return [...new Set(urls)];
};

const ARTEFACT_MARKERS = [
  "_HELL",
  "_KEEPER",
  "_MORGANA",
  "_UNDEAD",
  "_AVALON",
  "_FEY",
  "_CRYSTAL",
  "_FW_",
  "_INSIGHT",
];

export const estimateCraftingDefaults = (uniqueName, sellPrice = 0, itemValue = 0) => {
  const clean = String(uniqueName ?? "").trim().toUpperCase();
  const { tier } = parseItemTierAndEnchantment(clean);
  const isArtefact = ARTEFACT_MARKERS.some((marker) => clean.includes(marker));
  const refPrice = Math.max(0, Number(sellPrice) || (Number(itemValue) || 0) * 16);
  const roundStep = (v) => (v >= 1000 ? Math.round(v / 100) * 100 : Math.round(v));

  const returnableCost = isArtefact ? roundStep(refPrice * 0.52) : roundStep(refPrice * 0.76);
  const artefactCost = isArtefact ? roundStep(refPrice * 0.24) : 0;

  const nonJournalItem =
    clean.includes("_POTION_") ||
    clean.includes("_MEAL_") ||
    clean.includes("_MOUNT_") ||
    clean.includes("_WOOD") ||
    clean.includes("_ORE") ||
    clean.includes("_FIBER") ||
    clean.includes("_HIDE") ||
    clean.includes("_ROCK") ||
    clean.includes("_FISH");

  let journalProfit = 0;
  if (!nonJournalItem && refPrice > 0) {
    const t = tier || 6;
    journalProfit = t >= 8 ? 22000 : t === 7 ? 12500 : t === 6 ? 5500 : 2000;
  }

  return {
    isArtefact,
    returnableCost,
    artefactCost,
    journalProfit,
  };
};

/**
 * Evaluates exact per-crafted-unit returnable resource and non-returnable artefact costs
 * from a catalog `CraftingRecipe` and summarized Albion Data Project price rows.
 */
export const computeRecipeMaterialCosts = (recipe, summarizedPrices = []) => {
  if (!recipe || !Array.isArray(recipe.ingredients) || recipe.ingredients.length === 0) {
    return null;
  }
  const amountCrafted = Math.max(1, Number(recipe.amountCrafted) || 1);
  const byId = new Map((summarizedPrices || []).map((r) => [r.itemId, r]));

  let returnablePerUnitFloat = 0;
  let nonReturnablePerUnitFloat = 0;
  let pricedCount = 0;
  let missingCount = 0;

  const lines = recipe.ingredients.map((ing) => {
    const row = byId.get(ing.uniqueName);
    const hasSell = (row?.bestSellPrice || 0) > 0;
    const hasBuy = (row?.bestBuyOrder || 0) > 0;
    const unitPrice = hasSell ? row.bestSellPrice : hasBuy ? row.bestBuyOrder : 0;
    const city = hasSell ? row.bestSellCity : hasBuy ? row.bestBuyCity : "-";
    const priceSource = hasSell ? "sell" : hasBuy ? "buy" : "missing";
    const count = Math.max(0, Number(ing.count) || 0);
    const lineBatchCost = count * unitPrice;
    const linePerCraftedUnit = lineBatchCost / amountCrafted;

    if (unitPrice > 0) pricedCount += 1;
    else missingCount += 1;

    if (ing.isReturnable) returnablePerUnitFloat += linePerCraftedUnit;
    else nonReturnablePerUnitFloat += linePerCraftedUnit;

    return {
      uniqueName: ing.uniqueName,
      nameFr: ing.nameFr || ing.uniqueName,
      count,
      isReturnable: Boolean(ing.isReturnable),
      unitPrice,
      city,
      priceSource,
      lineBatchCost: Math.round(lineBatchCost),
      linePerCraftedUnit: Math.round(linePerCraftedUnit),
    };
  });

  return {
    itemUniqueName: recipe.itemUniqueName,
    amountCrafted,
    craftingFocus: Number(recipe.craftingFocus) || 0,
    itemValue: Number(recipe.itemValue) || 0,
    returnablePerUnit: Math.round(returnablePerUnitFloat),
    nonReturnablePerUnit: Math.round(nonReturnablePerUnitFloat),
    pricedCount,
    missingCount,
    lines,
  };
};

const isPriceableUniqueName = (name) =>
  typeof name === "string" &&
  name.length > 0 &&
  name !== "SILVER" &&
  name !== "UNARMED" &&
  !name.startsWith("ITEM_") &&
  /^T\d_[A-Z0-9_@]+$/.test(name);

/** Collects unique priceable Albion item ids and quantities from loot and gathering logs. */
export const collectSessionPriceableItems = (snapshot) => {
  const qtyByItem = {};
  const add = (name, qty) => {
    const clean = String(name ?? "").trim();
    const amount = Math.max(0, Math.round(Number(qty) || 0));
    if (!isPriceableUniqueName(clean) || amount <= 0) return;
    qtyByItem[clean] = (qtyByItem[clean] || 0) + amount;
  };
  for (const l of snapshot?.lootLog || []) add(l?.uniqueName, l?.quantity);
  for (const g of snapshot?.gatheringLog || []) add(g?.uniqueName, g?.totalAmount);
  return { items: Object.keys(qtyByItem), qtyByItem };
};

/** Sums the live market value of a session using best sell prices (falls back to buy orders). */
export const computeSessionMarketValue = (summarized = [], qtyByItem = {}) => {
  let total = 0;
  let pricedItems = 0;
  for (const row of summarized) {
    const qty = Number(qtyByItem[row.itemId]) || 0;
    const unit = row.bestSellPrice > 0 ? row.bestSellPrice : row.bestBuyOrder;
    if (qty > 0 && unit > 0) {
      total += unit * qty;
      pricedItems += 1;
    }
  }
  return { total, pricedItems };
};

export const parseItemTierAndEnchantment = (uniqueName) => {
  const clean = String(uniqueName ?? "").trim();
  const m = clean.match(/^T(\d+)_/);
  const tier = m ? parseInt(m[1], 10) : null;
  const enchMatch = clean.match(/@(\d+)$/);
  const enchantment = enchMatch ? parseInt(enchMatch[1], 10) : 0;
  const label = tier !== null ? `T${tier}${enchantment > 0 ? `.${enchantment}` : ""}` : "";
  return { tier, enchantment, label };
};

export const CITY_INFO = {
  Martlock: { key: "martlock", icon: "🛡️", label: "Martlock" },
  Caerleon: { key: "caerleon", icon: "⚔️", label: "Caerleon" },
  Bridgewatch: { key: "bridgewatch", icon: "🏜️", label: "Bridgewatch" },
  Lymhurst: { key: "lymhurst", icon: "🌲", label: "Lymhurst" },
  "Fort Sterling": { key: "fort-sterling", icon: "❄️", label: "Fort Sterling" },
  Thetford: { key: "thetford", icon: "🌿", label: "Thetford" },
  Brecilien: { key: "brecilien", icon: "🌸", label: "Brecilien" },
  "Black Market": { key: "black-market", icon: "🏴", label: "Marché Noir" },
};

export const getCityBadgeInfo = (cityName) => {
  const clean = String(cityName ?? "").trim();
  if (!clean || clean === "-") return null;
  return CITY_INFO[clean] || { key: "default", icon: "🏛️", label: clean };
};
