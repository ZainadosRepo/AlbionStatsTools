import {
  buildAlbionDataPriceUrl,
  formatDuration,
  formatExactInt,
  formatMeterClipboard,
  formatNumber,
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
let toastTimer = null;

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

const persistSettings = async () => {
  try {
    await invokeCmd("save_settings", { settings });
  } catch (e) {
    console.warn("save_settings:", e);
  }
};

const applyWindowPreferences = async () => {
  const shell = el("app");
  shell?.classList.toggle("compact-overlay", settings.compactOverlay);
  el("btn-compact")?.classList.toggle("active", settings.compactOverlay);
  el("btn-pin")?.classList.toggle("active", settings.alwaysOnTop);
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
    badge.textContent = isLive ? "● Capture UDP 5056" : "● Prêt / Simulateur";
    badge.title = cap.detail || "";
  }

  const lp = snap.localPlayer;
  const guildTag = lp.guildName ? ` [${lp.guildName}]` : "";
  el("header-player-name").textContent = `${lp.username}${guildTag}`;
  el("header-map-name").textContent = lp.currentMapNameFr;
  el("header-silver").textContent = `${formatNumber(lp.silver)} Ag`;
};

const renderSnapshotSelect = () => {
  const select = el("snapshot-select");
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
      return `
        <div class="meter-row ${isSel ? "selected" : ""}" data-player-name="${escapeHtml(p.name)}">
          <div class="meter-bar-fill meter-bar-${escapeHtml(settings.meterMode)}" style="width: ${p.barPercent.toFixed(1)}%"></div>
          <div class="meter-row-content">
            <div class="meter-left">
              <span class="meter-rank">${p.rank}</span>
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
              <td><strong>${escapeHtml(s.nameFr)}</strong></td>
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
            <td><strong>${escapeHtml(g.nameFr)}</strong></td>
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
              <span>${escapeHtml(sl.nameFr)}</span>
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
              <td>${escapeHtml(l.nameFr)}</td>
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
  if (orders.length === 0 && mails.length === 0) {
    el("captured-market-rows").innerHTML = `<div class="empty-state">Aucun ordre ou courrier du marché capturé.</div>`;
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
            <td><span class="badge-subtle">${o.isRequest ? "Achat" : "Offre Q" + o.qualityLevel}</span></td>
            <td><strong>${escapeHtml(o.itemNameFr)}</strong></td>
            <td>×${o.amount}</td>
            <td>${formatExactInt(o.unitPriceSilver)} Ag</td>
            <td>${formatExactInt(o.totalPriceSilver)} Ag</td>
          </tr>
        `,
          )
          .join("")}
        ${mails
          .map(
            (m) => `
          <tr>
            <td><span class="status-badge status-live">Courrier Vendu</span></td>
            <td><strong>${escapeHtml(m.itemNameFr)}</strong></td>
            <td>×${m.amount}</td>
            <td>${formatExactInt(m.unitPriceSilver)} Ag</td>
            <td>${formatExactInt(m.totalPriceSilver)} Ag</td>
          </tr>
        `,
          )
          .join("")}
      </tbody>
    </table>
  `;
};

const updateCraftingSimulation = async () => {
  const input = {
    itemName: el("craft-name").value || "Objet",
    craftQuantity: Math.max(1, Number(el("craft-qty").value) || 1),
    sellUnitPrice: Math.max(0, Number(el("craft-sell").value) || 0),
    itemValue: Math.max(0, Number(el("craft-item-val").value) || 0),
    productionBonusPercent: Math.max(0, Number(el("craft-bonus").value) || 0),
    usageFeePer100Nutrition: Math.max(0, Number(el("craft-fee").value) || 0),
    isPremium: Boolean(el("craft-premium").checked),
    isSellOrder: Boolean(el("craft-sell-order").checked),
    journalProfitPerItem: Math.max(0, Number(el("craft-journal").value) || 0),
    materials: [
      {
        name: "Ressources principales",
        quantityPerItem: 1,
        unitPrice: Math.max(0, Number(el("craft-mat-ret").value) || 0),
        isReturnable: true,
      },
      {
        name: "Artefact / Cœur",
        quantityPerItem: 1,
        unitPrice: Math.max(0, Number(el("craft-mat-art").value) || 0),
        isReturnable: false,
      },
    ],
  };

  const res = await invokeCmd("calculate_crafting", { input });
  if (!res) return;

  const profitColor = res.netProfitPerItem >= 0 ? "#56e39f" : "#ff7b7b";
  el("craft-result").innerHTML = `
    <div>
      <div class="kpi-label">Taux de Retour (RRR)</div>
      <strong>${(res.resourceReturnRate * 100).toFixed(2)}%</strong>
      <div class="kpi-foot">Taxe station : ${formatExactInt(res.stationFeePerItem)} Ag</div>
    </div>
    <div>
      <div class="kpi-label">Coût de Revient Net / u</div>
      <strong>${formatExactInt(res.totalCostPerItem)} Ag</strong>
      <div class="kpi-foot">Taxe marché : ${(res.marketTaxRate * 100).toFixed(1)}%</div>
    </div>
    <div>
      <div class="kpi-label">Profit Net Lot (×${res.craftQuantity})</div>
      <strong style="color:${profitColor}">${formatExactInt(res.totalBatchProfit)} Ag</strong>
      <div class="kpi-foot">Marge : ${res.marginPercent.toFixed(1)}% (${formatExactInt(res.netProfitPerItem)}/u)</div>
    </div>
  `;
};

const fetchAlbionDataPrices = async () => {
  const wrap = el("api-price-rows");
  wrap.innerHTML = `<div class="empty-state">Interrogation de l'API Albion Data Project (${escapeHtml(settings.region)})…</div>`;
  const trackedItems = [
    "T7_2H_CLEAVER_HELL@1",
    "T8_MAIN_HOLYSTAFF_AVALON@2",
    "T8_2H_BOW_AVALON@2",
    "T8_2H_INFERNOSTAFF_MORGANA@2",
    "T8_BAG_INSIGHT",
  ];
  const url = buildAlbionDataPriceUrl(settings.region, trackedItems, settings.selectedCities, [1, 2, 3]);
  try {
    const resp = await fetch(url);
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    const data = await resp.json();
    const summarized = summarizeMarketPrices(data);
    if (summarized.length === 0) {
      wrap.innerHTML = `<div class="empty-state">Aucun prix récent retourné pour cette région.</div>`;
      return;
    }
    wrap.innerHTML = `
      <table class="data-table">
        <thead>
          <tr>
            <th>Identifiant</th>
            <th>Meilleure Vente</th>
            <th>Meilleur Achat</th>
            <th>Écart Arbitrage</th>
          </tr>
        </thead>
        <tbody>
          ${summarized
            .map(
              (r) => `
            <tr>
              <td><strong>${escapeHtml(r.itemId)}</strong></td>
              <td>${formatExactInt(r.bestSellPrice)} Ag (${escapeHtml(r.bestSellCity)})</td>
              <td>${formatExactInt(r.bestBuyOrder)} Ag (${escapeHtml(r.bestBuyCity)})</td>
              <td>${formatExactInt(r.spreadSilver)} Ag (${r.spreadPercent.toFixed(1)}%)</td>
            </tr>
          `,
            )
            .join("")}
        </tbody>
      </table>
    `;
  } catch (err) {
    wrap.innerHTML = `<div class="empty-state">API hors-ligne ou indisponible (${escapeHtml(err.message)}).</div>`;
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

const refreshState = async () => {
  const res = await invokeCmd("get_state");
  if (res) {
    latestState = res;
    renderAll();
  }
};

const initEvents = () => {
  // Navigation tabs
  document.querySelectorAll(".nav-tab").forEach((btn) => {
    btn.addEventListener("click", () => {
      const target = btn.getAttribute("data-tab");
      document.querySelectorAll(".nav-tab").forEach((b) => b.classList.toggle("active", b === btn));
      document.querySelectorAll(".tab-panel").forEach((p) => {
        p.classList.toggle("active", p.id === `tab-${target}`);
      });
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

  // Snapshot selector
  el("snapshot-select")?.addEventListener("change", (ev) => {
    selectedSnapshotId = ev.target.value || "live";
    renderCombatTab();
  });

  // Titlebar controls
  el("btn-simulate")?.addEventListener("click", async () => {
    const res = await invokeCmd("simulate_session");
    if (res) {
      latestState = res;
      selectedSnapshotId = "live";
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
      await refreshState();
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
      renderAll();
      showToast("Compteur de combat réinitialisé.");
    }
  });

  el("btn-reset-session")?.addEventListener("click", async () => {
    const res = await invokeCmd("reset_session");
    if (res) {
      latestState = res;
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
  await refreshState();
  await updateCraftingSimulation();

  setInterval(refreshState, settings.autoRefreshMs);
};

boot();
