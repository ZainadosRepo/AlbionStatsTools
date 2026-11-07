import {
  buildAlbionDataPriceUrl,
  buildAlbionItemIconUrl,
  buildAlbionSpellIconUrl,
  collectSessionPriceableItems,
  computeSessionMarketValue,
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
let lastPriceRows = [];
const itemInfoCache = new Map();

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

const renderCityBadge = (cityName) => {
  const info = getCityBadgeInfo(cityName);
  if (!info) return `<span class="dash-empty">—</span>`;
  return `<span class="city-pill city-${info.key}">${info.icon} ${escapeHtml(info.label)}</span>`;
};

const renderItemThumb = (uniqueName, size = 36, cls = "item-icon-sm") => {
  const url = buildAlbionItemIconUrl(uniqueName, size);
  if (!url) return "";
  return `<img class="${cls}" src="${escapeHtml(url)}" alt="" loading="lazy" onerror="this.hidden=true" />`;
};

const renderItemThumbWithTier = (uniqueName, size = 38) => {
  const url = buildAlbionItemIconUrl(uniqueName, size);
  if (!url) return "";
  const meta = parseItemTierAndEnchantment(uniqueName);
  const tierCls = meta.tier ? `tier-box-${meta.tier}` : "";
  const tierBadge = meta.label ? `<span class="tier-chip ${meta.tier ? `tier-${meta.tier}` : ""}">${meta.label}</span>` : "";
  const enchBadge = meta.enchantment > 0 ? `<span class="ench-dot ench-${meta.enchantment}" title="Enchantement .${meta.enchantment}"></span>` : "";
  return `
    <div class="item-thumb-box ${tierCls}">
      <img class="item-thumb-img" src="${escapeHtml(url)}" alt="" loading="lazy" onerror="this.hidden=true" />
      ${tierBadge}
      ${enchBadge}
    </div>
  `;
};

const renderSpellThumb = (uniqueName, size = 32) => {
  const url = buildAlbionSpellIconUrl(uniqueName, size);
  if (!url) return "";
  return `<img class="item-icon-sm" src="${escapeHtml(url)}" alt="" loading="lazy" onerror="this.hidden=true" />`;
};

const persistSettings = async () => {
  try {
    await invokeCmd("save_settings", { settings });
  } catch (e) {
    console.warn("save_settings:", e);
  }
};

const applyOverlayOpacity = () => {
  const ratio = Math.max(0.35, Math.min(1, (Number(settings.overlayOpacity) || 96) / 100));
  document.documentElement.style.setProperty("--shell-opacity", ratio.toFixed(2));
  const slider = el("overlay-opacity");
  if (slider && Number(slider.value) !== settings.overlayOpacity) {
    slider.value = String(settings.overlayOpacity);
  }
  if (slider) slider.title = `Opacité de la fenêtre overlay (${settings.overlayOpacity}%)`;
};

const applyWindowPreferences = async () => {
  const shell = el("app");
  shell?.classList.toggle("compact-overlay", settings.compactOverlay);
  el("btn-compact")?.classList.toggle("active", settings.compactOverlay);
  el("btn-pin")?.classList.toggle("active", settings.alwaysOnTop);
  applyOverlayOpacity();
  if (tauriWindow?.setAlwaysOnTop) {
    try {
      await tauriWindow.setAlwaysOnTop(settings.alwaysOnTop);
    } catch (e) {
      console.warn("setAlwaysOnTop:", e);
    }
  }
};

const activeMeterDataset = () => {
  const snap = latestState?.snapshot;
  if (!snap) {
    return {
      mapName: "Caerleon",
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
    mapName: snap.localPlayer?.currentMapNameFr || "Caerleon",
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
    badge.innerHTML = `<span class="badge-dot"></span><span class="badge-text">${isLive ? "Capture UDP 5056" : "Prêt / Simulateur"}</span>`;
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
      syncBtn.title = `Catalogue : ${formatExactInt(snap.catalogItemCount)} objets • ${formatExactInt(snap.catalogSpellCount || 0)} sorts — cliquer pour synchroniser ao-bin-dumps (objets + zones + sorts)`;
    }
  }

  const clickThrough = Boolean(latestState?.clickThrough);
  el("btn-click-through")?.classList.toggle("active", clickThrough);
  el("app")?.classList.toggle("click-through", clickThrough);
  const ctBanner = el("click-through-banner");
  if (ctBanner) ctBanner.hidden = !clickThrough;

  const lp = snap.localPlayer;
  const guildTag = lp.guildName ? ` [${lp.guildName}]` : "";
  const rawName = String(lp.username || "En attente…");
  const displayName = rawName.startsWith("En attente") ? "En attente…" : `${rawName}${guildTag}`;
  el("header-player-name").textContent = displayName;
  el("header-map-name").textContent = lp.currentMapNameFr || "Caerleon";
  el("header-silver").textContent = `${formatExactInt(lp.silver)} Ag`;
};

const renderSnapshotSelect = () => {
  const select = el("snapshot-select");
  const delBtn = el("btn-delete-snapshot");
  const snaps = latestState?.snapshot?.snapshots || [];
  if (!select) return;

  const prev = selectedSnapshotId;
  let html = `<option value="live">🔴 Combat en direct</option>`;
  for (const s of snaps) {
    html += `<option value="${escapeHtml(s.id)}">📸 ${escapeHtml(s.label)}</option>`;
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
    container.innerHTML = `<div class="empty-state">Aucun combat actif. Lancez Albion Online ou cliquez sur <strong>⚡ Simuler Session</strong> pour tester le compteur.</div>`;
    el("spell-rows").innerHTML = `<div class="empty-state">Sélectionnez un joueur pour inspecter ses sorts.</div>`;
    return;
  }

  if (!selectedPlayerName || !ranked.some((p) => p.name === selectedPlayerName)) {
    selectedPlayerName = ranked[0].name;
  }

  container.innerHTML = ranked
    .map((p) => {
      const isSel = p.name === selectedPlayerName;
      const weaponThumb = renderItemThumb(p.weaponUniqueName, 44, "item-icon");
      const rankBadge =
        p.rank === 1
          ? `<span class="meter-rank rank-1" title="1er">🥇</span>`
          : p.rank === 2
            ? `<span class="meter-rank rank-2" title="2e">🥈</span>`
            : p.rank === 3
              ? `<span class="meter-rank rank-3" title="3e">🥉</span>`
              : `<span class="meter-rank">#${p.rank}</span>`;
      return `
        <div class="meter-row ${isSel ? "selected" : ""}" data-player-name="${escapeHtml(p.name)}">
          <div class="meter-bar-fill meter-bar-${escapeHtml(settings.meterMode)}" style="width: ${p.barPercent.toFixed(1)}%"></div>
          <div class="meter-row-content">
            <div class="meter-left">
              ${rankBadge}
              ${weaponThumb}
              <div class="meter-ident">
                <span class="meter-name">${escapeHtml(p.name)} ${p.isSelf ? "★" : ""}</span>
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
            <th>Ticks</th>
            <th>Dégâts</th>
            <th>Soins</th>
            <th>Overheal</th>
            <th>Part</th>
          </tr>
        </thead>
        <tbody>
          ${spells
            .map(
              (s) => `
            <tr>
              <td>
                <span class="cell-with-icon" title="${escapeHtml(s.uniqueName)}">
                  ${renderSpellThumb(s.uniqueName, 32)}
                  <strong>${escapeHtml(s.nameFr)}</strong>
                </span>
              </td>
              <td>${s.ticks}</td>
              <td>${formatExactInt(s.damage)}</td>
              <td>${formatExactInt(s.heal)}</td>
              <td>${formatExactInt(s.overhealed)}</td>
              <td>${s.sharePercent.toFixed(1)}%</td>
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
  if (!snap) return;
  const s = snap.session;
  const lp = snap.localPlayer;

  el("sess-elapsed").textContent = formatDuration(s.elapsedSeconds);
  el("sess-packets").textContent = formatExactInt(snap.network.packetsProcessed);
  el("sess-events").textContent = formatExactInt(snap.network.telemetryEventsProcessed);

  el("kpi-fame").textContent = formatNumber(s.fame);
  el("kpi-fame-hr").textContent = `${formatNumber(s.famePerHour)} / heure`;
  el("kpi-fame-extra").textContent = `Zone : ${formatNumber(s.zoneFame)} • Sacoche : ${formatNumber(s.satchelFame)}`;

  el("kpi-silver").textContent = `${formatNumber(s.silver)} Ag`;
  el("kpi-silver-hr").textContent = `${formatNumber(s.silverPerHour)} Ag / heure`;
  el("kpi-silver-tax").textContent = `Taxe guilde : ${formatNumber(s.guildTaxPaid)} • Réparations : ${formatNumber(s.repairCosts)}`;

  el("kpi-respec").textContent = formatNumber(s.respecPoints);
  el("kpi-respec-hr").textContent = `${formatNumber(s.respecPerHour)} / heure`;
  el("kpi-respec-cost").textContent = `Argent payé : ${formatNumber(s.respecPaidSilver)} Ag`;

  el("kpi-might").textContent = `${formatNumber(s.might)} Might`;
  el("kpi-favor").textContent = `${formatNumber(s.favor)} Faveur`;
  el("kpi-might-hr").textContent = `${formatNumber(s.mightPerHour)} Might/h • ${formatNumber(s.favorPerHour)} Faveur/h`;

  el("kpi-kd").textContent = `${s.kills} K / ${s.deaths} D`;
  el("kpi-focus").textContent = `Focus : ${formatExactInt(lp.focusPoints)} / ${formatExactInt(lp.maxFocusPoints)}`;
  el("kpi-season").textContent = `Points GvG : ${s.seasonPoints} • LP : ${formatExactInt(lp.learningPoints)}`;

  el("faction-rows").innerHTML = `
    <table class="data-table">
      <thead>
        <tr>
          <th>Cité de Faction</th>
          <th>Jetons Gagnés</th>
          <th>Points de Rang</th>
          <th>Points de Drapeau</th>
        </tr>
      </thead>
      <tbody>
        ${(s.factions || [])
          .map(
            (f) => `
          <tr>
            <td><strong>${escapeHtml(f.city)}</strong></td>
            <td>${formatExactInt(f.coins)}</td>
            <td>${formatExactInt(f.rankPoints)}</td>
            <td>${formatExactInt(f.flagPoints)}</td>
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
  el("dungeon-count-badge").textContent = `${runs.length} instance(s)`;
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
          <th>Durée</th>
          <th>Renommée</th>
          <th>Fame / h</th>
          <th>Argent</th>
          <th>Might / Faveur</th>
          <th>Coffres / Feux Follets</th>
        </tr>
      </thead>
      <tbody>
        ${runs
          .map(
            (d) => `
          <tr>
            <td><strong>${escapeHtml(d.nameFr)}</strong> ${d.isActive ? '<span class="status-badge status-live">En cours</span>' : ""}</td>
            <td>T${d.tier}</td>
            <td>${formatDuration(d.durationSeconds)}</td>
            <td>${formatNumber(d.fame)}</td>
            <td>${formatNumber(d.famePerHour)}/h</td>
            <td>${formatNumber(d.silver)} Ag</td>
            <td>${formatExactInt(d.might)} / ${formatExactInt(d.favor)}</td>
            <td>📦 ${d.chestsOpened} • ✨ ${d.cagedWispsOpened}</td>
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
  if (!snap) return;
  const t = snap.gatheringTotals;
  el("gat-wood").textContent = formatExactInt(t.wood);
  el("gat-ore").textContent = formatExactInt(t.ore);
  el("gat-fiber").textContent = formatExactInt(t.fiber);
  el("gat-hide").textContent = formatExactInt(t.hide);
  el("gat-rock").textContent = formatExactInt(t.rock);
  el("gat-fish").textContent = formatExactInt(t.fish);
  el("gathering-summary-badge").textContent = `${formatExactInt(t.totalResources)} ressources (${t.totalNodes} nœuds) • ~${formatNumber(t.totalEstimatedSilver)} Ag`;

  const log = snap.gatheringLog || [];
  if (log.length === 0) {
    el("gathering-rows").innerHTML = `<div class="empty-state">Aucune récolte enregistrée.</div>`;
    return;
  }

  el("gathering-rows").innerHTML = `
    <table class="data-table">
      <thead>
        <tr>
          <th>Ressource</th>
          <th>Tier</th>
          <th>Base</th>
          <th>Bonus Récolteur</th>
          <th>Bonus Premium</th>
          <th>Total</th>
          <th>Valeur Estimée</th>
        </tr>
      </thead>
      <tbody>
        ${log
          .map(
            (g) => `
          <tr>
            <td>
              <span class="cell-with-icon">
                ${renderItemThumb(g.uniqueName, 36)}
                <strong>${escapeHtml(g.nameFr)}</strong>
              </span>
            </td>
            <td>T${g.tier}.${g.enchantment}</td>
            <td>${g.standardAmount}</td>
            <td>+${g.collectorBonus}</td>
            <td>+${g.premiumBonus}</td>
            <td><strong>${g.totalAmount}</strong></td>
            <td>${formatExactInt(g.estimatedSilver)} Ag</td>
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
  if (!snap) return;

  const players = snap.inspectedPlayers || [];
  el("inspect-count-badge").textContent = `${players.length} joueur(s)`;
  if (players.length === 0) {
    el("inspect-cards").innerHTML = `<div class="empty-state">Aucun joueur inspecté.</div>`;
  } else {
    el("inspect-cards").innerHTML = players
      .map(
        (p) => `
      <div class="inspect-card">
        <div class="inspect-head">
          <div>
            <strong>${escapeHtml(p.name)}</strong>
            ${p.guildName ? `<span class="badge-subtle">[${escapeHtml(p.guildName)}]</span>` : ""}
          </div>
          <span class="player-map">~${formatNumber(p.estimatedLoadoutValue)} Ag</span>
        </div>
        <div class="slot-list">
          ${(p.slots || [])
            .map(
              (sl) => `
            <div class="slot-item">
              <span class="slot-tag">${escapeHtml(sl.slot)}</span>
              <span class="slot-item-val">
                ${renderItemThumb(sl.uniqueName, 32)}
                <span>${escapeHtml(sl.nameFr)}</span>
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

  const loot = snap.lootLog || [];
  if (loot.length === 0) {
    el("loot-rows").innerHTML = `<div class="empty-state">Aucun butin ramassé.</div>`;
  } else {
    el("loot-rows").innerHTML = `
      <table class="data-table">
        <thead>
          <tr>
            <th>Ramassé par</th>
            <th>Source</th>
            <th>Objet</th>
            <th>Qté</th>
            <th>Estimation</th>
          </tr>
        </thead>
        <tbody>
          ${loot
            .map(
              (l) => `
            <tr>
              <td><strong>${escapeHtml(l.lootedBy)}</strong></td>
              <td>${escapeHtml(l.lootedFrom)}</td>
              <td>
                <span class="cell-with-icon">
                  ${renderItemThumb(l.uniqueName, 36)}
                  <span>${escapeHtml(l.nameFr)}</span>
                </span>
              </td>
              <td>×${l.quantity}</td>
              <td>${formatExactInt(l.estimatedSilver)} Ag</td>
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
  if (!snap) return;
  const orders = snap.marketOrders || [];
  const mails = snap.mailNotifications || [];
  const total = orders.length + mails.length;
  const badge = el("market-orders-badge");
  if (badge) badge.textContent = `${total} capture${total > 1 ? "s" : ""}`;

  if (orders.length === 0 && mails.length === 0) {
    el("captured-market-rows").innerHTML = `
      <div class="empty-state empty-market-state">
        <span class="empty-icon" aria-hidden="true">📜</span>
        <div class="empty-title">Aucun ordre ou courrier du marché capturé</div>
        <div class="empty-hint">Consultez l'hôtel des ventes ou ouvrez votre boîte aux lettres en jeu pour capturer automatiquement vos transactions.</div>
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
          <th>Qté</th>
          <th>Prix Unitaire</th>
          <th>Total</th>
        </tr>
      </thead>
      <tbody>
        ${orders
          .map(
            (o) => `
          <tr>
            <td><span class="badge-subtle ${o.isRequest ? "badge-buy-order" : "badge-sell-order"}">${o.isRequest ? "Demande d'Achat" : "Vente Q" + o.qualityLevel}</span></td>
            <td>
              <span class="cell-with-icon">
                ${renderItemThumbWithTier(o.itemUniqueName, 36)}
                <strong>${escapeHtml(o.itemNameFr)}</strong>
              </span>
            </td>
            <td><span class="qty-pill">×${o.amount}</span></td>
            <td><span class="silver-amount">${formatExactInt(o.unitPriceSilver)} <span class="ag-tag">Ag</span></span></td>
            <td><strong class="silver-amount">${formatExactInt(o.totalPriceSilver)} <span class="ag-tag">Ag</span></strong></td>
          </tr>
        `,
          )
          .join("")}
        ${mails
          .map(
            (m) => `
          <tr>
            <td><span class="status-badge status-live">Courrier Vendu</span></td>
            <td>
              <span class="cell-with-icon">
                ${renderItemThumbWithTier(m.itemUniqueName, 36)}
                <strong>${escapeHtml(m.itemNameFr)}</strong>
              </span>
            </td>
            <td><span class="qty-pill">×${m.amount}</span></td>
            <td><span class="silver-amount">${formatExactInt(m.unitPriceSilver)} <span class="ag-tag">Ag</span></span></td>
            <td><strong class="silver-amount text-gold">${formatExactInt(m.totalPriceSilver)} <span class="ag-tag">Ag</span></strong></td>
          </tr>
        `,
          )
          .join("")}
      </tbody>
    </table>
  `;
};

const updateCraftingSimulation = async () => {
  const matRetVal = Math.max(0, Number(el("craft-mat-ret")?.value) || 0);
  const matArtVal = Math.max(0, Number(el("craft-mat-art")?.value) || 0);
  const sellVal = Math.max(0, Number(el("craft-sell")?.value) || 0);
  const qty = Math.max(1, Number(el("craft-qty")?.value) || 1);
  const bonusVal = Math.max(0, Number(el("craft-bonus")?.value) || 0);

  // Sync active state on preset pills
  document.querySelectorAll(".preset-pill[data-preset-bonus]").forEach((b) => {
    const val = Number(b.getAttribute("data-preset-bonus"));
    b.classList.toggle("active", Math.abs(val - bonusVal) < 0.05);
  });

  const input = {
    itemName: el("craft-name")?.value || "Objet",
    craftQuantity: qty,
    sellUnitPrice: sellVal,
    itemValue: Math.max(0, Number(el("craft-item-val")?.value) || 0),
    productionBonusPercent: bonusVal,
    usageFeePer100Nutrition: Math.max(0, Number(el("craft-fee")?.value) || 0),
    isPremium: Boolean(el("craft-premium")?.checked),
    isSellOrder: Boolean(el("craft-sell-order")?.checked),
    journalProfitPerItem: Math.max(0, Number(el("craft-journal")?.value) || 0),
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

  const res = await invokeCmd("calculate_crafting", { input });
  if (!res) return;

  const isProfitable = res.totalBatchProfit >= 0;
  const rrrPct = (res.resourceReturnRate * 100).toFixed(2);
  const totalStationFee = Math.round(res.stationFeePerItem * res.craftQuantity);
  const totalMarketFee = Math.round(res.marketFeePerItem * res.craftQuantity);
  const totalGrossRevenue = Math.round(sellVal * res.craftQuantity);
  const totalNetRevenue = Math.round(res.totalBatchRevenue);
  const totalCost = Math.round(res.totalBatchCost);
  const roiPercent = totalCost > 0 ? (res.totalBatchProfit / totalCost) * 100 : 0;
  const savedMaterialsAg = Math.round(res.resourceReturnRate * (matRetVal * res.craftQuantity));

  // Stacked breakdown proportions
  const budgetBase = Math.max(totalGrossRevenue, totalCost + totalStationFee + totalMarketFee, 1);
  const matCostTotal = Math.round(res.netMaterialCostPerItem * res.craftQuantity);
  const matShare = Math.min(100, Math.max(4, (matCostTotal / budgetBase) * 100));
  const feeShare = Math.min(100, Math.max(2, ((totalStationFee + totalMarketFee) / budgetBase) * 100));
  const profitShare = isProfitable ? Math.max(0, Math.min(100, 100 - matShare - feeShare)) : 0;

  el("craft-result").innerHTML = `
    <div class="craft-dashboard">
      <!-- Hero Profit Banner -->
      <div class="craft-hero ${isProfitable ? "is-profit" : "is-loss"}">
        <div class="craft-hero-left">
          <div class="craft-hero-tag">
            <span class="hero-dot"></span>
            PROFIT NET TOTAL (${res.craftQuantity} ${res.craftQuantity > 1 ? "unités" : "unité"})
          </div>
          <div class="craft-hero-main">
            <span class="hero-num">${isProfitable ? "+" : ""}${formatExactInt(res.totalBatchProfit)}</span>
            <span class="hero-unit">Ag</span>
          </div>
          <div class="craft-hero-sub">
            <span class="chip-unit-profit">${isProfitable ? "+" : ""}${formatExactInt(res.netProfitPerItem)} Ag / u</span>
            <span class="hero-sep">•</span>
            <span class="chip-roi">ROI <strong>${roiPercent >= 0 ? "+" : ""}${roiPercent.toFixed(1)}%</strong></span>
          </div>
        </div>
        <div class="craft-hero-badge ${isProfitable ? "badge-profit" : "badge-loss"}">
          <span class="badge-arb-icon">${isProfitable ? "▲" : "▼"}</span>
          <span class="badge-status-title">${isProfitable ? "RENTABLE" : "DÉFICITAIRE"}</span>
          <span class="badge-margin-tag">Marge ${res.marginPercent.toFixed(1)}%</span>
        </div>
      </div>

      <!-- Key Metrics 4-Card Grid -->
      <div class="craft-kpi-grid">
        <div class="craft-kpi-item">
          <div class="kpi-item-head">
            <span class="kpi-item-label">Taux de Retour (RRR)</span>
            <span class="kpi-item-badge rrr-badge">${rrrPct}%</span>
          </div>
          <div class="rrr-progress-bar">
            <div class="rrr-progress-fill" style="width: ${Math.min(100, (res.resourceReturnRate / 0.539) * 100).toFixed(1)}%"></div>
          </div>
          <div class="kpi-item-foot">Économie mat. : <strong>~${formatExactInt(savedMaterialsAg)} Ag</strong></div>
        </div>

        <div class="craft-kpi-item">
          <div class="kpi-item-head">
            <span class="kpi-item-label">Coût de Revient / u</span>
            <span class="kpi-item-val">${formatExactInt(res.totalCostPerItem)} Ag</span>
          </div>
          <div class="kpi-item-foot">Coût total lot : <strong>${formatExactInt(totalCost)} Ag</strong></div>
        </div>

        <div class="craft-kpi-item">
          <div class="kpi-item-head">
            <span class="kpi-item-label">Vente Nette Lot</span>
            <span class="kpi-item-val text-gold">${formatExactInt(totalNetRevenue)} Ag</span>
          </div>
          <div class="kpi-item-foot">Vente brute : <strong>${formatExactInt(totalGrossRevenue)} Ag</strong></div>
        </div>

        <div class="craft-kpi-item">
          <div class="kpi-item-head">
            <span class="kpi-item-label">Frais Station &amp; Marché</span>
            <span class="kpi-item-val text-dim">${formatExactInt(totalStationFee + totalMarketFee)} Ag</span>
          </div>
          <div class="kpi-item-foot">Station : ${formatExactInt(totalStationFee)} Ag • Marché (${(res.marketTaxRate * 100).toFixed(1)}%) : ${formatExactInt(totalMarketFee)} Ag</div>
        </div>
      </div>

      <!-- Visual Breakdown Bar -->
      <div class="craft-breakdown-box">
        <div class="breakdown-bar">
          <div class="breakdown-seg seg-mat" style="width: ${matShare.toFixed(1)}%" title="Matériaux nets: ${matShare.toFixed(1)}%"></div>
          <div class="breakdown-seg seg-fee" style="width: ${feeShare.toFixed(1)}%" title="Taxes &amp; station: ${feeShare.toFixed(1)}%"></div>
          ${isProfitable ? `<div class="breakdown-seg seg-profit" style="width: ${profitShare.toFixed(1)}%" title="Profit net: ${profitShare.toFixed(1)}%"></div>` : ""}
        </div>
        <div class="breakdown-legend">
          <span class="leg-entry"><span class="leg-dot dot-mat"></span> Matériaux (${matShare.toFixed(0)}%)</span>
          <span class="leg-entry"><span class="leg-dot dot-fee"></span> Taxes (${feeShare.toFixed(0)}%)</span>
          ${isProfitable ? `<span class="leg-entry"><span class="leg-dot dot-profit"></span> Profit Net (${profitShare.toFixed(0)}%)</span>` : ""}
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

const fetchPriceRows = async (items) => {
  const chunks = [];
  for (let i = 0; i < items.length; i += PRICE_CHUNK_SIZE) {
    chunks.push(items.slice(i, i + PRICE_CHUNK_SIZE));
  }
  const responses = await Promise.all(
    chunks.map(async (chunk) => {
      const url = buildAlbionDataPriceUrl(settings.region, chunk, settings.selectedCities, [1, 2, 3]);
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
    // Tracked mode keeps every tracked item visible (so it can be removed); session mode only priced ones.
    const rows = isSession
      ? items.filter((id) => byId.has(id)).map((id) => byId.get(id))
      : items.map((id) => byId.get(id) || { itemId: id, ...empty });
    lastPriceRows = rows.map((r) => ({ ...r, info: infoById.get(r.itemId) || null }));

    if (isSession && banner) {
      const { total, pricedItems } = computeSessionMarketValue(summarized, qtyByItem);
      const estimated = (latestState?.snapshot?.lootLog || []).reduce(
        (acc, l) => acc + (Number(l.estimatedSilver) || 0),
        0,
      ) + (latestState?.snapshot?.gatheringTotals?.totalEstimatedSilver || 0);
      banner.innerHTML = `🎒 Valeur marché réelle de la session : <strong>${formatExactInt(total)} Ag</strong> • ${pricedItems}/${items.length} objet(s) cotés • estimation catalogue : ${formatExactInt(estimated)} Ag`;
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
            ${isSession ? "<th>Qté</th>" : ""}
            <th>Meilleure Vente</th>
            <th>Meilleur Achat</th>
            <th>Écart Arbitrage</th>
            <th></th>
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
              const thumb = renderItemThumbWithTier(r.itemId, 40);

              let arbHtml = `<span class="dash-empty">—</span>`;
              if (hasSpread) {
                arbHtml = `
                  <div class="arbitrage-pill ${isProfitSpread ? "arb-profit" : "arb-loss"}">
                    <span class="arb-arrow">${isProfitSpread ? "▲" : "▼"}</span>
                    <span class="arb-val">${isProfitSpread ? "+" : ""}${formatExactInt(r.spreadSilver)} Ag</span>
                    <span class="arb-pct">(${isProfitSpread ? "+" : ""}${r.spreadPercent.toFixed(1)}%)</span>
                  </div>
                `;
              }

              return `
            <tr>
              <td>
                <div class="cell-with-icon item-cell">
                  ${thumb}
                  <div class="item-text-group">
                    <strong class="item-display-name">${escapeHtml(name)}</strong>
                    ${name !== r.itemId ? `<span class="item-id-sub">${escapeHtml(r.itemId)}</span>` : ""}
                  </div>
                </div>
              </td>
              ${isSession ? `<td class="qty-cell">×${formatExactInt(qtyByItem[r.itemId] || 0)}</td>` : ""}
              <td>
                ${hasSell ? `
                  <div class="price-stack">
                    <span class="silver-amount">${formatExactInt(r.bestSellPrice)} <span class="ag-tag">Ag</span></span>
                    ${renderCityBadge(r.bestSellCity)}
                  </div>
                ` : `<span class="dash-empty">—</span>`}
              </td>
              <td>
                ${hasBuy ? `
                  <div class="price-stack">
                    <span class="silver-amount">${formatExactInt(r.bestBuyOrder)} <span class="ag-tag">Ag</span></span>
                    ${renderCityBadge(r.bestBuyCity)}
                  </div>
                ` : `<span class="dash-empty">—</span>`}
              </td>
              <td>
                ${arbHtml}
              </td>
              <td>
                <div class="row-actions">
                  <button class="btn btn-craft btn-xs" type="button" data-craft-item="${escapeHtml(r.itemId)}" title="Pré-remplir le calculateur d'artisanat avec cet objet">⚒ Craft</button>
                  ${isSession ? "" : `<button class="btn btn-danger-subtle btn-xs" type="button" data-untrack-item="${escapeHtml(r.itemId)}" title="Retirer des objets suivis">✕</button>`}
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

const prefillCraftingFromItem = (itemId) => {
  const row = lastPriceRows.find((r) => r.itemId === itemId);
  if (!row) return;
  const name = row.info?.nameFr || itemId;
  el("craft-name").value = name;
  const price = row.bestSellPrice > 0 ? row.bestSellPrice : row.bestBuyOrder;
  if (price > 0) el("craft-sell").value = String(Math.round(price));
  if (row.info?.itemValue > 0) el("craft-item-val").value = String(row.info.itemValue);
  updateCraftingSimulation();
  el("craft-form")?.scrollIntoView({ behavior: "smooth", block: "start" });
  showToast(`Calculateur pré-rempli : ${name}${price > 0 ? ` à ${formatExactInt(price)} Ag` : ""}`);
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
  if (remaining.length === 0) {
    showToast("Gardez au moins un objet suivi.");
    return;
  }
  settings = normalizeSettings({ ...settings, trackedItems: remaining });
  await persistSettings();
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
    box.innerHTML = `<div class="search-result-empty">Aucun objet trouvé. Astuce : synchronisez la base complète via 📦 Base.</div>`;
  } else {
    for (const it of results) itemInfoCache.set(it.uniqueName, it);
    box.innerHTML = results
      .map(
        (it) => `
        <button class="search-result-item" type="button" data-add-item="${escapeHtml(it.uniqueName)}">
          ${renderItemThumb(it.uniqueName, 32)}
          <span class="search-result-meta">
            <strong>${escapeHtml(it.nameFr || it.nameEn || it.uniqueName)}</strong>
            <small>${escapeHtml(it.uniqueName)} • T${it.tier}.${it.enchantment}</small>
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
  showToast("Téléchargement de la base d'objets, zones et sorts Albion (ao-bin-dumps)…");
  try {
    const base = "https://raw.githubusercontent.com/ao-data/ao-bin-dumps/master";
    const [itemsResp, worldResp, spellsResp] = await Promise.all([
      fetch(`${base}/formatted/items.json`),
      fetch(`${base}/formatted/world.json`).catch(() => null),
      fetch(`${base}/spells.xml`).catch(() => null),
    ]);
    if (!itemsResp.ok) throw new Error(`HTTP ${itemsResp.status}`);
    const itemsText = await itemsResp.text();
    const worldText = worldResp && worldResp.ok ? await worldResp.text() : null;
    const spellsText = spellsResp && spellsResp.ok ? await spellsResp.text() : null;
    const added = await invokeCmd("import_ao_bin_dumps", {
      jsonText: itemsText,
      worldJson: worldText,
      spellsXml: spellsText,
    });
    itemInfoCache.clear();
    await refreshState(true);
    const spellCount = latestState?.snapshot?.catalogSpellCount || 0;
    showToast(
      `Catalogue synchronisé et mis en cache (+${formatExactInt(added || 0)} entrées, ${formatExactInt(spellCount)} sorts) !`,
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
    res?.clickThrough ? 1 : 0,
    s.snapshots?.length || 0,
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
      showToast("Scénario Photon UDP (combat 5v5, brumes, récolte, butin, marché) injecté !");
    }
  });

  el("btn-compact")?.addEventListener("click", async () => {
    settings.compactOverlay = !settings.compactOverlay;
    await applyWindowPreferences();
    await persistSettings();
  });

  el("btn-pin")?.addEventListener("click", async () => {
    settings.alwaysOnTop = !settings.alwaysOnTop;
    await applyWindowPreferences();
    await persistSettings();
    showToast(settings.alwaysOnTop ? "Fenêtre épinglée au-dessus du jeu" : "Épinglage désactivé");
  });

  el("btn-click-through")?.addEventListener("click", async () => {
    const next = !latestState?.clickThrough;
    try {
      await invokeCmd("set_click_through", { enabled: next });
      await refreshState(true);
      if (next) {
        showToast("Souris traversante activée — Ctrl+Alt+L pour déverrouiller.");
      }
    } catch (err) {
      showToast(`Mode souris traversante indisponible : ${err}`);
    }
  });

  let opacityPersistTimer = null;
  el("overlay-opacity")?.addEventListener("input", (ev) => {
    settings.overlayOpacity = Math.max(35, Math.min(100, Math.round(Number(ev.target.value) || 96)));
    applyOverlayOpacity();
    if (opacityPersistTimer) clearTimeout(opacityPersistTimer);
    opacityPersistTimer = setTimeout(() => persistSettings(), 400);
  });

  el("btn-min")?.addEventListener("click", () => tauriWindow?.minimize?.());
  el("btn-close")?.addEventListener("click", () => tauriWindow?.close?.());

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
      showToast("Classement copié dans le presse-papiers !");
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

  el("btn-reset-session")?.addEventListener("click", async () => {
    const res = await invokeCmd("reset_session");
    if (res) {
      latestState = res;
      lastStateSignature = computeStateSignature(res);
      renderAll();
      showToast("Compteurs de session réinitialisés.");
    }
  });

  el("btn-export-csv")?.addEventListener("click", async () => {
    const csv = await invokeCmd("export_loot_csv");
    if (csv) {
      try {
        await navigator.clipboard.writeText(csv);
        showToast("Journal de butin CSV copié dans le presse-papiers !");
      } catch {
        showToast("Export CSV prêt.");
      }
    }
  });

  // Crafting calculator reactive inputs
  el("craft-form")?.addEventListener("input", () => {
    updateCraftingSimulation();
  });

  el("market-region")?.addEventListener("change", (ev) => {
    settings.region = ev.target.value || "europe";
    persistSettings();
  });

  el("btn-fetch-prices")?.addEventListener("click", () => {
    fetchAlbionDataPrices();
  });

  // Preset bonus buttons (0% Île, 18% Cité, 47.9% Focus)
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
  // Prevent the input from losing focus on mousedown; the click handler performs the selection.
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
    priceWrap.innerHTML = `<div class="empty-state">${settings.trackedItems.length} objet(s) suivi(s). Cliquez sur <strong>🌐 Actualiser Prix API</strong> ou recherchez un objet pour l'ajouter.</div>`;
  }

  setInterval(() => refreshState(false), settings.autoRefreshMs);
};

boot();
