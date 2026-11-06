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

export const buildAlbionItemIconUrl = (uniqueName, size = 40) => {
  const clean = String(uniqueName ?? "").trim();
  if (!clean || clean === "UNARMED" || clean === "SILVER" || clean.startsWith("ITEM_")) {
    return null;
  }
  const px = Math.max(16, Math.min(217, Math.round(Number(size) || 40)));
  return `https://render.albiononline.com/v1/item/${encodeURIComponent(clean)}.png?size=${px}`;
};
