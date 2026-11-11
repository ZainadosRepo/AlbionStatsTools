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
let activeTab = "combat";
let lastStateSignature = "";
let toastTimer = null;
let searchTimer = null;
let searchSeq = 0;
let recipePriceSeq = 0;
let activeCraftItemContext = null;
let lastPriceRows = [];
let browserMaximizedFallback = false;
const itemInfoCache = new Map();

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

// Global CSP-compliant capture-phase media fallback handler (replaces blocked inline onerror)
document.addEventListener(
  "error",
  (ev) => {
    const target = ev.target;
    if (!(target instanceof HTMLImageElement) || !target.classList.contains("slot-img")) {
      return;
    }
    const rawFallbacks = target.getAttribute("data-fallbacks") || "";
    const remaining = rawFallbacks.split("|").filter(Boolean);
    if (remaining.length > 0) {
      const nextUrl = remaining.shift();
      target.setAttribute("data-fallbacks", remaining.join("|"));
      target.src = nextUrl;
      return;
    }
    target.hidden = true;
    target.closest(".icon-slot")?.classList.add("is-fallback");
  },
  true,
);

const renderCityBadge = (cityName) => {
  const info = getCityBadgeInfo(cityName);
  if (!info) return `<span class="dash-empty">—</span>`;
  return `<span class="city-pill city-${info.key}"><span class="city-dot"></span>${escapeHtml(info.label)}</span>`;
};

const renderItemThumb = (uniqueName, cls = "icon-slot-sm", slotHint = "") => {
  const candidates = buildAlbionItemIconCandidates(uniqueName, 128);
  const glyph = inferItemFallbackGlyph(uniqueName, slotHint);
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
      <img class="slot-img" src="${escapeHtml(primary)}" data-fallbacks="${escapeHtml(fallbacks.join("|"))}" alt="" loading="lazy" decoding="async" />
    </span>
  `;
};

const renderItemThumbWithTier = (uniqueName, slotHint = "") => {
  const candidates = buildAlbionItemIconCandidates(uniqueName, 128);
  const glyph = inferItemFallbackGlyph(uniqueName, slotHint);
  const meta = parseItemTierAndEnchantment(uniqueName);
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
      <img class="slot-img item-thumb-img" src="${escapeHtml(primary)}" data-fallbacks="${escapeHtml(fallbacks.join("|"))}" alt="" loading="lazy" decoding="async" />
      ${tierBadge}
      ${enchBadge}
    </span>
  `;
};

const renderSpellThumb = (spell) => {
  const obj = typeof spell === "string" ? { uniqueName: spell } : spell || {};
  const candidates = buildAlbionSpellIconCandidates(obj, 64);
  const glyph = inferSpellFallbackGlyph(obj.uniqueName, obj.category);
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
      <img class="slot-img" src="${escapeHtml(primary)}" data-fallbacks="${escapeHtml(fallbacks.join("|"))}" alt="" loading="lazy" decoding="async" />
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

  const badge = el("capture-badge");
  if (badge && cap) {
    const isLive = cap.mode === "rawSocket";
    badge.className = `status-badge ${isLive ? "status-live" : "status-standby"}`;
    badge.innerHTML = `<span class="badge-dot"></span><span class="badge-text">${isLive ? "UDP 5056" : "Simulateur"}</span>`;
    badge.title = cap.detail || "";
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
      syncBtn.title = `Catalogue : ${formatExactInt(snap.catalogItemCount)} objets • ${formatExactInt(snap.catalogSpellCount || 0)} sorts • ${formatExactInt(snap.catalogRecipeCount || 0)} recettes — cliquer pour synchroniser ao-bin-dumps`;
    }
  }

  const lp = snap.localPlayer;
  const guildTag = lp.guildName ? ` [${lp.guildName}]` : "";
  const rawName = String(lp.username || "En attente…");
  const isStandby = rawName.startsWith("En attente");
  const displayName = isStandby ? "En attente…" : `${rawName}${guildTag}`;
  const nameTitle = isStandby
    ? "En attente de paquets Albion Online… Si le jeu est déjà ouvert, changez de zone ou relancez votre session pour synchroniser vos données."
    : displayName;
  const rawMap = String(lp.currentMapNameFr || "En attente…");
  const displayMap = rawMap.startsWith("En attente") ? "En attente…" : rawMap;
  el("header-player-name").textContent = displayName;
  el("header-player-name").title = nameTitle;
  el("header-map-name").textContent = displayMap;
  el("header-map-name").title = displayMap;
  el("header-silver").textContent = `${formatExactInt(lp.silver)} Silver`;
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

const renderCombatTab = () => {
  const data = activeMeterDataset();
  el("combat-timer").textContent = formatDuration(data.combatSeconds);
  el("party-total-dmg").textContent = formatNumber(data.totalDamage);
  el("party-total-heal").textContent = formatNumber(data.totalHeal);
  el("party-total-taken").textContent = formatNumber(data.totalTaken);

  const ranked = rankMeterPlayers(data.players, settings.meterMode);
  const container = el("meter-rows");
  if (ranked.length === 0) {
    container.innerHTML = `<div class="empty-state">Aucun combat actif. Lancez Albion Online ou cliquez sur <strong>Simuler</strong> pour tester le compteur.</div>`;
    el("spell-rows").innerHTML = `<div class="empty-state">Sélectionnez un joueur pour inspecter la répartition de ses sorts.</div>`;
    return;
  }

  if (!selectedPlayerName || !ranked.some((p) => p.name === selectedPlayerName)) {
    selectedPlayerName = ranked[0].name;
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

  container.querySelectorAll(".meter-row").forEach((row) => {
    row.addEventListener("click", () => {
      selectedPlayerName = row.getAttribute("data-player-name");
      renderCombatTab();
    });
  });

  const activePlayer = ranked.find((p) => p.name === selectedPlayerName) || ranked[0];
  el("spell-panel-title").textContent = `Sorts — ${activePlayer.name}`;
  el("spell-panel-weapon").textContent = activePlayer.weaponNameFr;

  const spells = activePlayer.spells || [];
  if (spells.length === 0) {
    el("spell-rows").innerHTML = `<div class="empty-state">Aucun sort enregistré pour ce joueur.</div>`;
  } else {
    el("spell-rows").innerHTML = `
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
  }
};

const renderSessionTab = () => {
  const snap = latestState?.snapshot;
  if (!snap) {
    const archWrap = el("archived-session-rows");
    if (archWrap && !archWrap.innerHTML) {
      archWrap.innerHTML = `<div class="empty-state">Aucune session archivée. Cliquez sur <strong>Terminer &amp; enregistrer</strong> ou fermez l'application après une activité pour sauvegarder le bilan sur le disque.</div>`;
    }
    const facWrap = el("faction-rows");
    if (facWrap && !facWrap.innerHTML) {
      facWrap.innerHTML = `<div class="empty-state">Aucune activité de guerre de factions enregistrée.</div>`;
    }
    return;
  }
  const s = snap.session;
  const lp = snap.localPlayer;

  el("sess-elapsed").textContent = formatDuration(s.elapsedSeconds);
  el("sess-packets").textContent = formatExactInt(snap.network.packetsProcessed);
  el("sess-events").textContent = formatExactInt(snap.network.telemetryEventsProcessed);

  el("kpi-fame").textContent = formatNumber(s.fame);
  el("kpi-fame-hr").textContent = `${formatNumber(s.famePerHour)} / heure`;
  el("kpi-fame-extra").textContent = `Zone : ${formatNumber(s.zoneFame)} • Sacoche : ${formatNumber(s.satchelFame)}`;

  el("kpi-silver").textContent = `${formatNumber(s.silver)} Silver`;
  el("kpi-silver-hr").textContent = `${formatNumber(s.silverPerHour)} Silver / h`;
  el("kpi-silver-tax").textContent = `Taxe guilde : ${formatNumber(s.guildTaxPaid)} • Réparations : ${formatNumber(s.repairCosts)}`;

  el("kpi-respec").textContent = formatNumber(s.respecPoints);
  el("kpi-respec-hr").textContent = `${formatNumber(s.respecPerHour)} / heure`;
  el("kpi-respec-cost").textContent = `Silver payé : ${formatNumber(s.respecPaidSilver)} Silver`;

  el("kpi-might").textContent = `${formatNumber(s.might)} Might`;
  el("kpi-favor").textContent = `${formatNumber(s.favor)} Faveur`;
  el("kpi-might-hr").textContent = `${formatNumber(s.mightPerHour)} Might/h • ${formatNumber(s.favorPerHour)} Faveur/h`;

  el("kpi-kd").textContent = `${s.kills} K / ${s.deaths} D`;
  el("kpi-focus").textContent = `Focus : ${formatExactInt(lp.focusPoints)} / ${formatExactInt(lp.maxFocusPoints)}`;
  el("kpi-season").textContent = `Points GvG : ${s.seasonPoints} • LP : ${formatExactInt(lp.learningPoints)}`;

  const archived = snap.archivedSessions || [];
  const archBadge = el("archived-session-badge");
  if (archBadge) {
    archBadge.textContent = `${archived.length} session${archived.length > 1 ? "s" : ""}`;
  }
  const archWrap = el("archived-session-rows");
  if (archWrap) {
    if (archived.length === 0) {
      archWrap.innerHTML = `<div class="empty-state">Aucune session archivée. Cliquez sur <strong>Terminer &amp; enregistrer</strong> ou fermez l'application après une activité pour sauvegarder le bilan sur le disque.</div>`;
    } else {
      archWrap.innerHTML = `
        <table class="data-table">
          <thead>
            <tr>
              <th>Session</th>
              <th class="num-col">Renommée</th>
              <th class="num-col">Silver</th>
              <th class="num-col col-optional">Bilan</th>
              <th class="num-col"></th>
            </tr>
          </thead>
          <tbody>
            ${archived
              .map((a) => {
                const dateStr =
                  a.endedAtMs > 0
                    ? new Date(a.endedAtMs).toLocaleString("fr-FR", {
                        day: "2-digit",
                        month: "2-digit",
                        hour: "2-digit",
                        minute: "2-digit",
                      })
                    : "—";
                const extraParts = [`${a.kills || 0}K/${a.deaths || 0}D`];
                if ((a.might || 0) > 0) extraParts.push(`${formatNumber(a.might)} M`);
                return `
              <tr>
                <td>
                  <div class="item-text-group">
                    <strong class="item-display-name">${escapeHtml(a.label)}</strong>
                    <span class="item-id-sub mono">${escapeHtml(dateStr)} • ${formatDuration(a.durationSeconds)}</span>
                  </div>
                </td>
                <td class="num-col mono">
                  <div class="session-stat-stack">
                    <span>${formatNumber(a.fame)}</span>
                    <small class="session-stat-sub">${formatNumber(a.famePerHour)}/h</small>
                  </div>
                </td>
                <td class="num-col mono">
                  <div class="session-stat-stack">
                    <span>${formatNumber(a.silver)} <span class="currency-tag">Silver</span></span>
                    <small class="session-stat-sub">${formatNumber(a.silverPerHour)}/h</small>
                  </div>
                </td>
                <td class="num-col mono col-optional">${escapeHtml(extraParts.join(" • "))}</td>
                <td class="num-col">
                  <button
                    class="btn btn-danger-subtle btn-xs btn-icon-xs"
                    type="button"
                    data-del-archived-session="${escapeHtml(a.id)}"
                    title="Supprimer cette session de l'historique"
                    aria-label="Supprimer la session"
                  >
                    <svg class="ico"><use href="#ico-trash"></use></svg>
                  </button>
                </td>
              </tr>
            `;
              })
              .join("")}
          </tbody>
        </table>
      `;
    }
  }

  el("faction-rows").innerHTML = `
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
        ${(s.factions || [])
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
  `;
};

const renderDungeonsTab = () => {
  const runs = latestState?.snapshot?.dungeons || [];
  el("dungeon-count-badge").textContent = `${runs.length} instance${runs.length > 1 ? "s" : ""}`;
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

const renderGatheringTab = () => {
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
  if (log.length === 0) {
    el("gathering-rows").innerHTML = `<div class="empty-state">Aucune récolte enregistrée durant cette session.</div>`;
    return;
  }

  el("gathering-rows").innerHTML = `
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
};

const renderLootAndInspectTab = () => {
  const snap = latestState?.snapshot;
  const players = snap?.inspectedPlayers || [];
  el("inspect-count-badge").textContent = `${players.length} joueur${players.length > 1 ? "s" : ""}`;
  if (players.length === 0) {
    el("inspect-cards").innerHTML = `<div class="empty-state">Aucun membre du groupe détecté.</div>`;
  } else {
    el("inspect-cards").innerHTML = players
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
  }

  const loot = snap?.lootLog || [];
  if (loot.length === 0) {
    el("loot-rows").innerHTML = `<div class="empty-state">Aucun butin ramassé.</div>`;
  } else {
    el("loot-rows").innerHTML = `
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
              <td class="num-col mono">×${l.quantity}</td>
              <td class="num-col mono text-accent">${formatExactInt(l.estimatedSilver)} Silver</td>
            </tr>
          `,
            )
            .join("")}
        </tbody>
      </table>
    `;
  }
};

const renderCapturedMarketOrders = () => {
  const snap = latestState?.snapshot;
  const orders = snap?.marketOrders || [];
  const mails = snap?.mailNotifications || [];
  const total = orders.length + mails.length;
  const badge = el("market-orders-badge");
  if (badge) badge.textContent = `${total} capture${total > 1 ? "s" : ""}`;

  if (orders.length === 0 && mails.length === 0) {
    el("captured-market-rows").innerHTML = `
      <div class="empty-state empty-market-state">
        <div class="empty-title">Aucun ordre ou courrier capturé</div>
        <div class="empty-hint">Ouvrez l'Hôtel des Ventes ou vos courriers en jeu pour capturer automatiquement vos transactions.</div>
      </div>
    `;
    return;
  }

  el("captured-market-rows").innerHTML = `
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
  }
  box.hidden = false;
};

const syncCatalogFromGitHub = async () => {
  const btn = el("btn-sync-catalog");
  if (btn) btn.disabled = true;
  showToast("Synchronisation de la base Albion (ao-bin-dumps : objets, recettes, zones et sorts)…");
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
    const recipeCount = latestState?.snapshot?.catalogRecipeCount || 0;
    showToast(
      `Catalogue synchronisé (+${formatExactInt(added || 0)} entrées, ${formatExactInt(spellCount)} sorts, ${formatExactInt(recipeCount)} recettes).`,
    );
  } catch (err) {
    showToast(`Échec synchronisation ao-bin-dumps : ${err.message || err}`);
  } finally {
    if (btn) btn.disabled = false;
  }
};

const renderActiveTab = () => {
  switch (activeTab) {
    case "session":
      renderSessionTab();
      break;
    case "dungeons":
      renderDungeonsTab();
      break;
    case "gathering":
      renderGatheringTab();
      break;
    case "loot":
      renderLootAndInspectTab();
      break;
    case "economy":
      renderCapturedMarketOrders();
      break;
    case "combat":
    default:
      renderSnapshotSelect();
      renderCombatTab();
      break;
  }
};

const renderAll = () => {
  renderHeader();
  renderSnapshotSelect();
  renderCombatTab();
  renderSessionTab();
  renderDungeonsTab();
  renderGatheringTab();
  renderLootAndInspectTab();
  renderCapturedMarketOrders();
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
    renderAll();
  } else {
    renderHeader();
    renderActiveTab();
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
      renderActiveTab();
    });
  });

  // Meter mode segmented control
  document.querySelectorAll(".seg-btn[data-meter-mode]").forEach((btn) => {
    btn.addEventListener("click", () => {
      settings.meterMode = btn.getAttribute("data-meter-mode") || "damage";
      document
        .querySelectorAll(".seg-btn[data-meter-mode]")
        .forEach((b) => b.classList.toggle("active", b === btn));
      persistSettings();
      renderCombatTab();
    });
  });

  // Snapshot selector & delete button
  el("snapshot-select")?.addEventListener("change", (ev) => {
    selectedSnapshotId = ev.target.value || "live";
    renderSnapshotSelect();
    renderCombatTab();
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
    syncCatalogFromGitHub();
  });

  el("btn-simulate")?.addEventListener("click", async () => {
    const res = await invokeCmd("simulate_session");
    if (res) {
      latestState = res;
      selectedSnapshotId = "live";
      lastStateSignature = computeStateSignature(res);
      renderAll();
      showToast("Scénario Photon UDP injecté (combat 5v5, brumes, récolte, butin, marché).");
    }
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
      renderAll();
      showToast("Compteur de combat réinitialisé.");
    }
  });

  el("btn-archive-session")?.addEventListener("click", async () => {
    const archived = await invokeCmd("archive_session", { label: null });
    if (archived) {
      await refreshState(true);
      showToast(
        `Session enregistrée : ${archived.label} (${formatNumber(archived.fame)} Fame • ${formatNumber(archived.silver)} Silver).`,
      );
    } else {
      showToast("Aucune activité de session à enregistrer pour le moment.");
    }
  });

  el("archived-session-rows")?.addEventListener("click", async (ev) => {
    const btn = ev.target.closest("[data-del-archived-session]");
    if (!btn) return;
    const id = btn.getAttribute("data-del-archived-session");
    if (!id) return;
    const deleted = await invokeCmd("delete_archived_session", { id });
    if (deleted) {
      await refreshState(true);
      showToast("Session archivée supprimée.");
    }
  });

  el("btn-reset-session")?.addEventListener("click", async () => {
    const res = await invokeCmd("reset_session");
    if (res) {
      latestState = res;
      lastStateSignature = computeStateSignature(res);
      renderAll();
      showToast("Session réinitialisée (Fame, Silver, Donjons, Récolte et Butin remis à zéro).");
    }
  });

  el("btn-clear-dungeons")?.addEventListener("click", async () => {
    const res = await invokeCmd("clear_dungeons");
    if (res) {
      latestState = res;
      renderAll();
      showToast("Historique des donjons et brumes effacé.");
    }
  });

  el("btn-clear-gathering")?.addEventListener("click", async () => {
    const res = await invokeCmd("clear_gathering");
    if (res) {
      latestState = res;
      renderAll();
      showToast("Journal et totaux de récolte effacés.");
    }
  });

  el("btn-clear-loot")?.addEventListener("click", async () => {
    const res = await invokeCmd("clear_loot");
    if (res) {
      latestState = res;
      renderAll();
      showToast("Journal de butin effacé.");
    }
  });

  el("btn-clear-market")?.addEventListener("click", async () => {
    const res = await invokeCmd("clear_market_data");
    if (res) {
      latestState = res;
      renderAll();
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
    renderMarketPrices();
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

  setInterval(() => refreshState(false), settings.autoRefreshMs);
};

boot();
