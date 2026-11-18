import {
  buildAlbionDataPriceUrl,
  buildAlbionItemIconCandidates,
  buildAlbionSpellIconCandidates,
  collectSessionPriceableItems,
  computeRecipeMaterialCosts,
  computeSessionMarketValue,
  estimateCraftingDefaults,
  formatDuration,
  formatExactInt,
  formatMeterClipboard,
  formatNumber,
  getCityBadgeInfo,
  normalizeSessionRecord,
  parseItemTierAndEnchantment,
  rankMeterPlayers,
  summarizeMarketPrices,
} from "./meter.js";
import { DEFAULT_SETTINGS, normalizeSettings } from "./settings.js";

const tauriCore = window.__TAURI__?.core;
const tauriWindow = window.__TAURI__?.window?.getCurrentWindow?.();

const invokeCmd = async (cmd, args = {}) => {
  if (!tauriCore?.invoke) return null;
  return tauriCore.invoke(cmd, args);
};

let settings = { ...DEFAULT_SETTINGS };
let latestState = null;
let selectedPlayerName = null;
let selectedSnapshotId = "live";
let selectedSessionId = "live";
let activeSessionSubtab = "all";
let activeTab = "combat";
let lastStateSignature = "";
let toastTimer = null;
let searchTimer = null;
let searchSeq = 0;
let recipePriceSeq = 0;
let activeCraftItemContext = null;
let lastPriceRows = [];
let browserMaximizedFallback = false;
let catalogAutoSyncAttempted = false;
let catalogSyncInFlight = false;
const itemInfoCache = new Map();
const expandedSessionCombatPlayers = new Set();
const resolvedIconCache = new Map();
const failedIconUrls = new Set();
const lastRenderKeys = new Map();

const SLOT_LABELS_FR = {
  mainHand: "Arme",
  offHand: "Main G.",
  head: "Tête",
  chest: "Torse",
  shoes: "Pieds",
  bag: "Sac",
  cape: "Cape",
  mount: "Monture",
  potion: "Potion",
  food: "Repas",
  resource: "Ressource",
  material: "Matériau",
};

const el = (id) => document.getElementById(id);

const shouldSkipRender = (sectionKey, dataKey, force = false) => {
  if (!force && lastRenderKeys.get(sectionKey) === dataKey) {
    return true;
  }
  lastRenderKeys.set(sectionKey, dataKey);
  return false;
};

const showToast = (msg) => {
  const banner = el("toast-banner");
  if (!banner) return;
  banner.textContent = msg;
  banner.hidden = false;
  if (toastTimer) clearTimeout(toastTimer);
  toastTimer = setTimeout(() => {
    banner.hidden = true;
  }, 3200);
};

const escapeHtml = (str) =>
  String(str ?? "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");

const inferItemFallbackGlyph = (uniqueName, slotHint = "") => {
  const clean = String(uniqueName ?? "").toUpperCase();
  const slot = String(slotHint ?? "");
  if (clean === "SILVER") return "ico-slot-silver";
  if (
    slot === "head" ||
    slot === "chest" ||
    slot === "shoes" ||
    slot === "cape" ||
    clean.includes("_HEAD_") ||
    clean.includes("_ARMOR_") ||
    clean.includes("_SHOES_") ||
    clean.includes("_CAPE")
  ) {
    return "ico-slot-armor";
  }
  if (slot === "bag" || clean.includes("_BAG")) return "ico-slot-bag";
  if (
    slot === "potion" ||
    slot === "food" ||
    clean.includes("_POTION_") ||
    clean.includes("_MEAL_")
  ) {
    return "ico-slot-potion";
  }
  if (
    slot === "resource" ||
    slot === "material" ||
    clean.includes("_WOOD") ||
    clean.includes("_ORE") ||
    clean.includes("_FIBER") ||
    clean.includes("_HIDE") ||
    clean.includes("_ROCK") ||
    clean.includes("_PLANKS") ||
    clean.includes("_METALBAR") ||
    clean.includes("_CLOTH") ||
    clean.includes("_LEATHER") ||
    clean.includes("_STONEBLOCK") ||
    clean.includes("_ARTEFACT") ||
    clean.includes("_FISH") ||
    clean.includes("_RUNE") ||
    clean.includes("_SOUL") ||
    clean.includes("_RELIC")
  ) {
    return "ico-slot-resource";
  }
  return "ico-slot-weapon";
};

const inferSpellFallbackGlyph = (uniqueName, category = "") => {
  const clean = String(uniqueName ?? "").toUpperCase();
  const cat = String(category ?? "").toLowerCase();
  if (clean === "AUTO_ATTACK" || cat === "attack") return "ico-spell-attack";
  if (cat === "heal") return "ico-spell-heal";
  if (cat === "buff") return "ico-spell-buff";
  return "ico-spell-damage";
};

const markSlotImgLoaded = (img) => {
  if (!(img instanceof HTMLImageElement) || !img.classList.contains("slot-img")) return;
  const loadedUrl = img.currentSrc || img.src;
  const cacheKey = img.getAttribute("data-icon-key");
  if (cacheKey && loadedUrl) {
    resolvedIconCache.set(cacheKey, loadedUrl);
  }
  img.hidden = false;
  img.classList.add("is-loaded");
  const slot = img.closest(".icon-slot");
  if (slot) {
    slot.classList.remove("is-fallback");
    slot.classList.add("is-loaded");
  }
};

const hydrateLoadedImages = (root = document) => {
  if (!root || typeof root.querySelectorAll !== "function") return;
  root.querySelectorAll("img.slot-img").forEach((img) => {
    if (img.complete && img.naturalWidth > 0) {
      markSlotImgLoaded(img);
    }
  });
};

const prepareIconCandidates = (cacheKey, rawCandidates) => {
  const cachedUrl = cacheKey ? resolvedIconCache.get(cacheKey) : null;
  const filtered = [];
  if (cachedUrl && !failedIconUrls.has(cachedUrl)) {
    filtered.push(cachedUrl);
  }
  for (const url of rawCandidates) {
    if (url && !failedIconUrls.has(url) && !filtered.includes(url)) {
      filtered.push(url);
    }
  }
  return filtered;
};

// Global CSP-compliant capture-phase media handlers (replace blocked inline onload/onerror)
document.addEventListener(
  "load",
  (ev) => {
    const target = ev.target;
    if (target instanceof HTMLImageElement && target.classList.contains("slot-img")) {
      markSlotImgLoaded(target);
    }
  },
  true,
);

document.addEventListener(
  "error",
  (ev) => {
    const target = ev.target;
    if (!(target instanceof HTMLImageElement) || !target.classList.contains("slot-img")) {
      return;
    }
    const failedUrl = target.currentSrc || target.getAttribute("src") || target.src;
    if (failedUrl) {
      failedIconUrls.add(failedUrl);
    }
    const cacheKey = target.getAttribute("data-icon-key");
    if (cacheKey && resolvedIconCache.get(cacheKey) === failedUrl) {
      resolvedIconCache.delete(cacheKey);
    }
    const rawFallbacks = target.getAttribute("data-fallbacks") || "";
    const remaining = rawFallbacks
      .split("|")
      .filter((u) => Boolean(u) && !failedIconUrls.has(u));
    if (remaining.length > 0) {
      const nextUrl = remaining.shift();
      target.setAttribute("data-fallbacks", remaining.join("|"));
      target.src = nextUrl;
      return;
    }
    target.hidden = true;
    target.classList.remove("is-loaded");
    const slot = target.closest(".icon-slot");
    if (slot) {
      slot.classList.remove("is-loaded");
      slot.classList.add("is-fallback");
    }
  },
  true,
);

const renderCityBadge = (cityName) => {
  const info = getCityBadgeInfo(cityName);
  if (!info) return `<span class="dash-empty">—</span>`;
  return `<span class="city-pill city-${info.key}"><span class="city-dot"></span>${escapeHtml(info.label)}</span>`;
};

const renderItemThumb = (uniqueName, cls = "icon-slot-sm", slotHint = "") => {
  const clean = String(uniqueName ?? "").trim();
  const iconKey = clean ? `item:${clean}` : "";
  const candidates = prepareIconCandidates(iconKey, buildAlbionItemIconCandidates(clean, 128));
  const glyph = inferItemFallbackGlyph(clean, slotHint);
  if (candidates.length === 0) {
    return `
      <span class="icon-slot ${cls} is-fallback" aria-hidden="true">
        <svg class="slot-fallback-svg"><use href="#${glyph}"></use></svg>
      </span>
    `;
  }
  const [primary, ...fallbacks] = candidates;
  return `
    <span class="icon-slot ${cls}">
      <svg class="slot-fallback-svg" aria-hidden="true"><use href="#${glyph}"></use></svg>
      <img class="slot-img" data-icon-key="${escapeHtml(iconKey)}" src="${escapeHtml(primary)}" data-fallbacks="${escapeHtml(fallbacks.join("|"))}" alt="" decoding="async" />
    </span>
  `;
};

const renderItemThumbWithTier = (uniqueName, slotHint = "") => {
  const clean = String(uniqueName ?? "").trim();
  const iconKey = clean ? `item:${clean}` : "";
  const candidates = prepareIconCandidates(iconKey, buildAlbionItemIconCandidates(clean, 128));
  const glyph = inferItemFallbackGlyph(clean, slotHint);
  const meta = parseItemTierAndEnchantment(clean);
  const tierCls = meta.tier ? `tier-box-${meta.tier}` : "";
  const tierBadge = meta.label
    ? `<span class="tier-chip ${meta.tier ? `tier-${meta.tier}` : ""}">${meta.label}</span>`
    : "";
  const enchBadge =
    meta.enchantment > 0
      ? `<span class="ench-dot ench-${meta.enchantment}" title="Enchantement .${meta.enchantment}"></span>`
      : "";

  if (candidates.length === 0) {
    return `
      <span class="icon-slot item-thumb-box ${tierCls} is-fallback">
        <svg class="slot-fallback-svg" aria-hidden="true"><use href="#${glyph}"></use></svg>
        ${tierBadge}
        ${enchBadge}
      </span>
    `;
  }
  const [primary, ...fallbacks] = candidates;
  return `
    <span class="icon-slot item-thumb-box ${tierCls}">
      <svg class="slot-fallback-svg" aria-hidden="true"><use href="#${glyph}"></use></svg>
      <img class="slot-img item-thumb-img" data-icon-key="${escapeHtml(iconKey)}" src="${escapeHtml(primary)}" data-fallbacks="${escapeHtml(fallbacks.join("|"))}" alt="" decoding="async" />
      ${tierBadge}
      ${enchBadge}
    </span>
  `;
};

const renderSpellThumb = (spell) => {
  const obj = typeof spell === "string" ? { uniqueName: spell } : spell || {};
  const clean = String(obj.uniqueName ?? "").trim();
  const iconKey = clean ? `spell:${clean}:${obj.uiSprite || ""}` : "";
  const candidates = prepareIconCandidates(iconKey, buildAlbionSpellIconCandidates(obj, 64));
  const glyph = inferSpellFallbackGlyph(clean, obj.category);
  const catCls = `spell-cat-${escapeHtml(String(obj.category || "damage").toLowerCase())}`;

  if (candidates.length === 0) {
    return `
      <span class="icon-slot icon-slot-sm ${catCls} is-fallback" aria-hidden="true">
        <svg class="slot-fallback-svg"><use href="#${glyph}"></use></svg>
      </span>
    `;
  }
  const [primary, ...fallbacks] = candidates;
  return `
    <span class="icon-slot icon-slot-sm ${catCls}">
      <svg class="slot-fallback-svg" aria-hidden="true"><use href="#${glyph}"></use></svg>
      <img class="slot-img" data-icon-key="${escapeHtml(iconKey)}" src="${escapeHtml(primary)}" data-fallbacks="${escapeHtml(fallbacks.join("|"))}" alt="" decoding="async" />
    </span>
  `;
};

const persistSettings = async () => {
  try {
    await invokeCmd("save_settings", { settings });
  } catch (e) {
    console.warn("save_settings:", e);
  }
};

const syncMaximizedState = async () => {
  let isMax = browserMaximizedFallback;
  if (tauriWindow?.isMaximized) {
    try {
      isMax = Boolean(await tauriWindow.isMaximized());
    } catch {
      // Ignore if permission or context is unavailable
    }
  }
  el("app")?.classList.toggle("is-maximized", isMax);
  const maxBtn = el("btn-max");
  const maxUse = el("btn-max-use");
  if (maxBtn) {
    const label = isMax ? "Restaurer la fenêtre" : "Agrandir la fenêtre";
    maxBtn.title = label;
    maxBtn.setAttribute("aria-label", label);
  }
  if (maxUse) {
    maxUse.setAttribute("href", isMax ? "#ico-restore" : "#ico-max");
  }
};

const toggleMaximizeWindow = async () => {
  if (tauriWindow?.toggleMaximize) {
    try {
      await tauriWindow.toggleMaximize();
    } catch (e) {
      console.warn("toggleMaximize:", e);
    }
  } else {
    browserMaximizedFallback = !browserMaximizedFallback;
  }
  await syncMaximizedState();
};

const applyWindowPreferences = async () => {
  const shell = el("app");
  shell?.classList.toggle("compact-layout", settings.compactMode);
  el("btn-compact")?.classList.toggle("active", settings.compactMode);
  await syncMaximizedState();
};

const activeMeterDataset = () => {
  const snap = latestState?.snapshot;
  if (!snap) {
    return {
      mapName: "En attente…",
      combatSeconds: 0,
      totalDamage: 0,
      totalHeal: 0,
      totalTaken: 0,
      players: [],
    };
  }
  if (selectedSnapshotId !== "live") {
    const found = (snap.snapshots || []).find((s) => s.id === selectedSnapshotId);
    if (found) {
      return {
        mapName: found.mapNameFr,
        combatSeconds: found.combatSeconds,
        totalDamage: found.totalDamage,
        totalHeal: found.totalHeal,
        totalTaken: found.totalTaken,
        players: found.players || [],
      };
    }
  }
  return {
    mapName: snap.localPlayer?.currentMapNameFr || "En attente…",
    combatSeconds: snap.combatSeconds || 0,
    totalDamage: snap.totalPartyDamage || 0,
    totalHeal: snap.totalPartyHeal || 0,
    totalTaken: snap.totalPartyTaken || 0,
    players: snap.meterPlayers || [],
  };
};

const renderHeader = () => {
  const snap = latestState?.snapshot;
  const cap = latestState?.capture;
  if (!snap) return;

  const lp = snap.localPlayer || {};
  const rawName = String(lp.username || "En attente…");
  const isStandby = rawName.startsWith("En attente");
  const rawPkts = cap?.rawDatagramsSeen || 0;
  const procPkts = snap.network?.packetsProcessed || 0;
  const totalPkts = Math.max(rawPkts, procPkts);
  const totalEvents = snap.network?.telemetryEventsProcessed || 0;

  const badge = el("capture-badge");
  if (badge && cap) {
    const isLiveCapture = cap.mode === "rawSocket" || cap.mode === "npcap";
    const modeLabel = cap.mode === "npcap" ? "Npcap 5056" : "UDP 5056";
    if (!isLiveCapture) {
      badge.className = "status-badge status-standby";
      badge.innerHTML = `<span class="badge-dot"></span><span class="badge-text">Mode veille</span>`;
      badge.title = cap.detail || "Mode veille (en attente de trafic Albion)";
    } else if (totalPkts === 0) {
      badge.className = "status-badge status-listening";
      badge.innerHTML = `<span class="badge-dot"></span><span class="badge-text">${modeLabel} (Écoute)</span>`;
      badge.title = `Capture active via ${cap.bindAddress} (${cap.mode})\n\n• Si Albion est ouvert, changez de zone ou attaquez un monstre pour générer du trafic.\n• Si aucun paquet n'apparaît après avoir bougé, lancez en mode Admin ou installez Npcap pour capturer directement au niveau de la carte réseau.`;
    } else if (isStandby) {
      badge.className = "status-badge status-syncing";
      badge.innerHTML = `<span class="badge-dot"></span><span class="badge-text">${modeLabel} (${formatExactInt(totalPkts)} pkts)</span>`;
      badge.title = `${formatExactInt(totalPkts)} paquets Photon capturés (${formatExactInt(totalEvents)} événements).\n\nEn attente de synchronisation du profil : changez de zone (portail / donjon) ou combattez un monstre pour initialiser votre personnage.`;
    } else {
      badge.className = "status-badge status-live";
      badge.innerHTML = `<span class="badge-dot"></span><span class="badge-text">${modeLabel}</span>`;
      badge.title = `Capture active via ${cap.bindAddress} (${cap.mode})\n• ${formatExactInt(totalPkts)} paquets capturés\n• ${formatExactInt(totalEvents)} événements décodés`;
    }
  }

  const syncBanner = el("sync-banner");
  if (syncBanner) {
    if (isStandby) {
      syncBanner.hidden = false;
      const statEl = el("sync-banner-stat");
      if (statEl) {
        if (totalPkts > 0) {
          statEl.textContent = `${formatExactInt(totalPkts)} paquets détectés (changement de zone requis)`;
        } else if (cap?.mode === "npcap") {
          statEl.textContent = "Npcap actif (0 paquet capturé — changez de carte)";
        } else {
          statEl.textContent = "Écoute active (0 paquet capturé)";
        }
      }
    } else {
      syncBanner.hidden = true;
    }
  }

  const elevateBtn = el("btn-elevate");
  if (elevateBtn && cap) {
    elevateBtn.hidden = !cap.needsElevation;
  }

  const catCount = el("catalog-count");
  if (catCount && snap.catalogItemCount) {
    catCount.textContent = formatExactInt(snap.catalogItemCount);
    const syncBtn = el("btn-sync-catalog");
    if (syncBtn) {
      syncBtn.title = `Catalogue : ${formatExactInt(snap.catalogItemCount)} objets • ${formatExactInt(snap.catalogSpellCount || 0)} sorts • ${formatExactInt(snap.catalogClusterCount || 0)} zones • ${formatExactInt(snap.catalogRecipeCount || 0)} recettes — cliquer pour synchroniser ao-bin-dumps`;
    }
  }

  const guildTag = lp.guildName ? ` [${lp.guildName}]` : "";
  const displayName = isStandby ? "En attente…" : `${rawName}${guildTag}`;
  const nameTitle = isStandby
    ? "En attente de paquets Albion Online… Si le jeu est déjà ouvert, changez de zone ou combattez un monstre pour synchroniser votre personnage."
    : displayName;
  const rawMap = String(lp.currentMapNameFr || "En attente…");
  const displayMap = rawMap.startsWith("En attente") ? "En attente…" : rawMap;
  el("header-player-name").textContent = displayName;
  el("header-player-name").title = nameTitle;
  el("header-map-name").textContent = displayMap;
  el("header-map-name").title = displayMap;
  el("header-silver").textContent = `${formatExactInt(lp.silver || 0)} Silver`;
};

const renderSnapshotSelect = () => {
  const select = el("snapshot-select");
  const delBtn = el("btn-delete-snapshot");
  const snaps = latestState?.snapshot?.snapshots || [];
  if (!select) return;

  const prev = selectedSnapshotId;
  let html = `<option value="live">Combat en direct</option>`;
  for (const s of snaps) {
    html += `<option value="${escapeHtml(s.id)}">${escapeHtml(s.label)}</option>`;
  }
  select.innerHTML = html;
  if (snaps.some((s) => s.id === prev)) {
    select.value = prev;
  } else {
    selectedSnapshotId = "live";
    select.value = "live";
  }
  if (delBtn) {
    delBtn.hidden = selectedSnapshotId === "live";
  }
};

const renderCombatTab = (force = false) => {
  const data = activeMeterDataset();
  el("combat-timer").textContent = formatDuration(data.combatSeconds);
  el("party-total-dmg").textContent = formatNumber(data.totalDamage);
  el("party-total-heal").textContent = formatNumber(data.totalHeal);
  el("party-total-taken").textContent = formatNumber(data.totalTaken);

  const ranked = rankMeterPlayers(data.players, settings.meterMode);
  if (ranked.length > 0 && (!selectedPlayerName || !ranked.some((p) => p.name === selectedPlayerName))) {
    selectedPlayerName = ranked[0].name;
  }

  const renderKey = JSON.stringify([settings.meterMode, selectedSnapshotId, selectedPlayerName, ranked]);
  if (shouldSkipRender("combat", renderKey, force)) {
    return;
  }

  const container = el("meter-rows");
  const spellWrap = el("spell-rows");
  if (ranked.length === 0) {
    container.innerHTML = `<div class="empty-state">Aucun combat actif. Lancez Albion Online, changez de zone ou engagez un combat pour afficher les statistiques en direct.</div>`;
    spellWrap.innerHTML = `<div class="empty-state">Sélectionnez un joueur pour inspecter la répartition de ses sorts.</div>`;
    el("spell-panel-title").textContent = "Détail des Sorts";
    el("spell-panel-weapon").textContent = "Sélectionnez un joueur";
    return;
  }

  container.innerHTML = ranked
    .map((p) => {
      const isSel = p.name === selectedPlayerName;
      const weaponThumb = renderItemThumb(p.weaponUniqueName, "icon-slot-md", "mainHand");
      const rankCls = p.rank <= 3 ? `rank-${p.rank}` : "";
      const rankLabel = String(p.rank).padStart(2, "0");
      return `
        <div class="meter-row ${isSel ? "selected" : ""}" data-player-name="${escapeHtml(p.name)}">
          <div class="meter-bar-fill meter-bar-${escapeHtml(settings.meterMode)}" style="width: ${p.barPercent.toFixed(1)}%"></div>
          <div class="meter-row-content">
            <div class="meter-left">
              <span class="meter-rank ${rankCls}">${rankLabel}</span>
              ${weaponThumb}
              <div class="meter-ident">
                <span class="meter-name">${escapeHtml(p.name)}${p.isSelf ? `<span class="self-tag">VOUS</span>` : ""}</span>
                <span class="meter-weapon">${escapeHtml(p.weaponNameFr)}</span>
              </div>
            </div>
            <div class="meter-right">
              <div class="meter-primary">${escapeHtml(p.primaryLabel)}</div>
              <div class="meter-secondary">${escapeHtml(p.secondaryLabel)}</div>
            </div>
          </div>
        </div>
      `;
    })
    .join("");
  hydrateLoadedImages(container);

  const activePlayer = ranked.find((p) => p.name === selectedPlayerName) || ranked[0];
  el("spell-panel-title").textContent = `Sorts — ${activePlayer.name}`;
  el("spell-panel-weapon").textContent = activePlayer.weaponNameFr;

  const spells = activePlayer.spells || [];
  if (spells.length === 0) {
    spellWrap.innerHTML = `<div class="empty-state">Aucun sort enregistré pour ce joueur.</div>`;
  } else {
    spellWrap.innerHTML = `
      <table class="data-table">
        <thead>
          <tr>
            <th>Sort</th>
            <th class="num-col" title="Nombre de ticks / coups enregistrés">Coups</th>
            <th class="num-col">Dégâts</th>
            <th class="num-col col-optional">Soins</th>
            <th class="num-col col-optional">Overheal</th>
            <th class="num-col">Part</th>
          </tr>
        </thead>
        <tbody>
          ${spells
            .map(
              (s) => `
            <tr>
              <td>
                <span class="cell-with-icon" title="${escapeHtml(s.uniqueName)}">
                  ${renderSpellThumb(s)}
                  <strong class="truncate-text">${escapeHtml(s.nameFr)}</strong>
                </span>
              </td>
              <td class="num-col mono">${s.ticks}</td>
              <td class="num-col mono">${formatExactInt(s.damage)}</td>
              <td class="num-col mono col-optional">${formatExactInt(s.heal)}</td>
              <td class="num-col mono col-optional">${formatExactInt(s.overhealed)}</td>
              <td class="num-col mono text-accent">${s.sharePercent.toFixed(1)}%</td>
            </tr>
          `,
            )
            .join("")}
        </tbody>
      </table>
    `;
    hydrateLoadedImages(spellWrap);
  }
};

const ZONE_KIND_LABELS_FR = {
  city: "Cité",
  safe: "Zone bleue",
  yellow: "Zone jaune",
  red: "Zone rouge",
  black: "Zone noire",
  openworld: "Monde ouvert",
  mists: "Brumes",
  avalon: "Routes d'Avalon",
  dungeon: "Donjon",
  solodungeon: "Donjon solo",
  groupdungeon: "Donjon de groupe",
  avaloniandungeon: "Donjon avalonien",
  corrupteddungeon: "Donjon corrompu",
  expedition: "Expédition",
  hellgate: "Hellgate",
  corrupted: "Donjon corrompu",
  island: "Île",
  unknown: "Zone",
};

const formatZoneKindFr = (kind, tier) => {
  const raw = String(kind ?? "").trim().toLowerCase();
  const label = ZONE_KIND_LABELS_FR[raw] || (raw ? raw : "Zone");
  return tier > 0 ? `${label} • T${tier}` : label;
};

const formatShortDateTime = (ms) => {
  if (!ms || ms <= 0) return "—";
  return new Date(ms).toLocaleString("fr-FR", {
    day: "2-digit",
    month: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
};

const sessionPlayerExpandKey = (sessionId, playerName) => `${sessionId}::${playerName}`;

const renderSessionZonesPanel = (rec) => {
  const zones = rec.zones || [];
  return `
    <div class="panel">
      <div class="panel-header">
        <div class="panel-heading-group">
          <h2>Zones traversées</h2>
          <span class="sub-label">Temps passé, renommée, silver et ressources par zone</span>
        </div>
        <span class="badge-subtle">${zones.length} zone${zones.length > 1 ? "s" : ""}</span>
      </div>
      <div class="table-wrap">
        ${
          zones.length === 0
            ? `<div class="empty-state">Aucune transition de zone enregistrée durant cette session.</div>`
            : `
          <table class="data-table">
            <thead>
              <tr>
                <th>Zone</th>
                <th>Type</th>
                <th class="num-col">Durée</th>
                <th class="num-col">Renommée</th>
                <th class="num-col">Silver</th>
                <th class="num-col col-optional">Récolte</th>
                <th class="num-col col-optional">K / D</th>
              </tr>
            </thead>
            <tbody>
              ${zones
                .map(
                  (z) => `
                <tr>
                  <td><strong>${escapeHtml(z.nameFr || z.clusterIndex || "Zone inconnue")}</strong></td>
                  <td><span class="mono-tag">${escapeHtml(formatZoneKindFr(z.kind, z.tier))}</span></td>
                  <td class="num-col mono">${formatDuration(z.durationSeconds)}</td>
                  <td class="num-col mono">${formatNumber(z.fame)}</td>
                  <td class="num-col mono">${formatNumber(z.silver)} Silver</td>
                  <td class="num-col mono col-optional">${formatExactInt(z.gatheringResources)} res. (${z.gatheringNodes || 0})</td>
                  <td class="num-col mono col-optional">${z.kills || 0} / ${z.deaths || 0}</td>
                </tr>
              `,
                )
                .join("")}
            </tbody>
          </table>
        `
        }
      </div>
    </div>
  `;
};

const renderSessionGatheringPanel = (rec) => {
  const t = rec.gatheringTotals || {};
  const log = rec.gatheringLog || [];
  return `
    <div class="panel">
      <div class="panel-header">
        <div class="panel-heading-group">
          <h2>Récolte de la session</h2>
          <span class="sub-label">${formatExactInt(t.totalResources)} ressources (${t.totalNodes || 0} nœuds) • ~${formatExactInt(t.totalEstimatedSilver)} Silver estimés</span>
        </div>
        <span class="badge-subtle">${log.length} type${log.length > 1 ? "s" : ""}</span>
      </div>
      <div class="session-gathering-strip">
        <div class="session-gat-pill"><span class="gat-kind"><svg class="ico"><use href="#ico-wood"></use></svg>Bois</span><strong>${formatExactInt(t.wood)}</strong></div>
        <div class="session-gat-pill"><span class="gat-kind"><svg class="ico"><use href="#ico-ore"></use></svg>Minerai</span><strong>${formatExactInt(t.ore)}</strong></div>
        <div class="session-gat-pill"><span class="gat-kind"><svg class="ico"><use href="#ico-fiber"></use></svg>Fibre</span><strong>${formatExactInt(t.fiber)}</strong></div>
        <div class="session-gat-pill"><span class="gat-kind"><svg class="ico"><use href="#ico-hide"></use></svg>Peau</span><strong>${formatExactInt(t.hide)}</strong></div>
        <div class="session-gat-pill"><span class="gat-kind"><svg class="ico"><use href="#ico-rock"></use></svg>Pierre</span><strong>${formatExactInt(t.rock)}</strong></div>
        <div class="session-gat-pill"><span class="gat-kind"><svg class="ico"><use href="#ico-fish"></use></svg>Poisson</span><strong>${formatExactInt(t.fish)}</strong></div>
      </div>
      <div class="table-wrap">
        ${
          log.length === 0
            ? `<div class="empty-state">Aucune ressource récoltée durant cette session.</div>`
            : `
          <table class="data-table">
            <thead>
              <tr>
                <th>Ressource</th>
                <th>Tier</th>
                <th class="num-col col-optional">Base</th>
                <th class="num-col col-optional">Récolteur</th>
                <th class="num-col col-optional">Premium</th>
                <th class="num-col">Total</th>
                <th class="num-col">Estimation</th>
              </tr>
            </thead>
            <tbody>
              ${log
                .map(
                  (g) => `
                <tr>
                  <td>
                    <span class="cell-with-icon">
                      ${renderItemThumb(g.uniqueName, "icon-slot-sm", "resource")}
                      <strong>${escapeHtml(g.nameFr)}</strong>
                    </span>
                  </td>
                  <td><span class="mono-tag">T${g.tier}.${g.enchantment}</span></td>
                  <td class="num-col mono col-optional">${g.standardAmount}</td>
                  <td class="num-col mono col-optional">+${g.collectorBonus}</td>
                  <td class="num-col mono col-optional">+${g.premiumBonus}</td>
                  <td class="num-col mono"><strong>${g.totalAmount}</strong></td>
                  <td class="num-col mono text-accent">${formatExactInt(g.estimatedSilver)} Silver</td>
                </tr>
              `,
                )
                .join("")}
            </tbody>
          </table>
        `
        }
      </div>
    </div>
  `;
};

const renderSessionCombatPanel = (rec) => {
  const players = [...(rec.combatPlayers || [])].sort(
    (a, b) => (b.damage || 0) + (b.heal || 0) - ((a.damage || 0) + (a.heal || 0)),
  );
  return `
    <div class="panel">
      <div class="panel-header">
        <div class="panel-heading-group">
          <h2>Combats &amp; Détail des sorts</h2>
          <span class="sub-label">Cliquez sur un joueur pour afficher ses sorts utilisés durant la session</span>
        </div>
        <span class="badge-subtle">${players.length} combattant${players.length > 1 ? "s" : ""}</span>
      </div>
      ${
        players.length === 0
          ? `<div class="empty-state">Aucun combat enregistré durant cette session.</div>`
          : `
        <div class="session-combat-list">
          ${players
            .map((p) => {
              const key = sessionPlayerExpandKey(rec.id, p.name);
              const isOpen = expandedSessionCombatPlayers.has(key);
              const spells = p.spells || [];
              return `
                <div class="session-combat-item">
                  <button type="button" class="session-combat-summary" data-toggle-session-player="${escapeHtml(p.name)}" aria-expanded="${isOpen ? "true" : "false"}">
                    <div class="session-combat-left">
                      ${renderItemThumb(p.weaponUniqueName, "icon-slot-sm", "mainHand")}
                      <div class="meter-ident">
                        <span class="meter-name">${escapeHtml(p.name)}${p.isSelf ? `<span class="self-tag">VOUS</span>` : ""}</span>
                        <span class="meter-weapon">${escapeHtml(p.weaponNameFr || p.weaponUniqueName || "Arme non inspectée")}</span>
                      </div>
                    </div>
                    <div class="session-combat-right">
                      <span><strong>${formatNumber(p.damage)}</strong> dég. <small class="text-dim">(${formatNumber(p.dps, 1)}/s)</small></span>
                      ${(p.heal || 0) > 0 ? `<span style="color: var(--heal);"><strong>${formatNumber(p.heal)}</strong> soins</span>` : ""}
                      <span class="col-optional">Subis ${formatNumber(p.takenDamage)}</span>
                      <span class="session-expand-hint">${isOpen ? "▾ Masquer" : `▸ ${spells.length} sort${spells.length > 1 ? "s" : ""}`}</span>
                    </div>
                  </button>
                  ${
                    isOpen
                      ? `
                    <div class="session-spell-drawer">
                      ${
                        spells.length === 0
                          ? `<div class="empty-state">Aucun sort enregistré.</div>`
                          : `
                        <table class="data-table">
                          <thead>
                            <tr>
                              <th>Sort</th>
                              <th class="num-col">Coups</th>
                              <th class="num-col">Dégâts</th>
                              <th class="num-col col-optional">Soins</th>
                              <th class="num-col col-optional">Overheal</th>
                              <th class="num-col">Part</th>
                            </tr>
                          </thead>
                          <tbody>
                            ${spells
                              .map(
                                (s) => `
                              <tr>
                                <td>
                                  <span class="cell-with-icon" title="${escapeHtml(s.uniqueName)}">
                                    ${renderSpellThumb(s)}
                                    <strong class="truncate-text">${escapeHtml(s.nameFr)}</strong>
                                  </span>
                                </td>
                                <td class="num-col mono">${s.ticks}</td>
                                <td class="num-col mono">${formatExactInt(s.damage)}</td>
                                <td class="num-col mono col-optional">${formatExactInt(s.heal)}</td>
                                <td class="num-col mono col-optional">${formatExactInt(s.overhealed)}</td>
                                <td class="num-col mono text-accent">${(s.sharePercent || 0).toFixed(1)}%</td>
                              </tr>
                            `,
                              )
                              .join("")}
                          </tbody>
                        </table>
                      `
                      }
                    </div>
                  `
                      : ""
                  }
                </div>
              `;
            })
            .join("")}
        </div>
      `
      }
    </div>
  `;
};

const renderSessionDungeonsPanel = (rec) => {
  const runs = rec.dungeons || [];
  return `
    <div class="panel">
      <div class="panel-header">
        <div class="panel-heading-group">
          <h2>Donjons &amp; Brumes</h2>
          <span class="sub-label">Instances parcourues, coffres et wisps</span>
        </div>
        <span class="badge-subtle">${runs.length} instance${runs.length > 1 ? "s" : ""}</span>
      </div>
      <div class="table-wrap">
        ${
          runs.length === 0
            ? `<div class="empty-state">Aucun donjon ou brume enregistré durant cette session.</div>`
            : `
          <table class="data-table">
            <thead>
              <tr>
                <th>Instance</th>
                <th>Tier</th>
                <th class="num-col">Durée</th>
                <th class="num-col">Renommée</th>
                <th class="num-col">Silver</th>
                <th class="num-col col-optional">Coffres / Wisps</th>
              </tr>
            </thead>
            <tbody>
              ${runs
                .map(
                  (d) => `
                <tr>
                  <td>
                    <span class="cell-with-icon">
                      <strong>${escapeHtml(d.nameFr)}</strong>
                      ${d.isActive ? '<span class="status-badge status-live"><span class="badge-dot"></span>En cours</span>' : ""}
                    </span>
                  </td>
                  <td><span class="mono-tag">T${d.tier}</span></td>
                  <td class="num-col mono">${formatDuration(d.durationSeconds)}</td>
                  <td class="num-col mono">${formatNumber(d.fame)}</td>
                  <td class="num-col mono">${formatNumber(d.silver)} Silver</td>
                  <td class="num-col mono col-optional">${d.chestsOpened || 0} / ${d.cagedWispsOpened || 0}</td>
                </tr>
              `,
                )
                .join("")}
            </tbody>
          </table>
        `
        }
      </div>
    </div>
  `;
};

const renderSessionLootPanel = (rec) => {
  const loot = rec.lootLog || [];
  const trades = rec.trades || [];
  return `
    <div class="panel">
      <div class="panel-header">
        <div class="panel-heading-group">
          <h2>Butin &amp; Échanges</h2>
          <span class="sub-label">Objets ramassés, silver et échanges directs</span>
        </div>
        <span class="badge-subtle">${loot.length} butin${loot.length > 1 ? "s" : ""}${trades.length > 0 ? ` • ${trades.length} échange${trades.length > 1 ? "s" : ""}` : ""}</span>
      </div>
      <div class="table-wrap">
        ${
          loot.length === 0
            ? `<div class="empty-state">Aucun butin ramassé durant cette session.</div>`
            : `
          <table class="data-table">
            <thead>
              <tr>
                <th>Joueur</th>
                <th class="col-optional">Source</th>
                <th>Objet</th>
                <th class="num-col">Qté</th>
                <th class="num-col">Estimation</th>
              </tr>
            </thead>
            <tbody>
              ${loot
                .map(
                  (l) => `
                <tr>
                  <td><strong>${escapeHtml(l.lootedBy)}</strong></td>
                  <td class="col-optional text-dim">${escapeHtml(l.lootedFrom)}</td>
                  <td>
                    <span class="cell-with-icon">
                      ${renderItemThumb(l.uniqueName, "icon-slot-sm")}
                      <span class="truncate-text">${escapeHtml(l.nameFr)}</span>
                    </span>
                  </td>
                  <td class="num-col mono">×${formatExactInt(l.quantity)}</td>
                  <td class="num-col mono text-accent">${formatExactInt(l.estimatedSilver)} Silver</td>
                </tr>
              `,
                )
                .join("")}
            </tbody>
          </table>
        `
        }
      </div>
      ${
        trades.length > 0
          ? `
        <div class="panel-subheader">
          <h3 class="sub-heading">Échanges directs</h3>
          <span class="badge-subtle">${trades.length}</span>
        </div>
        <div class="table-wrap">
          <table class="data-table">
            <thead>
              <tr>
                <th>Heure</th>
                <th class="num-col">Silver joueur 1</th>
                <th class="num-col">Silver joueur 2</th>
                <th class="num-col col-optional">Taxe</th>
                <th class="num-col">Objets</th>
              </tr>
            </thead>
            <tbody>
              ${trades
                .map((tr) => {
                  const timeStr =
                    tr.timestampMs > 0
                      ? new Date(tr.timestampMs).toLocaleTimeString("fr-FR", {
                          hour: "2-digit",
                          minute: "2-digit",
                          second: "2-digit",
                        })
                      : "—";
                  return `
                  <tr>
                    <td class="mono">${escapeHtml(timeStr)}</td>
                    <td class="num-col mono">${formatExactInt(tr.player1Silver)} Silver</td>
                    <td class="num-col mono">${formatExactInt(tr.player2Silver)} Silver</td>
                    <td class="num-col mono col-optional">${formatExactInt(tr.salesTax)} Silver</td>
                    <td class="num-col mono">${tr.itemCount || 0}</td>
                  </tr>
                `;
                })
                .join("")}
            </tbody>
          </table>
        </div>
      `
          : ""
      }
    </div>
  `;
};

const renderSessionFactionsPanel = (rec, activeOnly = false) => {
  const allFactions = rec.factions || [];
  const rows = activeOnly
    ? allFactions.filter((f) => (f.coins || 0) > 0 || (f.rankPoints || 0) > 0 || (f.flagPoints || 0) > 0)
    : allFactions;
  return `
    <div class="panel">
      <div class="panel-header">
        <div class="panel-heading-group">
          <h2>Guerre de Factions</h2>
          <span class="sub-label">Jetons gagnés, points de rang et points de drapeau</span>
        </div>
        <span class="badge-subtle">Cités royales</span>
      </div>
      <div class="table-wrap">
        ${
          rows.length === 0
            ? `<div class="empty-state">Aucune activité de guerre de factions enregistrée durant cette session.</div>`
            : `
          <table class="data-table">
            <thead>
              <tr>
                <th>Cité de Faction</th>
                <th class="num-col">Jetons gagnés</th>
                <th class="num-col">Points de rang</th>
                <th class="num-col col-optional">Points de drapeau</th>
              </tr>
            </thead>
            <tbody>
              ${rows
                .map(
                  (f) => `
                <tr>
                  <td>${renderCityBadge(f.city)}</td>
                  <td class="num-col mono">${formatExactInt(f.coins)}</td>
                  <td class="num-col mono">${formatExactInt(f.rankPoints)}</td>
                  <td class="num-col mono col-optional">${formatExactInt(f.flagPoints)}</td>
                </tr>
              `,
                )
                .join("")}
            </tbody>
          </table>
        `
        }
      </div>
    </div>
  `;
};

const renderSessionTab = (force = false) => {
  const snap = latestState?.snapshot;
  const listEl = el("session-list-rows");
  const headerEl = el("session-inspector-header");
  const bodyEl = el("session-inspector-body");
  const legacyNote = el("session-legacy-note");

  if (!snap) {
    if (listEl && !listEl.innerHTML) {
      listEl.innerHTML = `<div class="empty-state">En attente de télémétrie…</div>`;
    }
    if (bodyEl && !bodyEl.innerHTML) {
      bodyEl.innerHTML = `<div class="empty-state">Lancez Albion Online ou rechargez une session pour afficher l'historique complet.</div>`;
    }
    return;
  }

  const liveStats = snap.session || {};
  const lp = snap.localPlayer || {};
  el("sess-elapsed").textContent = formatDuration(liveStats.elapsedSeconds || 0);
  el("sess-packets").textContent = formatExactInt(snap.network?.packetsProcessed || 0);
  el("sess-events").textContent = formatExactInt(snap.network?.telemetryEventsProcessed || 0);

  let rec = normalizeSessionRecord(snap, selectedSessionId);
  if (rec.id !== selectedSessionId) {
    selectedSessionId = rec.id;
  }

  el("kpi-fame").textContent = formatNumber(rec.fame);
  el("kpi-fame-hr").textContent = `${formatNumber(rec.famePerHour)} / heure`;
  el("kpi-fame-extra").textContent = `Zone : ${formatNumber(rec.zoneFame)} • Sacoche : ${formatNumber(rec.satchelFame)}`;

  el("kpi-silver").textContent = `${formatNumber(rec.silver)} Silver`;
  el("kpi-silver-hr").textContent = `${formatNumber(rec.silverPerHour)} Silver / h`;
  el("kpi-silver-tax").textContent = `Taxe guilde : ${formatNumber(rec.guildTaxPaid)} • Réparations : ${formatNumber(rec.repairCosts)}`;

  el("kpi-respec").textContent = formatNumber(rec.respecPoints);
  el("kpi-respec-hr").textContent = `${formatNumber(rec.respecPerHour)} / heure`;
  el("kpi-respec-cost").textContent = `Silver payé : ${formatNumber(rec.respecPaidSilver)} Silver`;

  el("kpi-might").textContent = `${formatNumber(rec.might)} Might`;
  el("kpi-favor").textContent = `${formatNumber(rec.favor)} Faveur`;
  el("kpi-might-hr").textContent = `${formatNumber(rec.mightPerHour)} Might/h • ${formatNumber(rec.favorPerHour)} Faveur/h`;

  el("kpi-kd").textContent = `${rec.kills} K / ${rec.deaths} D`;
  if (rec.isLive) {
    el("kpi-focus").textContent = `Focus : ${formatExactInt(lp.focusPoints || 0)} / ${formatExactInt(lp.maxFocusPoints || 30000)}`;
    el("kpi-season").textContent = `Points GvG : ${rec.seasonPoints} • LP : ${formatExactInt(lp.learningPoints || 0)}`;
  } else {
    el("kpi-focus").textContent = `Récolte : ${formatExactInt(rec.gatheringTotals.totalResources)} res. (${rec.gatheringTotals.totalNodes || 0} nœuds)`;
    el("kpi-season").textContent = `${rec.zones.length} zone(s) • ${rec.dungeons.length} instance(s) • GvG : ${rec.seasonPoints}`;
  }

  const archived = snap.archivedSessions || [];
  const archBadge = el("archived-session-badge");
  if (archBadge) {
    archBadge.textContent = `${archived.length} archivée${archived.length > 1 ? "s" : ""}`;
  }

  const liveRec = normalizeSessionRecord(snap, "live");
  const sidebarKey = JSON.stringify([
    selectedSessionId,
    liveRec.mapNameFr,
    Math.floor(liveRec.durationSeconds),
    Math.round(liveRec.fame),
    Math.round(liveRec.silver),
    liveRec.gatheringTotals.totalResources,
    archived.map((a) => [
      a.id,
      a.label,
      a.endedAtMs,
      Math.round(a.durationSeconds),
      Math.round(a.fame),
      Math.round(a.silver),
      a.gatheringTotals?.totalResources || 0,
    ]),
  ]);

  if (listEl && !shouldSkipRender("session-sidebar", sidebarKey, force)) {
    const liveCard = `
      <div class="session-list-item ${selectedSessionId === "live" ? "is-selected" : ""}" role="button" tabindex="0" data-select-session="live">
        <div class="session-list-head">
          <span class="session-list-title">Session en direct</span>
          <span class="status-badge status-live"><span class="badge-dot"></span>En cours</span>
        </div>
        <div class="session-list-meta">
          <span>${escapeHtml(liveRec.mapNameFr)}</span>
          <span>•</span>
          <span class="mono">${formatDuration(liveRec.durationSeconds)}</span>
        </div>
        <div class="session-list-kpis">
          <span>${formatNumber(liveRec.fame)} Fame</span>
          <span>${formatNumber(liveRec.silver)} S</span>
          <span>${formatExactInt(liveRec.gatheringTotals.totalResources)} rés.</span>
        </div>
      </div>
    `;
    const archivedCards = archived
      .map((a) => {
        const resCount = a.gatheringTotals?.totalResources || 0;
        const zoneCount = Array.isArray(a.zones) ? a.zones.length : 0;
        return `
          <div class="session-list-item ${selectedSessionId === a.id ? "is-selected" : ""}" role="button" tabindex="0" data-select-session="${escapeHtml(a.id)}">
            <div class="session-list-head">
              <span class="session-list-title" title="${escapeHtml(a.label)}">${escapeHtml(a.label)}</span>
              <button
                class="btn btn-danger-subtle btn-xs btn-icon-xs"
                type="button"
                data-del-archived-session="${escapeHtml(a.id)}"
                title="Supprimer cette session de l'historique"
                aria-label="Supprimer la session"
              >
                <svg class="ico"><use href="#ico-trash"></use></svg>
              </button>
            </div>
            <div class="session-list-meta">
              <span>${escapeHtml(formatShortDateTime(a.endedAtMs))}</span>
              <span>•</span>
              <span class="mono">${formatDuration(a.durationSeconds)}</span>
              ${zoneCount > 0 ? `<span>• ${zoneCount} zone${zoneCount > 1 ? "s" : ""}</span>` : ""}
            </div>
            <div class="session-list-kpis">
              <span>${formatNumber(a.fame)} Fame</span>
              <span>${formatNumber(a.silver)} S</span>
              <span>${formatExactInt(resCount)} rés.</span>
            </div>
          </div>
        `;
      })
      .join("");
    listEl.innerHTML = `${liveCard}${archivedCards}`;
  }

  if (headerEl) {
    const timeSummary = rec.isLive
      ? `Zone actuelle : <strong>${escapeHtml(rec.mapNameFr)}</strong> • Durée : <span class="mono">${formatDuration(rec.durationSeconds)}</span>`
      : `Du <span class="mono">${escapeHtml(formatShortDateTime(rec.startedAtMs))}</span> au <span class="mono">${escapeHtml(formatShortDateTime(rec.endedAtMs))}</span> • Dernière zone : <strong>${escapeHtml(rec.mapNameFr)}</strong> • Durée : <span class="mono">${formatDuration(rec.durationSeconds)}</span>`;
    const statusPill = rec.isLive
      ? `<span class="status-badge status-live"><span class="badge-dot"></span>Active</span>`
      : `<span class="badge-subtle">Archivée</span>`;
    headerEl.innerHTML = `
      <div class="session-inspector-title-group">
        <div class="session-inspector-heading">
          <h2>${escapeHtml(rec.label)}</h2>
          ${statusPill}
        </div>
        <div class="session-inspector-sub">${timeSummary}</div>
      </div>
      <div class="inline-controls">
        <span class="badge-subtle">${rec.zones.length} zone(s)</span>
        <span class="badge-subtle">${formatExactInt(rec.gatheringTotals.totalResources)} récolte(s)</span>
        <span class="badge-subtle">${rec.lootLog.length} butin(s)</span>
      </div>
    `;
  }

  if (legacyNote) {
    legacyNote.hidden = !rec.isLegacySummaryOnly;
  }

  const activeFactionCount = rec.factions.filter(
    (f) => (f.coins || 0) > 0 || (f.rankPoints || 0) > 0 || (f.flagPoints || 0) > 0,
  ).length;
  el("subtab-count-zones").textContent = String(rec.zones.length);
  el("subtab-count-gathering").textContent = formatExactInt(rec.gatheringTotals.totalResources);
  el("subtab-count-combat").textContent = String(rec.combatPlayers.length);
  el("subtab-count-dungeons").textContent = String(rec.dungeons.length);
  el("subtab-count-loot").textContent = String(rec.lootLog.length + rec.trades.length);
  el("subtab-count-factions").textContent = String(activeFactionCount);

  document.querySelectorAll("#session-subtabs .seg-btn[data-session-subtab]").forEach((btn) => {
    btn.classList.toggle("active", btn.getAttribute("data-session-subtab") === activeSessionSubtab);
  });

  if (!bodyEl) return;
  const bodyKey = JSON.stringify([
    rec.id,
    activeSessionSubtab,
    [...expandedSessionCombatPlayers].sort(),
    rec.zones,
    rec.gatheringTotals,
    rec.gatheringLog,
    rec.combatPlayers,
    rec.dungeons,
    rec.lootLog,
    rec.trades,
    rec.factions,
  ]);
  if (shouldSkipRender("session-body", bodyKey, force)) {
    return;
  }

  let sectionsHtml = "";
  switch (activeSessionSubtab) {
    case "zones":
      sectionsHtml = renderSessionZonesPanel(rec);
      break;
    case "gathering":
      sectionsHtml = renderSessionGatheringPanel(rec);
      break;
    case "combat":
      sectionsHtml = renderSessionCombatPanel(rec);
      break;
    case "dungeons":
      sectionsHtml = renderSessionDungeonsPanel(rec);
      break;
    case "loot":
      sectionsHtml = renderSessionLootPanel(rec);
      break;
    case "factions":
      sectionsHtml = renderSessionFactionsPanel(rec, false);
      break;
    case "all":
    default: {
      const blocks = [];
      blocks.push(renderSessionZonesPanel(rec));
      blocks.push(renderSessionGatheringPanel(rec));
      if (rec.combatPlayers.length > 0) blocks.push(renderSessionCombatPanel(rec));
      if (rec.dungeons.length > 0) blocks.push(renderSessionDungeonsPanel(rec));
      if (rec.lootLog.length > 0 || rec.trades.length > 0) blocks.push(renderSessionLootPanel(rec));
      if (activeFactionCount > 0) blocks.push(renderSessionFactionsPanel(rec, true));
      sectionsHtml = blocks.join("");
      break;
    }
  }
  bodyEl.innerHTML = sectionsHtml;
  hydrateLoadedImages(bodyEl);
};

const renderDungeonsTab = (force = false) => {
  const runs = latestState?.snapshot?.dungeons || [];
  el("dungeon-count-badge").textContent = `${runs.length} instance${runs.length > 1 ? "s" : ""}`;
  const renderKey = JSON.stringify(runs);
  if (shouldSkipRender("dungeons", renderKey, force)) {
    return;
  }

  const wrap = el("dungeon-rows");
  if (runs.length === 0) {
    wrap.innerHTML = `<div class="empty-state">Aucun donjon ou brume parcouru durant cette session.</div>`;
    return;
  }

  wrap.innerHTML = `
    <table class="data-table">
      <thead>
        <tr>
          <th>Instance</th>
          <th>Tier</th>
          <th class="num-col">Durée</th>
          <th class="num-col">Renommée</th>
          <th class="num-col col-optional">Fame / h</th>
          <th class="num-col">Silver</th>
          <th class="num-col col-optional">Might / Faveur</th>
          <th class="num-col col-optional">Coffres / Wisps</th>
        </tr>
      </thead>
      <tbody>
        ${runs
          .map(
            (d) => `
          <tr>
            <td>
              <span class="cell-with-icon">
                <strong>${escapeHtml(d.nameFr)}</strong>
                ${d.isActive ? '<span class="status-badge status-live"><span class="badge-dot"></span>En cours</span>' : ""}
              </span>
            </td>
            <td><span class="mono-tag">T${d.tier}</span></td>
            <td class="num-col mono">${formatDuration(d.durationSeconds)}</td>
            <td class="num-col mono">${formatNumber(d.fame)}</td>
            <td class="num-col mono col-optional text-accent">${formatNumber(d.famePerHour)}/h</td>
            <td class="num-col mono">${formatNumber(d.silver)} Silver</td>
            <td class="num-col mono col-optional">${formatExactInt(d.might)} / ${formatExactInt(d.favor)}</td>
            <td class="num-col mono col-optional">${d.chestsOpened} / ${d.cagedWispsOpened}</td>
          </tr>
        `,
          )
          .join("")}
      </tbody>
    </table>
  `;
};

const renderGatheringTab = (force = false) => {
  const snap = latestState?.snapshot;
  if (!snap) {
    const gWrap = el("gathering-rows");
    if (gWrap && !gWrap.innerHTML) {
      gWrap.innerHTML = `<div class="empty-state">Aucune récolte enregistrée durant cette session.</div>`;
    }
    return;
  }
  const t = snap.gatheringTotals;
  el("gat-wood").textContent = formatExactInt(t.wood);
  el("gat-ore").textContent = formatExactInt(t.ore);
  el("gat-fiber").textContent = formatExactInt(t.fiber);
  el("gat-hide").textContent = formatExactInt(t.hide);
  el("gat-rock").textContent = formatExactInt(t.rock);
  el("gat-fish").textContent = formatExactInt(t.fish);
  el("gathering-summary-badge").textContent = `${formatExactInt(t.totalResources)} ressources (${t.totalNodes} nœuds) • ~${formatNumber(t.totalEstimatedSilver)} Silver`;

  const log = snap.gatheringLog || [];
  const renderKey = JSON.stringify(log);
  if (shouldSkipRender("gathering", renderKey, force)) {
    return;
  }

  const wrap = el("gathering-rows");
  if (log.length === 0) {
    wrap.innerHTML = `<div class="empty-state">Aucune récolte enregistrée durant cette session.</div>`;
    return;
  }

  wrap.innerHTML = `
    <table class="data-table">
      <thead>
        <tr>
          <th>Ressource</th>
          <th>Tier</th>
          <th class="num-col col-optional">Base</th>
          <th class="num-col col-optional">Récolteur</th>
          <th class="num-col col-optional">Premium</th>
          <th class="num-col">Total</th>
          <th class="num-col">Estimation</th>
        </tr>
      </thead>
      <tbody>
        ${log
          .map(
            (g) => `
          <tr>
            <td>
              <span class="cell-with-icon">
                ${renderItemThumb(g.uniqueName, "icon-slot-sm", "resource")}
                <strong>${escapeHtml(g.nameFr)}</strong>
              </span>
            </td>
            <td><span class="mono-tag">T${g.tier}.${g.enchantment}</span></td>
            <td class="num-col mono col-optional">${g.standardAmount}</td>
            <td class="num-col mono col-optional">+${g.collectorBonus}</td>
            <td class="num-col mono col-optional">+${g.premiumBonus}</td>
            <td class="num-col mono"><strong>${g.totalAmount}</strong></td>
            <td class="num-col mono text-accent">${formatExactInt(g.estimatedSilver)} Silver</td>
          </tr>
        `,
          )
          .join("")}
      </tbody>
    </table>
  `;
  hydrateLoadedImages(wrap);
};

const renderLootAndInspectTab = (force = false) => {
  const snap = latestState?.snapshot;
  const players = snap?.inspectedPlayers || [];
  const loot = snap?.lootLog || [];
  el("inspect-count-badge").textContent = `${players.length} joueur${players.length > 1 ? "s" : ""}`;

  const renderKey = JSON.stringify([players, loot]);
  if (shouldSkipRender("loot-inspect", renderKey, force)) {
    return;
  }

  const cardsWrap = el("inspect-cards");
  if (players.length === 0) {
    cardsWrap.innerHTML = `<div class="empty-state">Aucun membre du groupe détecté.</div>`;
  } else {
    cardsWrap.innerHTML = players
      .map(
        (p) => `
      <div class="inspect-card">
        <div class="inspect-head">
          <div class="inspect-ident">
            <strong>${escapeHtml(p.name)}</strong>
            ${p.guildName ? `<span class="badge-subtle">[${escapeHtml(p.guildName)}]</span>` : ""}
          </div>
          <span class="mono text-accent">~${formatNumber(p.estimatedLoadoutValue)} Silver</span>
        </div>
        <div class="slot-list">
          ${(p.slots || [])
            .map(
              (sl) => `
            <div class="slot-item">
              <span class="slot-tag">${escapeHtml(SLOT_LABELS_FR[sl.slot] || sl.slot)}</span>
              <span class="slot-item-val">
                ${renderItemThumb(sl.uniqueName, "icon-slot-xs", sl.slot)}
                <span class="truncate-text">${escapeHtml(sl.nameFr)}</span>
              </span>
            </div>
          `,
            )
            .join("")}
        </div>
      </div>
    `,
      )
      .join("");
    hydrateLoadedImages(cardsWrap);
  }

  const lootWrap = el("loot-rows");
  if (loot.length === 0) {
    lootWrap.innerHTML = `<div class="empty-state">Aucun butin ramassé.</div>`;
  } else {
    lootWrap.innerHTML = `
      <table class="data-table">
        <thead>
          <tr>
            <th>Joueur</th>
            <th class="col-optional">Source</th>
            <th>Objet</th>
            <th class="num-col">Qté</th>
            <th class="num-col">Estimation</th>
          </tr>
        </thead>
        <tbody>
          ${loot
            .map(
              (l) => `
            <tr>
              <td><strong>${escapeHtml(l.lootedBy)}</strong></td>
              <td class="col-optional text-dim">${escapeHtml(l.lootedFrom)}</td>
              <td>
                <span class="cell-with-icon">
                  ${renderItemThumb(l.uniqueName, "icon-slot-sm")}
                  <span class="truncate-text">${escapeHtml(l.nameFr)}</span>
                </span>
              </td>
              <td class="num-col mono">×${formatExactInt(l.quantity)}</td>
              <td class="num-col mono text-accent">${formatExactInt(l.estimatedSilver)} Silver</td>
            </tr>
          `,
            )
            .join("")}
        </tbody>
      </table>
    `;
    hydrateLoadedImages(lootWrap);
  }
};

const renderCapturedMarketOrders = (force = false) => {
  const snap = latestState?.snapshot;
  const orders = snap?.marketOrders || [];
  const mails = snap?.mailNotifications || [];
  const total = orders.length + mails.length;
  const badge = el("market-orders-badge");
  if (badge) badge.textContent = `${total} capture${total > 1 ? "s" : ""}`;

  const renderKey = JSON.stringify([orders, mails]);
  if (shouldSkipRender("captured-market", renderKey, force)) {
    return;
  }

  const wrap = el("captured-market-rows");
  if (orders.length === 0 && mails.length === 0) {
    wrap.innerHTML = `
      <div class="empty-state empty-market-state">
        <div class="empty-title">Aucun ordre ou courrier capturé</div>
        <div class="empty-hint">Ouvrez l'Hôtel des Ventes ou vos courriers en jeu pour capturer automatiquement vos transactions.</div>
      </div>
    `;
    return;
  }

  wrap.innerHTML = `
    <table class="data-table">
      <thead>
        <tr>
          <th>Type</th>
          <th>Objet</th>
          <th class="num-col">Qté</th>
          <th class="num-col col-optional">Prix / u</th>
          <th class="num-col">Total</th>
        </tr>
      </thead>
      <tbody>
        ${orders
          .map(
            (o) => `
          <tr>
            <td><span class="badge-subtle ${o.isRequest ? "badge-buy-order" : "badge-sell-order"}">${o.isRequest ? "Achat" : "Vente Q" + o.qualityLevel}</span></td>
            <td>
              <span class="cell-with-icon">
                ${renderItemThumbWithTier(o.itemUniqueName)}
                <strong class="truncate-text">${escapeHtml(o.itemNameFr)}</strong>
              </span>
            </td>
            <td class="num-col mono">×${o.amount}</td>
            <td class="num-col mono col-optional">${formatExactInt(o.unitPriceSilver)} Silver</td>
            <td class="num-col mono"><strong>${formatExactInt(o.totalPriceSilver)} Silver</strong></td>
          </tr>
        `,
          )
          .join("")}
        ${mails
          .map(
            (m) => `
          <tr>
            <td><span class="status-badge status-live"><span class="badge-dot"></span>Vendu</span></td>
            <td>
              <span class="cell-with-icon">
                ${renderItemThumbWithTier(m.itemUniqueName)}
                <strong class="truncate-text">${escapeHtml(m.itemNameFr)}</strong>
              </span>
            </td>
            <td class="num-col mono">×${m.amount}</td>
            <td class="num-col mono col-optional">${formatExactInt(m.unitPriceSilver)} Silver</td>
            <td class="num-col mono text-accent"><strong>${formatExactInt(m.totalPriceSilver)} Silver</strong></td>
          </tr>
        `,
          )
          .join("")}
      </tbody>
    </table>
  `;
  hydrateLoadedImages(wrap);
};

const computeCraftingFallback = (input) => {
  const qty = Math.max(1, Number(input.craftQuantity) || 1);
  const bonus = Math.max(0, Number(input.productionBonusPercent) || 0);
  const rrr = bonus > 0 ? 1 - 1 / (1 + bonus / 100) : 0;
  let grossMat = 0;
  let netMat = 0;
  for (const mat of input.materials || []) {
    const line = (Number(mat.quantityPerItem) || 0) * Math.max(0, Number(mat.unitPrice) || 0);
    grossMat += line;
    netMat += mat.isReturnable ? line * (1 - rrr) : line;
  }
  const itemValue = Math.max(0, Number(input.itemValue) || 0);
  const feeRate = Math.max(0, Number(input.usageFeePer100Nutrition) || 0);
  const stationFee = itemValue > 0 && feeRate > 0 ? itemValue * 0.1125 * (feeRate / 100) : 0;
  const marketTaxRate = (input.isPremium ? 0.04 : 0.08) + (input.isSellOrder ? 0.025 : 0);
  const sellUnit = Math.max(0, Number(input.sellUnitPrice) || 0);
  const marketFee = sellUnit * marketTaxRate;
  const totalCostPerItem = netMat + stationFee + marketFee;
  const netRevenuePerItem = sellUnit + Math.max(0, Number(input.journalProfitPerItem) || 0);
  const netProfitPerItem = netRevenuePerItem - totalCostPerItem;
  return {
    itemName: input.itemName,
    craftQuantity: qty,
    resourceReturnRate: rrr,
    grossMaterialCostPerItem: grossMat,
    netMaterialCostPerItem: netMat,
    stationFeePerItem: stationFee,
    marketTaxRate,
    marketFeePerItem: marketFee,
    totalCostPerItem,
    netRevenuePerItem,
    netProfitPerItem,
    totalBatchCost: totalCostPerItem * qty,
    totalBatchRevenue: netRevenuePerItem * qty,
    totalBatchProfit: netProfitPerItem * qty,
    marginPercent: totalCostPerItem > 0 ? (netProfitPerItem / totalCostPerItem) * 100 : 0,
  };
};

const updateCraftingSimulation = async () => {
  const craftName = (el("craft-name")?.value || "").trim();
  const matRetVal = Math.max(0, Number(el("craft-mat-ret")?.value) || 0);
  const matArtVal = Math.max(0, Number(el("craft-mat-art")?.value) || 0);
  const sellVal = Math.max(0, Number(el("craft-sell")?.value) || 0);
  const itemVal = Math.max(0, Number(el("craft-item-val")?.value) || 0);
  const feeVal = Math.max(0, Number(el("craft-fee")?.value) || 0);
  const journalVal = Math.max(0, Number(el("craft-journal")?.value) || 0);
  const qty = Math.max(1, Number(el("craft-qty")?.value) || 1);
  const bonusVal = Math.max(0, Number(el("craft-bonus")?.value) || 0);

  document.querySelectorAll(".preset-pill[data-preset-bonus]").forEach((b) => {
    const val = Number(b.getAttribute("data-preset-bonus"));
    b.classList.toggle("active", Math.abs(val - bonusVal) < 0.05);
  });

  const resDiv = el("craft-result");
  if (!resDiv) return;

  if (!craftName && sellVal === 0 && matRetVal === 0 && matArtVal === 0) {
    resDiv.innerHTML = `
      <div class="empty-state">
        Cliquez sur <strong>Craft</strong> sur un objet suivi ou renseignez le formulaire ci-dessus pour simuler les rendements (RRR), taxes et marge nette.
      </div>
    `;
    return;
  }

  const input = {
    itemName: craftName || "Objet",
    craftQuantity: qty,
    sellUnitPrice: sellVal,
    itemValue: itemVal,
    productionBonusPercent: bonusVal,
    usageFeePer100Nutrition: feeVal,
    isPremium: Boolean(el("craft-premium")?.checked),
    isSellOrder: Boolean(el("craft-sell-order")?.checked),
    journalProfitPerItem: journalVal,
    materials: [
      {
        name: "Ressources principales",
        quantityPerItem: 1,
        unitPrice: matRetVal,
        isReturnable: true,
      },
      {
        name: "Artefact / Cœur",
        quantityPerItem: 1,
        unitPrice: matArtVal,
        isReturnable: false,
      },
    ],
  };

  const res = (await invokeCmd("calculate_crafting", { input })) || computeCraftingFallback(input);
  if (!res) return;

  const isProfitable = res.totalBatchProfit >= 0;
  const rrrPct = (res.resourceReturnRate * 100).toFixed(2).replace(".", ",");
  const taxPct = (res.marketTaxRate * 100).toFixed(1).replace(".", ",");
  const netReturnablePerItem = Math.round(matRetVal * (1 - res.resourceReturnRate));
  const savedPerItem = Math.round(matRetVal * res.resourceReturnRate);
  const stationFeePerItem = Math.round(res.stationFeePerItem);
  const marketFeePerItem = Math.round(res.marketFeePerItem);
  const totalCost = Math.round(res.totalBatchCost);
  const totalRevenue = Math.round(res.totalBatchRevenue);
  const roiPercent = totalCost > 0 ? (res.totalBatchProfit / totalCost) * 100 : 0;

  const budgetBase = Math.max(totalRevenue, totalCost, 1);
  const matCostTotal = Math.round(res.netMaterialCostPerItem * res.craftQuantity);
  const feeTotal = Math.round((res.stationFeePerItem + res.marketFeePerItem) * res.craftQuantity);
  const matShare = Math.min(100, Math.max(3, (matCostTotal / budgetBase) * 100));
  const feeShare = Math.min(100, Math.max(2, (feeTotal / budgetBase) * 100));
  const profitShare = isProfitable ? Math.max(0, Math.min(100, 100 - matShare - feeShare)) : 0;

  el("craft-result").innerHTML = `
    <div class="craft-dashboard">
      <div class="craft-hero ${isProfitable ? "is-profit" : "is-loss"}">
        <div class="craft-hero-left">
          <span class="craft-hero-tag">
            <span class="hero-dot"></span>
            Profit net (${res.craftQuantity} u)
          </span>
          <div class="craft-hero-main">
            <span class="hero-num">${isProfitable ? "+" : ""}${formatExactInt(res.totalBatchProfit)}</span>
            <span class="hero-unit">Silver</span>
          </div>
          <div class="craft-hero-sub">
            <span class="mono">${isProfitable ? "+" : ""}${formatExactInt(res.netProfitPerItem)} Silver/u</span>
            <span class="hero-sep">•</span>
            <span>ROI <strong class="mono">${roiPercent >= 0 ? "+" : ""}${roiPercent.toFixed(1)}%</strong></span>
            <span class="hero-sep">•</span>
            <span>RRR <strong class="mono">${rrrPct}%</strong></span>
          </div>
        </div>
        <div class="craft-hero-badge ${isProfitable ? "badge-profit" : "badge-loss"}">
          <span class="badge-status-title">${isProfitable ? "RENTABLE" : "DÉFICITAIRE"}</span>
          <span class="badge-margin-tag mono">Marge ${res.marginPercent.toFixed(1)}%</span>
        </div>
      </div>

      <div class="cost-ledger" aria-label="Détail du calcul du coût unitaire">
        <div class="ledger-row">
          <span class="ledger-label">Ressources avec RRR <small>(${formatExactInt(matRetVal)} − ${rrrPct}% = −${formatExactInt(savedPerItem)} Silver)</small></span>
          <span class="ledger-val mono">${formatExactInt(netReturnablePerItem)} Silver</span>
        </div>
        <div class="ledger-row">
          <span class="ledger-label">Artefacts &amp; cœurs <small>(sans retour RRR)</small></span>
          <span class="ledger-val mono">+ ${formatExactInt(matArtVal)} Silver</span>
        </div>
        <div class="ledger-row">
          <span class="ledger-label">Taxe station <small>(${formatExactInt(itemVal)} × 0,1125 × ${feeVal}/100)</small></span>
          <span class="ledger-val mono">+ ${formatExactInt(stationFeePerItem)} Silver</span>
        </div>
        <div class="ledger-row">
          <span class="ledger-label">Taxes marché <small>(${taxPct}% sur ${formatExactInt(sellVal)} Silver)</small></span>
          <span class="ledger-val mono">+ ${formatExactInt(marketFeePerItem)} Silver</span>
        </div>
        <div class="ledger-row ledger-total">
          <span class="ledger-label">Coût complet / u <small>(lot ×${res.craftQuantity} : ${formatExactInt(totalCost)} Silver)</small></span>
          <strong class="ledger-val mono">${formatExactInt(res.totalCostPerItem)} Silver</strong>
        </div>
        <div class="ledger-row ledger-revenue">
          <span class="ledger-label">Recette vente + journal / u <small>(lot ×${res.craftQuantity} : ${formatExactInt(totalRevenue)} Silver)</small></span>
          <strong class="ledger-val mono text-accent">${formatExactInt(res.netRevenuePerItem)} Silver</strong>
        </div>
      </div>

      <div class="craft-breakdown-box">
        <div class="breakdown-bar">
          <div class="breakdown-seg seg-mat" style="width: ${matShare.toFixed(1)}%" title="Matériaux nets : ${matShare.toFixed(1)}%"></div>
          <div class="breakdown-seg seg-fee" style="width: ${feeShare.toFixed(1)}%" title="Station &amp; taxes : ${feeShare.toFixed(1)}%"></div>
          ${isProfitable ? `<div class="breakdown-seg seg-profit" style="width: ${profitShare.toFixed(1)}%" title="Profit net : ${profitShare.toFixed(1)}%"></div>` : ""}
        </div>
        <div class="breakdown-legend">
          <span class="leg-entry"><span class="leg-dot dot-mat"></span> Matériaux ${matShare.toFixed(0)}%</span>
          <span class="leg-entry"><span class="leg-dot dot-fee"></span> Taxes ${feeShare.toFixed(0)}%</span>
          ${isProfitable ? `<span class="leg-entry"><span class="leg-dot dot-profit"></span> Marge ${profitShare.toFixed(0)}%</span>` : ""}
        </div>
      </div>
    </div>
  `;
};

const PRICE_CHUNK_SIZE = 40;

const resolveItemInfo = async (uniqueName) => {
  if (itemInfoCache.has(uniqueName)) return itemInfoCache.get(uniqueName);
  let info = null;
  try {
    const found = await invokeCmd("search_catalog_items", { query: uniqueName, limit: 1 });
    if (Array.isArray(found) && found[0]?.uniqueName === uniqueName) {
      info = found[0];
    }
  } catch (e) {
    console.warn("search_catalog_items:", e);
  }
  itemInfoCache.set(uniqueName, info);
  return info;
};

const fetchPriceRows = async (items, qualities = [1, 2, 3]) => {
  const chunks = [];
  for (let i = 0; i < items.length; i += PRICE_CHUNK_SIZE) {
    chunks.push(items.slice(i, i + PRICE_CHUNK_SIZE));
  }
  const responses = await Promise.all(
    chunks.map(async (chunk) => {
      const url = buildAlbionDataPriceUrl(settings.region, chunk, settings.selectedCities, qualities);
      const resp = await fetch(url);
      if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
      return resp.json();
    }),
  );
  return summarizeMarketPrices(responses.flat());
};

const fetchAlbionDataPrices = async (customItems = null, qtyByItem = null) => {
  const wrap = el("api-price-rows");
  const banner = el("session-valuation-banner");
  const isSession = Boolean(qtyByItem);
  const items = (customItems || settings.trackedItems || []).filter(Boolean);
  if (!isSession && banner) banner.hidden = true;
  if (items.length === 0) {
    lastPriceRows = [];
    wrap.innerHTML = `<div class="empty-state">Aucun objet suivi. Recherchez un objet ci-dessus pour l'ajouter.</div>`;
    return;
  }
  wrap.innerHTML = `<div class="empty-state">Interrogation de l'API Albion Data Project (${escapeHtml(settings.region)}, ${items.length} objet(s))…</div>`;
  try {
    const [summarized, infos] = await Promise.all([
      fetchPriceRows(items),
      Promise.all(items.map((id) => resolveItemInfo(id))),
    ]);
    const infoById = new Map(items.map((id, i) => [id, infos[i]]));
    const byId = new Map(summarized.map((r) => [r.itemId, r]));
    const empty = {
      bestSellPrice: 0,
      bestSellCity: "-",
      bestBuyOrder: 0,
      bestBuyCity: "-",
      spreadSilver: 0,
      spreadPercent: 0,
    };
    const rows = isSession
      ? items.filter((id) => byId.has(id)).map((id) => byId.get(id))
      : items.map((id) => byId.get(id) || { itemId: id, ...empty });
    lastPriceRows = rows.map((r) => ({ ...r, info: infoById.get(r.itemId) || null }));

    if (isSession && banner) {
      const { total, pricedItems } = computeSessionMarketValue(summarized, qtyByItem);
      const estimated =
        (latestState?.snapshot?.lootLog || []).reduce(
          (acc, l) => acc + (Number(l.estimatedSilver) || 0),
          0,
        ) + (latestState?.snapshot?.gatheringTotals?.totalEstimatedSilver || 0);
      banner.innerHTML = `Valeur marché réelle de la session : <strong class="mono">${formatExactInt(total)} Silver</strong> • ${pricedItems}/${items.length} objet(s) cotés • estimation catalogue : <span class="mono">${formatExactInt(estimated)} Silver</span>`;
      banner.hidden = false;
    }

    if (lastPriceRows.length === 0) {
      wrap.innerHTML = `<div class="empty-state">Aucun prix récent retourné pour cette région.</div>`;
      return;
    }
    wrap.innerHTML = `
      <table class="data-table market-data-table">
        <thead>
          <tr>
            <th>Objet</th>
            ${isSession ? '<th class="num-col">Qté</th>' : ""}
            <th>Meilleure vente</th>
            <th>Meilleur achat</th>
            <th class="col-optional">Arbitrage</th>
            <th class="num-col"></th>
          </tr>
        </thead>
        <tbody>
          ${lastPriceRows
            .map((r) => {
              const name = r.info?.nameFr || r.itemId;
              const hasSell = r.bestSellPrice > 0;
              const hasBuy = r.bestBuyOrder > 0;
              const hasSpread = hasSell && hasBuy;
              const isProfitSpread = hasSpread && r.spreadSilver > 0;
              const thumb = renderItemThumbWithTier(r.itemId, r.info?.slot || "");

              let arbHtml = `<span class="dash-empty">—</span>`;
              if (hasSpread) {
                arbHtml = `
                  <span class="arbitrage-pill mono ${isProfitSpread ? "arb-profit" : "arb-loss"}">
                    ${isProfitSpread ? "+" : ""}${formatExactInt(r.spreadSilver)} Silver (${isProfitSpread ? "+" : ""}${r.spreadPercent.toFixed(1)}%)
                  </span>
                `;
              }

              return `
            <tr>
              <td>
                <div class="cell-with-icon item-cell">
                  ${thumb}
                  <div class="item-text-group">
                    <strong class="item-display-name">${escapeHtml(name)}</strong>
                    ${name !== r.itemId ? `<span class="item-id-sub mono">${escapeHtml(r.itemId)}</span>` : ""}
                  </div>
                </div>
              </td>
              ${isSession ? `<td class="num-col mono">×${formatExactInt(qtyByItem[r.itemId] || 0)}</td>` : ""}
              <td>
                ${
                  hasSell
                    ? `
                  <div class="price-stack">
                    <span class="silver-amount mono">${formatExactInt(r.bestSellPrice)} <span class="currency-tag">Silver</span></span>
                    ${renderCityBadge(r.bestSellCity)}
                  </div>
                `
                    : `<span class="dash-empty">—</span>`
                }
              </td>
              <td>
                ${
                  hasBuy
                    ? `
                  <div class="price-stack">
                    <span class="silver-amount mono">${formatExactInt(r.bestBuyOrder)} <span class="currency-tag">Silver</span></span>
                    ${renderCityBadge(r.bestBuyCity)}
                  </div>
                `
                    : `<span class="dash-empty">—</span>`
                }
              </td>
              <td class="col-optional">${arbHtml}</td>
              <td class="num-col">
                <div class="row-actions">
                  <button class="btn btn-craft btn-xs" type="button" data-craft-item="${escapeHtml(r.itemId)}" title="Pré-remplir le calculateur d'artisanat avec cet objet">
                    <svg class="ico"><use href="#ico-hammer"></use></svg>
                    <span>Craft</span>
                  </button>
                  ${
                    isSession
                      ? ""
                      : `<button class="btn btn-danger-subtle btn-xs btn-icon-xs" type="button" data-untrack-item="${escapeHtml(r.itemId)}" title="Retirer des objets suivis" aria-label="Retirer">
                          <svg class="ico"><use href="#ico-close"></use></svg>
                        </button>`
                  }
                </div>
              </td>
            </tr>
          `;
            })
            .join("")}
        </tbody>
      </table>
    `;
    hydrateLoadedImages(wrap);
  } catch (err) {
    wrap.innerHTML = `<div class="empty-state">API hors-ligne ou indisponible (${escapeHtml(err.message || err)}).</div>`;
  }
};

const renderRecipeBreakdown = (recipe, evalRes = null) => {
  const box = el("craft-recipe-box");
  if (!box || !recipe) return;
  const amountCrafted = Math.max(1, Number(recipe.amountCrafted) || 1);
  const focusVal = Number(recipe.craftingFocus) || 0;
  const lines = evalRes
    ? evalRes.lines
    : (recipe.ingredients || []).map((ing) => ({
        uniqueName: ing.uniqueName,
        nameFr: ing.nameFr || ing.uniqueName,
        count: ing.count,
        isReturnable: Boolean(ing.isReturnable),
        unitPrice: 0,
        city: "-",
        linePerCraftedUnit: 0,
      }));

  box.innerHTML = `
    <div class="recipe-header">
      <span class="recipe-title">Recette items.xml <span class="mono recipe-id-tag">${escapeHtml(recipe.itemUniqueName)}</span></span>
      <div class="recipe-badges">
        ${amountCrafted > 1 ? `<span class="badge-subtle">Lot ×${amountCrafted} (/ u)</span>` : ""}
        ${focusVal > 0 ? `<span class="badge-subtle">Focus ${formatExactInt(focusVal)}</span>` : ""}
      </div>
    </div>
    <div class="recipe-ing-list">
      ${lines
        .map(
          (ing) => `
        <div class="recipe-ing-row">
          <div class="recipe-ing-left">
            ${renderItemThumb(ing.uniqueName, "icon-slot-xs", "material")}
            <span class="recipe-ing-qty mono">${formatExactInt(ing.count)}×</span>
            <span class="recipe-ing-name" title="${escapeHtml(ing.uniqueName)}">${escapeHtml(ing.nameFr)}</span>
            <span class="rrr-tag ${ing.isReturnable ? "is-ret" : "is-nonret"}">${ing.isReturnable ? "RRR" : "0% RRR"}</span>
          </div>
          <div class="recipe-ing-right">
            ${
              ing.unitPrice > 0
                ? `
              ${renderCityBadge(ing.city)}
              <span class="recipe-unit-price mono">${formatExactInt(ing.unitPrice)} <span class="currency-tag">Silver</span></span>
              <strong class="recipe-line-total mono">${formatExactInt(ing.linePerCraftedUnit)} Silver</strong>
            `
                : `<span class="rrr-tag is-missing">Non coté</span>`
            }
          </div>
        </div>
      `,
        )
        .join("")}
    </div>
  `;
  box.hidden = false;
  hydrateLoadedImages(box);
};

const loadAndPriceCraftingRecipe = async (ctx, showFeedbackToast = false) => {
  if (!ctx?.itemId) return;
  const seq = ++recipePriceSeq;
  const note = el("craft-prefill-note");
  const refreshBtn = el("btn-refresh-recipe-prices");
  const recipeBox = el("craft-recipe-box");

  let recipe = null;
  try {
    recipe = await invokeCmd("get_crafting_recipe", { uniqueName: ctx.itemId });
  } catch (e) {
    console.warn("get_crafting_recipe:", e);
  }
  if (seq !== recipePriceSeq) return;

  if (!recipe || !Array.isArray(recipe.ingredients) || recipe.ingredients.length === 0) {
    if (refreshBtn) refreshBtn.hidden = true;
    if (recipeBox) recipeBox.hidden = true;
    const est = estimateCraftingDefaults(ctx.itemId, ctx.price, ctx.itemVal);
    if (el("craft-mat-ret")) el("craft-mat-ret").value = String(est.returnableCost);
    if (el("craft-mat-art")) el("craft-mat-art").value = String(est.artefactCost);
    if (el("craft-journal")) el("craft-journal").value = String(est.journalProfit);
    if (note) {
      note.textContent = `Pré-rempli pour ${ctx.name} — aucune recette d'établi indexée, coûts estimés`;
      note.classList.add("is-highlighted");
    }
    await updateCraftingSimulation();
    if (showFeedbackToast) {
      showToast(`Calculateur mis à jour : ${ctx.name}${ctx.price > 0 ? ` (${formatExactInt(ctx.price)} Silver)` : ""}`);
    }
    return;
  }

  if (refreshBtn) {
    refreshBtn.hidden = false;
    refreshBtn.disabled = true;
  }
  const effectiveItemVal = recipe.itemValue > 0 ? recipe.itemValue : ctx.itemVal;
  if (effectiveItemVal > 0 && el("craft-item-val")) {
    el("craft-item-val").value = String(effectiveItemVal);
  }
  const est = estimateCraftingDefaults(ctx.itemId, ctx.price, effectiveItemVal);
  if (el("craft-journal")) el("craft-journal").value = String(est.journalProfit);

  if (recipeBox) {
    recipeBox.innerHTML = `<div class="recipe-loading">Cotations matériaux Albion Data (${escapeHtml(settings.region)}) en cours pour <strong>${escapeHtml(ctx.name)}</strong>…</div>`;
    recipeBox.hidden = false;
  }

  try {
    const ingIds = [...new Set(recipe.ingredients.map((i) => i.uniqueName))];
    const matSummarized = await fetchPriceRows(ingIds, [1, 2]);
    if (seq !== recipePriceSeq) return;

    const evalRes = computeRecipeMaterialCosts(recipe, matSummarized);
    if (el("craft-mat-ret")) el("craft-mat-ret").value = String(evalRes.returnablePerUnit);
    if (el("craft-mat-art")) el("craft-mat-art").value = String(evalRes.artefactCost ?? evalRes.nonReturnablePerUnit);

    renderRecipeBreakdown(recipe, evalRes);

    if (note) {
      note.textContent =
        evalRes.missingCount === 0
          ? `Recette exacte items.xml (${settings.region}) — ${evalRes.pricedCount}/${evalRes.lines.length} matériaux cotés en direct`
          : `Recette items.xml (${settings.region}) — ${evalRes.pricedCount}/${evalRes.lines.length} matériaux cotés (${evalRes.missingCount} non coté(s), vérifiez le coût)`;
      note.classList.add("is-highlighted");
    }
    await updateCraftingSimulation();
    if (showFeedbackToast) {
      showToast(
        `Recette chargée : ${ctx.name} — RRR ${formatExactInt(evalRes.returnablePerUnit)} Silver${evalRes.nonReturnablePerUnit > 0 ? ` • Artefact ${formatExactInt(evalRes.nonReturnablePerUnit)} Silver` : ""}`,
      );
    }
  } catch (err) {
    if (seq !== recipePriceSeq) return;
    if (el("craft-mat-ret")) el("craft-mat-ret").value = String(est.returnableCost);
    if (el("craft-mat-art")) el("craft-mat-art").value = String(est.artefactCost);
    renderRecipeBreakdown(recipe, null);
    if (note) {
      note.textContent = `Recette items.xml chargée (${ctx.name}) — API matériaux hors-ligne (${err.message || err}), coûts estimés`;
      note.classList.add("is-highlighted");
    }
    await updateCraftingSimulation();
  } finally {
    if (seq === recipePriceSeq && refreshBtn) refreshBtn.disabled = false;
  }
};

const prefillCraftingFromItem = (itemId) => {
  const row = lastPriceRows.find((r) => r.itemId === itemId);
  if (!row) return;
  const name = row.info?.nameFr || itemId;
  const price = row.bestSellPrice > 0 ? row.bestSellPrice : row.bestBuyOrder;
  const itemVal = row.info?.itemValue > 0 ? row.info.itemValue : Number(el("craft-item-val")?.value) || 0;

  el("craft-name").value = name;
  if (price > 0) el("craft-sell").value = String(Math.round(price));
  if (row.info?.itemValue > 0) el("craft-item-val").value = String(row.info.itemValue);

  activeCraftItemContext = { itemId, name, price, itemVal };
  el("craft-form")?.scrollIntoView({ behavior: "smooth", block: "nearest" });
  loadAndPriceCraftingRecipe(activeCraftItemContext, true);
};

const addTrackedItem = async (item) => {
  if (!item?.uniqueName) return;
  itemInfoCache.set(item.uniqueName, item);
  const next = [item.uniqueName, ...settings.trackedItems.filter((id) => id !== item.uniqueName)];
  settings = normalizeSettings({ ...settings, trackedItems: next });
  await persistSettings();
  showToast(`${item.nameFr || item.uniqueName} ajouté aux objets suivis.`);
  await fetchAlbionDataPrices();
};

const removeTrackedItem = async (itemId) => {
  const remaining = settings.trackedItems.filter((id) => id !== itemId);
  settings = normalizeSettings({ ...settings, trackedItems: remaining });
  await persistSettings();
  showToast("Objet retiré des objets suivis.");
  await fetchAlbionDataPrices();
};

const hideSearchResults = () => {
  const box = el("market-search-results");
  if (box) box.hidden = true;
};

const runMarketSearch = async (query) => {
  const box = el("market-search-results");
  if (!box) return;
  const q = query.trim();
  if (q.length < 2) {
    hideSearchResults();
    return;
  }
  const seq = ++searchSeq;
  let results = [];
  try {
    results = (await invokeCmd("search_catalog_items", { query: q, limit: 12 })) || [];
  } catch (e) {
    console.warn("search_catalog_items:", e);
  }
  if (seq !== searchSeq) return;
  if (results.length === 0) {
    box.innerHTML = `<div class="search-result-empty">Aucun objet trouvé. Synchronisez la base complète via le bouton Catalogue.</div>`;
  } else {
    for (const it of results) itemInfoCache.set(it.uniqueName, it);
    box.innerHTML = results
      .map(
        (it) => `
        <button class="search-result-item" type="button" data-add-item="${escapeHtml(it.uniqueName)}">
          ${renderItemThumb(it.uniqueName, "icon-slot-sm", it.slot)}
          <span class="search-result-meta">
            <strong>${escapeHtml(it.nameFr || it.nameEn || it.uniqueName)}</strong>
            <small class="mono">${escapeHtml(it.uniqueName)} • T${it.tier}.${it.enchantment}</small>
          </span>
        </button>
      `,
      )
      .join("");
    hydrateLoadedImages(box);
  }
  box.hidden = false;
};

const syncCatalogFromGitHub = async ({ silent = false } = {}) => {
  if (catalogSyncInFlight) return;
  catalogSyncInFlight = true;
  const btn = el("btn-sync-catalog");
  if (btn) btn.disabled = true;
  if (!silent) {
    showToast("Synchronisation de la base Albion (ao-bin-dumps : objets, recettes, zones et sorts)…");
  }
  try {
    const base = "https://raw.githubusercontent.com/ao-data/ao-bin-dumps/master";
    const [itemsResp, worldResp, spellsResp, itemsXmlResp] = await Promise.all([
      fetch(`${base}/formatted/items.json`),
      fetch(`${base}/formatted/world.json`).catch(() => null),
      fetch(`${base}/spells.xml`).catch(() => null),
      fetch(`${base}/items.xml`).catch(() => null),
    ]);
    if (!itemsResp.ok) throw new Error(`HTTP ${itemsResp.status}`);
    const itemsText = await itemsResp.text();
    const worldText = worldResp && worldResp.ok ? await worldResp.text() : null;
    const spellsText = spellsResp && spellsResp.ok ? await spellsResp.text() : null;
    const itemsXmlText = itemsXmlResp && itemsXmlResp.ok ? await itemsXmlResp.text() : null;
    const added = await invokeCmd("import_ao_bin_dumps", {
      jsonText: itemsText,
      worldJson: worldText,
      spellsXml: spellsText,
      itemsXml: itemsXmlText,
    });
    itemInfoCache.clear();
    await refreshState(true);
    const spellCount = latestState?.snapshot?.catalogSpellCount || 0;
    const clusterCount = latestState?.snapshot?.catalogClusterCount || 0;
    const recipeCount = latestState?.snapshot?.catalogRecipeCount || 0;
    if (!silent || (added || 0) > 0) {
      showToast(
        silent
          ? `Base Albion complétée automatiquement (${formatExactInt(clusterCount)} zones, ${formatExactInt(spellCount)} sorts, ${formatExactInt(recipeCount)} recettes).`
          : `Catalogue synchronisé (+${formatExactInt(added || 0)} entrées, ${formatExactInt(clusterCount)} zones, ${formatExactInt(spellCount)} sorts, ${formatExactInt(recipeCount)} recettes).`,
      );
    }
  } catch (err) {
    if (silent) {
      console.warn("Synchronisation auto ao-bin-dumps ignorée :", err);
    } else {
      showToast(`Échec synchronisation ao-bin-dumps : ${err.message || err}`);
    }
  } finally {
    catalogSyncInFlight = false;
    if (btn) btn.disabled = false;
  }
};

const renderActiveTab = (force = false) => {
  switch (activeTab) {
    case "session":
      renderSessionTab(force);
      break;
    case "dungeons":
      renderDungeonsTab(force);
      break;
    case "gathering":
      renderGatheringTab(force);
      break;
    case "loot":
      renderLootAndInspectTab(force);
      break;
    case "economy":
      renderCapturedMarketOrders(force);
      break;
    case "combat":
    default:
      renderSnapshotSelect();
      renderCombatTab(force);
      break;
  }
};

const renderAll = (force = false) => {
  renderHeader();
  renderSnapshotSelect();
  renderCombatTab(force);
  renderSessionTab(force);
  renderDungeonsTab(force);
  renderGatheringTab(force);
  renderLootAndInspectTab(force);
  renderCapturedMarketOrders(force);
};

const computeStateSignature = (res) => {
  const s = res?.snapshot;
  const c = res?.capture;
  if (!s) return "";
  return [
    s.network?.packetsProcessed || 0,
    s.network?.telemetryEventsProcessed || 0,
    c?.rawDatagramsSeen || 0,
    c?.mode || "",
    c?.needsElevation ? 1 : 0,
    s.catalogItemCount || 0,
    s.catalogSpellCount || 0,
    s.catalogRecipeCount || 0,
    s.catalogClusterCount || 0,
    s.catalogWeaponSpellCount || 0,
    s.localPlayer?.username || "",
    s.localPlayer?.currentMapIndex || "",
    s.localPlayer?.currentMapNameFr || "",
    Math.round(s.localPlayer?.silver || 0),
    Math.round(s.session?.fame || 0),
    Math.round(s.session?.silver || 0),
    s.gatheringTotals?.totalResources || 0,
    s.dungeons?.length || 0,
    s.lootLog?.length || 0,
    s.inspectedPlayers?.length || 0,
    s.liveSessionZones?.length || 0,
    s.marketOrders?.length || 0,
    s.mailNotifications?.length || 0,
    s.trades?.length || 0,
    s.snapshots?.length || 0,
    s.archivedSessions?.length || 0,
    s.localPlayer?.inCombat ? Math.floor(s.combatSeconds || 0) : 0,
  ].join(":");
};

const refreshState = async (force = false) => {
  const res = await invokeCmd("get_state");
  if (!res) return;
  const sig = computeStateSignature(res);
  latestState = res;
  if (!force && sig === lastStateSignature) {
    return;
  }
  lastStateSignature = sig;
  if (force) {
    renderAll(true);
  } else {
    renderHeader();
    renderActiveTab(false);
  }
};

const initEvents = () => {
  // Navigation tabs
  document.querySelectorAll(".nav-tab").forEach((btn) => {
    btn.addEventListener("click", () => {
      const target = btn.getAttribute("data-tab") || "combat";
      activeTab = target;
      document.querySelectorAll(".nav-tab").forEach((b) => b.classList.toggle("active", b === btn));
      document.querySelectorAll(".tab-panel").forEach((p) => {
        p.classList.toggle("active", p.id === `tab-${target}`);
      });
      renderActiveTab(false);
    });
  });

  // Combat meter row selection delegation (avoids DOM rebuild listener churn)
  el("meter-rows")?.addEventListener("click", (ev) => {
    const row = ev.target.closest(".meter-row");
    if (!row) return;
    selectedPlayerName = row.getAttribute("data-player-name");
    renderCombatTab(true);
  });

  // Meter mode segmented control
  document.querySelectorAll(".seg-btn[data-meter-mode]").forEach((btn) => {
    btn.addEventListener("click", () => {
      settings.meterMode = btn.getAttribute("data-meter-mode") || "damage";
      document
        .querySelectorAll(".seg-btn[data-meter-mode]")
        .forEach((b) => b.classList.toggle("active", b === btn));
      persistSettings();
      renderCombatTab(true);
    });
  });

  // Snapshot selector & delete button
  el("snapshot-select")?.addEventListener("change", (ev) => {
    selectedSnapshotId = ev.target.value || "live";
    renderSnapshotSelect();
    renderCombatTab(true);
  });

  el("btn-delete-snapshot")?.addEventListener("click", async () => {
    if (selectedSnapshotId === "live") return;
    const deleted = await invokeCmd("delete_snapshot", { id: selectedSnapshotId });
    if (deleted) {
      selectedSnapshotId = "live";
      await refreshState(true);
      showToast("Snapshot supprimé.");
    }
  });

  // Titlebar controls
  el("btn-elevate")?.addEventListener("click", async () => {
    try {
      await invokeCmd("relaunch_as_admin");
    } catch (err) {
      showToast(`${err}`);
    }
  });

  el("btn-sync-catalog")?.addEventListener("click", () => {
    syncCatalogFromGitHub({ silent: false });
  });

  el("btn-compact")?.addEventListener("click", async () => {
    settings.compactMode = !settings.compactMode;
    await applyWindowPreferences();
    await persistSettings();
  });

  el("btn-min")?.addEventListener("click", () => tauriWindow?.minimize?.());
  el("btn-max")?.addEventListener("click", () => toggleMaximizeWindow());
  el("btn-close")?.addEventListener("click", () => tauriWindow?.close?.());

  document.querySelector(".titlebar")?.addEventListener("dblclick", (ev) => {
    if (ev.target.closest("button, input, select, label")) return;
    if (!tauriWindow) {
      toggleMaximizeWindow();
    } else {
      setTimeout(() => syncMaximizedState(), 80);
    }
  });

  window.addEventListener("resize", () => {
    syncMaximizedState();
  });

  // Combat toolbar buttons
  el("btn-copy-meter")?.addEventListener("click", async () => {
    const data = activeMeterDataset();
    const text = formatMeterClipboard(
      data.players,
      settings.meterMode,
      settings.copyStyle,
      data.mapName,
      data.combatSeconds,
    );
    try {
      await navigator.clipboard.writeText(text);
      showToast("Classement copié dans le presse-papiers.");
    } catch {
      showToast(text);
    }
  });

  el("btn-snapshot-meter")?.addEventListener("click", async () => {
    const snap = await invokeCmd("take_snapshot", { label: null });
    if (snap) {
      await refreshState(true);
      showToast(`Snapshot enregistré : ${snap.label}`);
    } else {
      showToast("Aucun combat actif à sauvegarder.");
    }
  });

  el("btn-reset-meter")?.addEventListener("click", async () => {
    const res = await invokeCmd("reset_meter");
    if (res) {
      latestState = res;
      selectedSnapshotId = "live";
      lastStateSignature = computeStateSignature(res);
      renderAll(true);
      showToast("Compteur de combat réinitialisé.");
    }
  });

  el("btn-archive-session")?.addEventListener("click", async () => {
    const archived = await invokeCmd("archive_session", { label: null });
    if (archived) {
      selectedSessionId = archived.id || "live";
      await refreshState(true);
      showToast(
        `Session enregistrée : ${archived.label} (${formatNumber(archived.fame)} Fame • ${formatNumber(archived.silver)} Silver).`,
      );
    } else {
      showToast("Aucune activité de session à enregistrer pour le moment.");
    }
  });

  const handleSelectSessionRow = (row) => {
    const id = row?.getAttribute("data-select-session");
    if (!id) return;
    selectedSessionId = id;
    renderSessionTab(true);
  };

  el("session-list-rows")?.addEventListener("click", async (ev) => {
    const delBtn = ev.target.closest("[data-del-archived-session]");
    if (delBtn) {
      ev.stopPropagation();
      const id = delBtn.getAttribute("data-del-archived-session");
      if (!id) return;
      const deleted = await invokeCmd("delete_archived_session", { id });
      if (deleted) {
        if (selectedSessionId === id) {
          selectedSessionId = "live";
        }
        await refreshState(true);
        showToast("Session archivée supprimée.");
      }
      return;
    }
    const row = ev.target.closest("[data-select-session]");
    if (row) handleSelectSessionRow(row);
  });

  el("session-list-rows")?.addEventListener("keydown", (ev) => {
    if (ev.key !== "Enter" && ev.key !== " ") return;
    if (ev.target.closest("button")) return;
    const row = ev.target.closest("[data-select-session]");
    if (row) {
      ev.preventDefault();
      handleSelectSessionRow(row);
    }
  });

  el("session-subtabs")?.addEventListener("click", (ev) => {
    const btn = ev.target.closest(".seg-btn[data-session-subtab]");
    if (!btn) return;
    activeSessionSubtab = btn.getAttribute("data-session-subtab") || "all";
    renderSessionTab(true);
  });

  el("session-inspector-body")?.addEventListener("click", (ev) => {
    const btn = ev.target.closest("[data-toggle-session-player]");
    if (!btn) return;
    const playerName = btn.getAttribute("data-toggle-session-player");
    if (!playerName) return;
    const key = sessionPlayerExpandKey(selectedSessionId, playerName);
    if (expandedSessionCombatPlayers.has(key)) {
      expandedSessionCombatPlayers.delete(key);
    } else {
      expandedSessionCombatPlayers.add(key);
    }
    renderSessionTab(true);
  });

  el("btn-reset-session")?.addEventListener("click", async () => {
    const res = await invokeCmd("reset_session");
    if (res) {
      latestState = res;
      selectedSessionId = "live";
      expandedSessionCombatPlayers.clear();
      lastStateSignature = computeStateSignature(res);
      renderAll(true);
      showToast("Session réinitialisée (Fame, Silver, Donjons, Récolte et Butin remis à zéro).");
    }
  });

  el("btn-clear-dungeons")?.addEventListener("click", async () => {
    const res = await invokeCmd("clear_dungeons");
    if (res) {
      latestState = res;
      lastStateSignature = computeStateSignature(res);
      renderAll(true);
      showToast("Historique des donjons et brumes effacé.");
    }
  });

  el("btn-clear-gathering")?.addEventListener("click", async () => {
    const res = await invokeCmd("clear_gathering");
    if (res) {
      latestState = res;
      lastStateSignature = computeStateSignature(res);
      renderAll(true);
      showToast("Journal et totaux de récolte effacés.");
    }
  });

  el("btn-clear-loot")?.addEventListener("click", async () => {
    const res = await invokeCmd("clear_loot");
    if (res) {
      latestState = res;
      lastStateSignature = computeStateSignature(res);
      renderAll(true);
      showToast("Journal de butin effacé.");
    }
  });

  el("btn-clear-market")?.addEventListener("click", async () => {
    const res = await invokeCmd("clear_market_data");
    if (res) {
      latestState = res;
      lastStateSignature = computeStateSignature(res);
      renderAll(true);
      showToast("Ordres de marché et courriers capturés effacés.");
    }
  });

  el("btn-clear-tracked")?.addEventListener("click", async () => {
    if (!settings.trackedItems || settings.trackedItems.length === 0) {
      showToast("Aucun objet suivi à effacer.");
      return;
    }
    settings.trackedItems = [];
    lastPriceRows = [];
    await persistSettings();
    await fetchAlbionDataPrices();
    showToast("Tous les objets suivis ont été retirés.");
  });

  el("btn-export-csv")?.addEventListener("click", async () => {
    const csv = await invokeCmd("export_loot_csv");
    if (csv) {
      try {
        await navigator.clipboard.writeText(csv);
        showToast("Journal de butin CSV copié dans le presse-papiers.");
      } catch {
        showToast("Export CSV prêt.");
      }
    }
  });

  // Crafting calculator reactive inputs
  el("craft-form")?.addEventListener("input", (ev) => {
    if (ev.target?.id === "craft-name") {
      activeCraftItemContext = null;
      const refreshBtn = el("btn-refresh-recipe-prices");
      const recipeBox = el("craft-recipe-box");
      if (refreshBtn) refreshBtn.hidden = true;
      if (recipeBox) recipeBox.hidden = true;
    }
    updateCraftingSimulation();
  });

  el("btn-refresh-recipe-prices")?.addEventListener("click", () => {
    if (activeCraftItemContext) {
      loadAndPriceCraftingRecipe(activeCraftItemContext, true);
    }
  });

  el("market-region")?.addEventListener("change", (ev) => {
    settings.region = ev.target.value || "europe";
    persistSettings();
    if (activeCraftItemContext) {
      loadAndPriceCraftingRecipe(activeCraftItemContext, false);
    }
  });

  el("btn-fetch-prices")?.addEventListener("click", () => {
    fetchAlbionDataPrices();
  });

  // Preset bonus buttons
  document.querySelectorAll(".preset-pill[data-preset-bonus]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const b = btn.getAttribute("data-preset-bonus");
      const inp = el("craft-bonus");
      if (inp && b !== null) {
        inp.value = b;
        updateCraftingSimulation();
      }
    });
  });

  // Free-text market search over the (synced) catalog
  const searchInput = el("market-search-input");
  const searchClearBtn = el("market-search-clear");
  searchInput?.addEventListener("input", (ev) => {
    const q = ev.target.value || "";
    if (searchClearBtn) searchClearBtn.hidden = q.length === 0;
    if (searchTimer) clearTimeout(searchTimer);
    searchTimer = setTimeout(() => runMarketSearch(q), 220);
  });
  searchClearBtn?.addEventListener("click", () => {
    if (searchInput) {
      searchInput.value = "";
      searchClearBtn.hidden = true;
      hideSearchResults();
      searchInput.focus();
    }
  });
  searchInput?.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape") {
      hideSearchResults();
    } else if (ev.key === "Enter") {
      ev.preventDefault();
      el("market-search-results")?.querySelector("[data-add-item]")?.click();
    }
  });
  searchInput?.addEventListener("focus", () => {
    if ((searchInput.value || "").trim().length >= 2) runMarketSearch(searchInput.value);
  });
  el("market-search-results")?.addEventListener("mousedown", (ev) => {
    if (ev.target.closest("[data-add-item]")) ev.preventDefault();
  });
  el("market-search-results")?.addEventListener("click", (ev) => {
    const btn = ev.target.closest("[data-add-item]");
    if (!btn) return;
    const id = btn.getAttribute("data-add-item");
    hideSearchResults();
    if (searchInput) searchInput.value = "";
    if (searchClearBtn) searchClearBtn.hidden = true;
    addTrackedItem(itemInfoCache.get(id) || { uniqueName: id });
  });
  document.addEventListener("click", (ev) => {
    if (!ev.target.closest(".market-search-field")) hideSearchResults();
  });

  el("api-price-rows")?.addEventListener("click", (ev) => {
    const craftBtn = ev.target.closest("[data-craft-item]");
    if (craftBtn) {
      prefillCraftingFromItem(craftBtn.getAttribute("data-craft-item"));
      return;
    }
    const untrackBtn = ev.target.closest("[data-untrack-item]");
    if (untrackBtn) {
      removeTrackedItem(untrackBtn.getAttribute("data-untrack-item"));
    }
  });

  el("btn-price-session")?.addEventListener("click", async () => {
    const { items, qtyByItem } = collectSessionPriceableItems(latestState?.snapshot);
    if (items.length === 0) {
      showToast("Aucun butin ni récolte à valoriser pour cette session.");
      return;
    }
    await fetchAlbionDataPrices(items, qtyByItem);
  });
};

const boot = async () => {
  initEvents();
  const loaded = await invokeCmd("load_settings");
  if (loaded?.settings) {
    settings = normalizeSettings(loaded.settings);
    const rawTracked = Array.isArray(loaded.settings.trackedItems) ? loaded.settings.trackedItems : [];
    if (rawTracked.length !== settings.trackedItems.length) {
      await persistSettings();
    }
  }
  if (loaded?.warning) {
    showToast(loaded.warning);
  }

  document.querySelectorAll(".seg-btn[data-meter-mode]").forEach((b) => {
    b.classList.toggle("active", b.getAttribute("data-meter-mode") === settings.meterMode);
  });
  const regSelect = el("market-region");
  if (regSelect) regSelect.value = settings.region;

  await applyWindowPreferences();
  await refreshState(true);
  await updateCraftingSimulation();

  const priceWrap = el("api-price-rows");
  if (priceWrap) {
    priceWrap.innerHTML =
      settings.trackedItems.length > 0
        ? `<div class="empty-state">${settings.trackedItems.length} objet(s) suivi(s). Cliquez sur <strong>Actualiser</strong> ou recherchez un objet pour l'ajouter.</div>`
        : `<div class="empty-state">Aucun objet suivi. Utilisez la barre de recherche ci-dessus pour ajouter des objets du marché à surveiller.</div>`;
  }

  const snap = latestState?.snapshot;
  if (
    tauriCore?.invoke &&
    snap &&
    !catalogAutoSyncAttempted &&
    ((snap.catalogClusterCount || 0) < 100 ||
      (snap.catalogSpellCount || 0) < 200 ||
      (snap.catalogWeaponSpellCount || 0) < 100)
  ) {
    catalogAutoSyncAttempted = true;
    syncCatalogFromGitHub({ silent: true });
  }

  setInterval(() => refreshState(false), settings.autoRefreshMs);
};

boot();
