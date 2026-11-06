//! Deterministic single-owner state machine for Albion Online telemetry:
//! Combat Damage/Heal/Overheal/Taken meter, Session & Faction KPIs, Dungeons & Mists,
//! Gathering, Party Inspect, Loot Logger (with 2-second deduplication), and Economy.

use crate::catalog::Catalog;
use crate::events::{
    decode_photon_message, encode_telemetry_event_to_photon, AlbionTelemetryEvent,
    CharacterEquipment, CityFaction, EffectOrigin, EffectType, HealthTick,
};
use crate::photon::{build_photon_fragmented_packets, build_photon_udp_packet, PhotonParser};
use crate::protocol16::{FixPoint, Guid16};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

const MAX_LOOT_LOG: usize = 250;
const MAX_GATHERING_LOG: usize = 200;
const MAX_DUNGEON_RUNS: usize = 60;
const MAX_MARKET_ENTRIES: usize = 150;
const MAX_SNAPSHOTS: usize = 30;
const LOOT_DEDUP_WINDOW_MS: u64 = 2_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpellStat {
    pub spell_index: i32,
    pub unique_name: String,
    pub name_fr: String,
    pub name_en: String,
    pub category: String,
    pub ticks: u32,
    pub damage: i64,
    pub heal: i64,
    pub overhealed: i64,
    pub share_percent: f64,
    pub dps: f64,
    pub hps: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DamageMeterPlayer {
    pub object_id: i64,
    pub guid: String,
    pub name: String,
    pub guild_name: String,
    pub is_self: bool,
    pub weapon_unique_name: String,
    pub weapon_name_fr: String,
    pub weapon_name_en: String,
    pub damage: i64,
    pub heal: i64,
    pub overhealed: i64,
    pub taken_damage: i64,
    pub combat_seconds: f64,
    pub dps: f64,
    pub hps: f64,
    pub overheal_percent: f64,
    pub damage_percentage: f64,
    pub heal_percentage: f64,
    pub taken_damage_percentage: f64,
    pub spells: Vec<SpellStat>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeterSnapshot {
    pub id: String,
    pub label: String,
    pub timestamp_ms: u64,
    pub map_name_fr: String,
    pub combat_seconds: f64,
    pub total_damage: i64,
    pub total_heal: i64,
    pub total_taken: i64,
    pub players: Vec<DamageMeterPlayer>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FactionCitySummary {
    pub city: String,
    pub coins: f64,
    pub rank_points: f64,
    pub flag_points: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStats {
    pub elapsed_seconds: f64,
    pub fame: f64,
    pub fame_per_hour: f64,
    pub zone_fame: f64,
    pub satchel_fame: f64,
    pub silver: f64,
    pub silver_per_hour: f64,
    pub guild_tax_paid: f64,
    pub cluster_tax_paid: f64,
    pub respec_points: f64,
    pub respec_per_hour: f64,
    pub respec_paid_silver: f64,
    pub might: f64,
    pub might_per_hour: f64,
    pub favor: f64,
    pub favor_per_hour: f64,
    pub season_points: i32,
    pub repair_costs: f64,
    pub kills: u32,
    pub deaths: u32,
    pub factions: Vec<FactionCitySummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DungeonRun {
    pub id: u64,
    pub cluster_index: String,
    pub name_fr: String,
    pub name_en: String,
    pub kind: String,
    pub tier: u8,
    pub is_active: bool,
    pub started_at_ms: u64,
    pub duration_seconds: f64,
    pub fame: f64,
    pub fame_per_hour: f64,
    pub silver: f64,
    pub respec: f64,
    pub might: f64,
    pub favor: f64,
    pub chests_opened: u32,
    pub caged_wisps_seen: u32,
    pub caged_wisps_opened: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatheringEntry {
    pub id: u64,
    pub timestamp_ms: u64,
    pub item_index: i32,
    pub unique_name: String,
    pub name_fr: String,
    pub name_en: String,
    pub resource_kind: String,
    pub tier: u8,
    pub enchantment: u8,
    pub standard_amount: i32,
    pub collector_bonus: i32,
    pub premium_bonus: i32,
    pub total_amount: i32,
    pub estimated_silver: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatheringTotals {
    pub total_nodes: u32,
    pub total_resources: i64,
    pub total_estimated_silver: i64,
    pub wood: i64,
    pub ore: i64,
    pub fiber: i64,
    pub hide: i64,
    pub rock: i64,
    pub fish: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LootLogEntry {
    pub id: u64,
    pub timestamp_ms: u64,
    pub looted_by: String,
    pub looted_from: String,
    pub item_index: i32,
    pub unique_name: String,
    pub name_fr: String,
    pub name_en: String,
    pub quantity: i32,
    pub is_silver: bool,
    pub estimated_silver: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EquippedSlotView {
    pub slot: String,
    pub item_index: i32,
    pub unique_name: String,
    pub name_fr: String,
    pub name_en: String,
    pub tier: u8,
    pub enchantment: u8,
    pub item_value: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectedPlayer {
    pub object_id: i64,
    pub guid: String,
    pub name: String,
    pub guild_name: String,
    pub is_party_member: bool,
    pub is_self: bool,
    pub estimated_loadout_value: i64,
    pub slots: Vec<EquippedSlotView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketOrderEntry {
    pub id: i64,
    pub is_request: bool,
    pub item_unique_name: String,
    pub item_name_fr: String,
    pub amount: i32,
    pub unit_price_silver: f64,
    pub total_price_silver: f64,
    pub quality_level: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailNotificationEntry {
    pub mail_id: i64,
    pub item_unique_name: String,
    pub item_name_fr: String,
    pub amount: i32,
    pub unit_price_silver: f64,
    pub total_price_silver: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TradeLogEntry {
    pub id: u64,
    pub timestamp_ms: u64,
    pub player1_silver: f64,
    pub player2_silver: f64,
    pub sales_tax: f64,
    pub item_count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalPlayerProfile {
    pub object_id: i64,
    pub username: String,
    pub guild_name: String,
    pub alliance_name: String,
    pub current_map_index: String,
    pub current_map_name_fr: String,
    pub silver: f64,
    pub gold: f64,
    pub focus_points: f64,
    pub max_focus_points: f64,
    pub learning_points: f64,
    pub reputation: f64,
    pub is_respec_active: bool,
    pub in_combat: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkDiagnostics {
    pub packets_processed: u64,
    pub commands_processed: u64,
    pub telemetry_events_processed: u64,
    pub crc_mismatches: u64,
    pub pending_fragments: usize,
}

/// Complete immutable snapshot returned to the UI on poll or command completion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSnapshot {
    pub catalog_version: String,
    pub catalog_item_count: usize,
    pub catalog_spell_count: usize,
    pub local_player: LocalPlayerProfile,
    pub combat_seconds: f64,
    pub total_party_damage: i64,
    pub total_party_heal: i64,
    pub total_party_taken: i64,
    pub meter_players: Vec<DamageMeterPlayer>,
    pub snapshots: Vec<MeterSnapshot>,
    pub session: SessionStats,
    pub dungeons: Vec<DungeonRun>,
    pub gathering_totals: GatheringTotals,
    pub gathering_log: Vec<GatheringEntry>,
    pub loot_log: Vec<LootLogEntry>,
    pub inspected_players: Vec<InspectedPlayer>,
    pub market_orders: Vec<MarketOrderEntry>,
    pub mail_notifications: Vec<MailNotificationEntry>,
    pub trades: Vec<TradeLogEntry>,
    pub network: NetworkDiagnostics,
}

/// Subset of session history persisted to disk across application restarts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedHistory {
    #[serde(default)]
    pub snapshots: Vec<MeterSnapshot>,
    #[serde(default)]
    pub dungeons: Vec<DungeonRun>,
    #[serde(default)]
    pub gathering_totals: GatheringTotals,
    #[serde(default)]
    pub gathering_log: Vec<GatheringEntry>,
    #[serde(default)]
    pub loot_log: Vec<LootLogEntry>,
}

#[derive(Debug, Clone, Default)]
struct InternalSpellAccum {
    ticks: u32,
    damage: i64,
    heal: i64,
    overhealed: i64,
}

#[derive(Debug, Clone)]
struct InternalEntity {
    object_id: i64,
    guid: Guid16,
    name: String,
    guild_name: String,
    is_party: bool,
    is_self: bool,
    equipment: CharacterEquipment,
    damage: i64,
    heal: i64,
    overhealed: i64,
    taken_damage: i64,
    first_combat_ms: Option<u64>,
    last_combat_ms: Option<u64>,
    spells: BTreeMap<i32, InternalSpellAccum>,
}

#[derive(Debug, Clone)]
struct DiscoveredItem {
    item_id: i32,
    quantity: i32,
    estimated_market_value: i64,
}

/// Single-owner telemetry analyzer state machine.
pub struct AnalyzerEngine {
    pub catalog: Catalog,
    pub photon: PhotonParser,
    session_start_ms: u64,
    last_now_ms: u64,
    combat_start_ms: Option<u64>,
    combat_end_ms: Option<u64>,
    last_auto_snapshot_combat_ms: Option<u64>,
    dirty_history: bool,
    pub telemetry_events_processed: u64,

    local_player: LocalPlayerProfile,
    entities: BTreeMap<i64, InternalEntity>,
    guid_to_object_id: HashMap<Guid16, i64>,
    party_guids: HashMap<Guid16, String>,
    last_player_health: HashMap<i64, f64>,
    discovered_items: HashMap<i64, DiscoveredItem>,

    snapshots: Vec<MeterSnapshot>,
    session: SessionStats,
    faction_map: BTreeMap<CityFaction, (f64, f64, f64)>,
    dungeons: Vec<DungeonRun>,
    gathering_totals: GatheringTotals,
    gathering_log: Vec<GatheringEntry>,
    loot_log: Vec<LootLogEntry>,
    market_orders: Vec<MarketOrderEntry>,
    mail_notifications: Vec<MailNotificationEntry>,
    trades: Vec<TradeLogEntry>,
    next_seq_id: u64,
}

#[derive(Debug, Deserialize)]
struct RawAuctionJson {
    #[serde(rename = "Id", default)]
    id: i64,
    #[serde(rename = "UnitPriceSilver", default)]
    unit_price_silver: i64,
    #[serde(rename = "TotalPriceSilver", default)]
    total_price_silver: i64,
    #[serde(rename = "Amount", default)]
    amount: i32,
    #[serde(rename = "ItemTypeId", default)]
    item_type_id: String,
    #[serde(rename = "QualityLevel", default)]
    quality_level: u8,
}

impl AnalyzerEngine {
    pub fn new(catalog: Catalog, now_ms: u64) -> Self {
        let mut faction_map = BTreeMap::new();
        for city in [
            CityFaction::Martlock,
            CityFaction::Lymhurst,
            CityFaction::Bridgewatch,
            CityFaction::FortSterling,
            CityFaction::Thetford,
            CityFaction::Caerleon,
        ] {
            faction_map.insert(city, (0.0, 0.0, 0.0));
        }

        Self {
            catalog,
            photon: PhotonParser::new(),
            session_start_ms: now_ms,
            last_now_ms: now_ms,
            combat_start_ms: None,
            combat_end_ms: None,
            last_auto_snapshot_combat_ms: None,
            dirty_history: false,
            telemetry_events_processed: 0,
            local_player: LocalPlayerProfile {
                object_id: 0,
                username: "En attente de connexion…".into(),
                guild_name: String::new(),
                alliance_name: String::new(),
                current_map_index: "3005".into(),
                current_map_name_fr: "Caerleon".into(),
                silver: 0.0,
                gold: 0.0,
                focus_points: 0.0,
                max_focus_points: 30_000.0,
                learning_points: 0.0,
                reputation: 0.0,
                is_respec_active: false,
                in_combat: false,
            },
            entities: BTreeMap::new(),
            guid_to_object_id: HashMap::new(),
            party_guids: HashMap::new(),
            last_player_health: HashMap::new(),
            discovered_items: HashMap::new(),
            snapshots: Vec::new(),
            session: SessionStats {
                elapsed_seconds: 0.0,
                fame: 0.0,
                fame_per_hour: 0.0,
                zone_fame: 0.0,
                satchel_fame: 0.0,
                silver: 0.0,
                silver_per_hour: 0.0,
                guild_tax_paid: 0.0,
                cluster_tax_paid: 0.0,
                respec_points: 0.0,
                respec_per_hour: 0.0,
                respec_paid_silver: 0.0,
                might: 0.0,
                might_per_hour: 0.0,
                favor: 0.0,
                favor_per_hour: 0.0,
                season_points: 0,
                repair_costs: 0.0,
                kills: 0,
                deaths: 0,
                factions: Vec::new(),
            },
            faction_map,
            dungeons: Vec::new(),
            gathering_totals: GatheringTotals::default(),
            gathering_log: Vec::new(),
            loot_log: Vec::new(),
            market_orders: Vec::new(),
            mail_notifications: Vec::new(),
            trades: Vec::new(),
            next_seq_id: 1,
        }
    }

    /// Returns `true` and clears the dirty flag if persisted history changed since the last flush.
    pub fn take_dirty_history(&mut self) -> bool {
        std::mem::replace(&mut self.dirty_history, false)
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_seq_id;
        self.next_seq_id += 1;
        id
    }

    /// Parses a raw Photon UDP datagram payload and dispatches all decoded Albion events.
    pub fn ingest_udp_payload(&mut self, payload: &[u8], now_ms: u64) -> usize {
        self.last_now_ms = self.last_now_ms.max(now_ms);
        let messages = self.photon.parse_udp_payload(payload);
        let mut decoded_count = 0usize;
        for msg in &messages {
            if let Some(ev) = decode_photon_message(msg) {
                self.ingest_telemetry_event(ev, now_ms);
                decoded_count += 1;
            }
        }
        decoded_count
    }

    /// Processes a single [`AlbionTelemetryEvent`] and updates all tracking subsystems.
    pub fn ingest_telemetry_event(&mut self, event: AlbionTelemetryEvent, now_ms: u64) {
        self.last_now_ms = self.last_now_ms.max(now_ms);
        self.telemetry_events_processed += 1;

        match event {
            AlbionTelemetryEvent::JoinResponse {
                user_object_id,
                user_guid,
                username,
                map_index,
                main_map_index: _,
                guild_name,
                alliance_name,
                current_focus_points,
                max_focus_points,
                silver,
                gold,
                learning_points,
                reputation,
                is_respec_active,
            } => {
                let map_name_fr = self
                    .catalog
                    .cluster(&map_index)
                    .map(|c| c.name_fr.clone())
                    .unwrap_or_else(|| format!("Zone {map_index}"));

                self.local_player = LocalPlayerProfile {
                    object_id: user_object_id,
                    username: username.clone(),
                    guild_name: guild_name.clone(),
                    alliance_name,
                    current_map_index: map_index,
                    current_map_name_fr: map_name_fr,
                    silver: silver.as_f64(),
                    gold: gold.as_f64(),
                    focus_points: current_focus_points,
                    max_focus_points,
                    learning_points: learning_points.as_f64(),
                    reputation,
                    is_respec_active,
                    in_combat: false,
                };

                self.party_guids.insert(user_guid, username.clone());
                self.guid_to_object_id.insert(user_guid, user_object_id);
                let existing_equip = self
                    .entities
                    .get(&user_object_id)
                    .map(|e| e.equipment.clone())
                    .unwrap_or_default();
                let ent = self.ensure_entity(user_object_id);
                ent.guid = user_guid;
                ent.name = username;
                ent.guild_name = guild_name;
                ent.is_party = true;
                ent.is_self = true;
                ent.equipment = existing_equip;
            }
            AlbionTelemetryEvent::NewCharacter {
                object_id,
                name,
                guid,
                guild_name,
                equipment,
            } => {
                self.guid_to_object_id.insert(guid, object_id);
                let is_party = self.party_guids.contains_key(&guid)
                    || object_id == self.local_player.object_id;
                let is_self = object_id == self.local_player.object_id;
                let ent = self.ensure_entity(object_id);
                ent.guid = guid;
                if !name.is_empty() {
                    ent.name = name;
                }
                if !guild_name.is_empty() {
                    ent.guild_name = guild_name;
                }
                ent.is_party = is_party;
                ent.is_self = is_self;
                ent.equipment = equipment;
            }
            AlbionTelemetryEvent::CharacterEquipmentChanged {
                object_id,
                equipment,
            } => {
                let ent = self.ensure_entity(object_id);
                ent.equipment = equipment;
            }
            AlbionTelemetryEvent::PartyJoined {
                party_object_id: _,
                members,
            } => {
                for (guid, username) in members {
                    self.party_guids.insert(guid, username.clone());
                    if let Some(&obj_id) = self.guid_to_object_id.get(&guid) {
                        if let Some(ent) = self.entities.get_mut(&obj_id) {
                            ent.is_party = true;
                            if ent.name.is_empty() {
                                ent.name = username;
                            }
                        }
                    }
                }
            }
            AlbionTelemetryEvent::PartyPlayerJoined {
                party_object_id: _,
                user_guid,
                username,
            } => {
                self.party_guids.insert(user_guid, username.clone());
                if let Some(&obj_id) = self.guid_to_object_id.get(&user_guid) {
                    if let Some(ent) = self.entities.get_mut(&obj_id) {
                        ent.is_party = true;
                        if ent.name.is_empty() {
                            ent.name = username;
                        }
                    }
                }
            }
            AlbionTelemetryEvent::PartyPlayerLeft {
                party_object_id: _,
                user_guid,
            } => {
                self.party_guids.remove(&user_guid);
                if let Some(&obj_id) = self.guid_to_object_id.get(&user_guid) {
                    if let Some(ent) = self.entities.get_mut(&obj_id) {
                        if !ent.is_self {
                            ent.is_party = false;
                        }
                    }
                }
            }
            AlbionTelemetryEvent::PartyDisbanded { .. } => {
                self.party_guids.clear();
                for ent in self.entities.values_mut() {
                    ent.is_party = ent.is_self;
                }
            }
            AlbionTelemetryEvent::ActiveSpellEffectsUpdate {
                causer_id,
                spell_indices,
            } => {
                // Weapon inference (`DetectUsedWeapon`) when main_hand is not yet known
                let mut inferred = None;
                for &sp in &spell_indices {
                    if let Some(weapon) = self.catalog.infer_weapon_from_spell(sp) {
                        inferred = Some(weapon.index);
                        break;
                    }
                }
                if let Some(weapon_idx) = inferred {
                    let ent = self.ensure_entity(causer_id);
                    if ent.equipment.main_hand == 0 {
                        ent.equipment.main_hand = weapon_idx;
                    }
                }
            }
            AlbionTelemetryEvent::InCombatStateUpdate {
                object_id,
                in_active_combat,
                in_passive_combat,
            } => {
                if object_id == self.local_player.object_id {
                    let was_in_combat = self.local_player.in_combat;
                    let now_in_combat = in_active_combat || in_passive_combat;
                    self.local_player.in_combat = now_in_combat;
                    if was_in_combat && !now_in_combat {
                        self.maybe_auto_snapshot_combat(now_ms);
                    }
                }
            }
            AlbionTelemetryEvent::HealthUpdate(tick) => {
                self.process_health_tick(&tick, now_ms);
            }
            AlbionTelemetryEvent::HealthUpdates { ticks } => {
                for tick in &ticks {
                    self.process_health_tick(tick, now_ms);
                }
            }
            AlbionTelemetryEvent::UpdateFame {
                fame_with_zone_multiplier,
                zone_fame,
                satchel_fame,
                ..
            } => {
                let gained = fame_with_zone_multiplier.as_f64().max(0.0);
                self.session.fame += gained;
                self.session.zone_fame += zone_fame.as_f64().max(0.0);
                self.session.satchel_fame += satchel_fame.as_f64().max(0.0);
                if let Some(run) = self.dungeons.first_mut().filter(|d| d.is_active) {
                    run.fame += gained;
                }
            }
            AlbionTelemetryEvent::TakeSilver {
                yield_pre_tax,
                guild_tax,
                cluster_tax,
                ..
            } => {
                let pre = yield_pre_tax.as_f64().max(0.0);
                let g_tax = guild_tax.as_f64().max(0.0);
                let c_tax = cluster_tax.as_f64().max(0.0);
                let net = (pre - g_tax - c_tax).max(0.0);
                self.session.silver += net;
                self.session.guild_tax_paid += g_tax;
                self.session.cluster_tax_paid += c_tax;
                if let Some(run) = self.dungeons.first_mut().filter(|d| d.is_active) {
                    run.silver += net;
                }
            }
            AlbionTelemetryEvent::UpdateMoney {
                current_player_silver,
                ..
            } => {
                self.local_player.silver = current_player_silver.as_f64();
            }
            AlbionTelemetryEvent::UpdateReSpecPoints {
                gained_respec_points,
                paid_silver,
                ..
            } => {
                let gained = gained_respec_points.as_f64().max(0.0);
                self.session.respec_points += gained;
                self.session.respec_paid_silver += paid_silver.as_f64().max(0.0);
                if let Some(run) = self.dungeons.first_mut().filter(|d| d.is_active) {
                    run.respec += gained;
                }
            }
            AlbionTelemetryEvent::MightAndFavorReceived { might, favor, .. } => {
                let m = might.as_f64().max(0.0);
                let f = favor.as_f64().max(0.0);
                self.session.might += m;
                self.session.favor += f;
                if let Some(run) = self.dungeons.first_mut().filter(|d| d.is_active) {
                    run.might += m;
                    run.favor += f;
                }
            }
            AlbionTelemetryEvent::UpdateCurrency {
                city_faction,
                gained_faction_coins,
                faction_rank_points,
                ..
            } => {
                if city_faction != CityFaction::Unknown {
                    let entry = self
                        .faction_map
                        .entry(city_faction)
                        .or_insert((0.0, 0.0, 0.0));
                    entry.0 += gained_faction_coins.as_f64().max(0.0);
                    entry.1 += faction_rank_points.as_f64().max(0.0);
                }
            }
            AlbionTelemetryEvent::UpdateFactionStanding {
                city_faction,
                gained_flag_points,
                ..
            } => {
                if city_faction != CityFaction::Unknown {
                    let entry = self
                        .faction_map
                        .entry(city_faction)
                        .or_insert((0.0, 0.0, 0.0));
                    entry.2 += gained_flag_points.as_f64().max(0.0);
                }
            }
            AlbionTelemetryEvent::ReceivedGvgSeasonPoints { season_points } => {
                self.session.season_points += season_points;
            }
            AlbionTelemetryEvent::RepairEquipmentRequest { repair_cost, .. } => {
                self.session.repair_costs += repair_cost.as_f64().max(0.0);
            }
            AlbionTelemetryEvent::Died {
                died_player,
                killed_by,
                ..
            } => {
                if !self.local_player.username.is_empty() {
                    if died_player.eq_ignore_ascii_case(&self.local_player.username) {
                        self.session.deaths += 1;
                    } else if killed_by.eq_ignore_ascii_case(&self.local_player.username) {
                        self.session.kills += 1;
                    }
                }
            }
            AlbionTelemetryEvent::ChangeClusterResponse {
                index,
                world_map_data_type,
                island_name,
                mists_dungeon_tier,
            } => {
                self.handle_cluster_change(
                    &index,
                    &world_map_data_type,
                    &island_name,
                    mists_dungeon_tier,
                    now_ms,
                );
            }
            AlbionTelemetryEvent::LootChestOpened { .. } => {
                if let Some(run) = self.dungeons.first_mut().filter(|d| d.is_active) {
                    run.chests_opened += 1;
                }
            }
            AlbionTelemetryEvent::MistsCagedWisp { opened, .. } => {
                if let Some(run) = self.dungeons.first_mut().filter(|d| d.is_active) {
                    if opened {
                        run.caged_wisps_opened += 1;
                    } else {
                        run.caged_wisps_seen += 1;
                    }
                }
            }
            AlbionTelemetryEvent::HarvestFinished {
                item_id,
                standard_amount,
                collector_bonus_amount,
                premium_bonus_amount,
                ..
            } => {
                self.handle_harvest(
                    item_id,
                    standard_amount,
                    collector_bonus_amount,
                    premium_bonus_amount,
                    now_ms,
                );
            }
            AlbionTelemetryEvent::NewDiscoveredItem {
                object_id,
                item_id,
                quantity,
                estimated_market_value,
                ..
            } => {
                self.discovered_items.insert(
                    object_id,
                    DiscoveredItem {
                        item_id,
                        quantity,
                        estimated_market_value,
                    },
                );
            }
            AlbionTelemetryEvent::OtherGrabbedLoot {
                object_id,
                looted_from_name,
                looted_by_name,
                is_silver,
                item_index,
                quantity,
            } => {
                self.handle_loot_grabbed(
                    object_id,
                    looted_from_name,
                    looted_by_name,
                    is_silver,
                    item_index,
                    quantity,
                    now_ms,
                );
            }
            AlbionTelemetryEvent::AuctionEntriesResponse {
                is_requests,
                raw_json_entries,
            } => {
                self.handle_auction_entries(is_requests, &raw_json_entries);
            }
            AlbionTelemetryEvent::ReadMailResponse { mail_id, content } => {
                self.handle_mail_response(mail_id, &content);
            }
            AlbionTelemetryEvent::TradeFinished {
                player1_silver,
                player2_silver,
                sales_tax,
                item_count,
            } => {
                let id = self.alloc_id();
                self.trades.insert(
                    0,
                    TradeLogEntry {
                        id,
                        timestamp_ms: now_ms,
                        player1_silver: player1_silver.as_f64(),
                        player2_silver: player2_silver.as_f64(),
                        sales_tax: sales_tax.as_f64(),
                        item_count,
                    },
                );
                if self.trades.len() > MAX_MARKET_ENTRIES {
                    self.trades.truncate(MAX_MARKET_ENTRIES);
                }
            }
            AlbionTelemetryEvent::Leave { .. } | AlbionTelemetryEvent::NewLoot { .. } => {}
        }
    }

    fn ensure_entity(&mut self, object_id: i64) -> &mut InternalEntity {
        let is_self = object_id != 0 && object_id == self.local_player.object_id;
        self.entities
            .entry(object_id)
            .or_insert_with(|| InternalEntity {
                object_id,
                guid: Guid16::default(),
                name: if is_self {
                    self.local_player.username.clone()
                } else {
                    format!("Joueur #{object_id}")
                },
                guild_name: String::new(),
                is_party: is_self,
                is_self,
                equipment: CharacterEquipment::default(),
                damage: 0,
                heal: 0,
                overhealed: 0,
                taken_damage: 0,
                first_combat_ms: None,
                last_combat_ms: None,
                spells: BTreeMap::new(),
            })
    }

    fn process_health_tick(&mut self, tick: &HealthTick, now_ms: u64) {
        if tick.health_change.abs() < 0.01 {
            return;
        }
        let amount = tick.health_change.abs().round() as i64;
        if amount <= 0 {
            return;
        }

        let is_damage = tick.health_change <= 0.0;
        let spell_idx = if tick.causing_spell_index <= 0 {
            0
        } else {
            tick.causing_spell_index
        };

        // Check if target is already at max health for overheal classification
        let prev_health = self
            .last_player_health
            .get(&tick.affected_object_id)
            .copied();
        let is_max_health_reached = !is_damage
            && prev_health
                .map(|last| (last - tick.new_health_value).abs() < 1e-6)
                .unwrap_or(false);
        self.last_player_health
            .insert(tick.affected_object_id, tick.new_health_value);

        let causer_in_party = self
            .entities
            .get(&tick.causer_id)
            .map(|e| e.is_party || e.is_self)
            .unwrap_or(false);
        let affected_in_party = self
            .entities
            .get(&tick.affected_object_id)
            .map(|e| e.is_party || e.is_self)
            .unwrap_or(false);

        if !causer_in_party && !affected_in_party {
            return;
        }

        self.combat_start_ms = Some(self.combat_start_ms.map_or(now_ms, |s| s.min(now_ms)));
        self.combat_end_ms = Some(self.combat_end_ms.map_or(now_ms, |e| e.max(now_ms)));

        if causer_in_party && tick.causer_id != 0 {
            let weapon_inferred = self
                .catalog
                .infer_weapon_from_spell(spell_idx)
                .map(|w| w.index);
            let causer = self.ensure_entity(tick.causer_id);
            if causer.equipment.main_hand == 0 {
                if let Some(w_id) = weapon_inferred {
                    causer.equipment.main_hand = w_id;
                }
            }
            causer.first_combat_ms = Some(causer.first_combat_ms.map_or(now_ms, |s| s.min(now_ms)));
            causer.last_combat_ms = Some(causer.last_combat_ms.map_or(now_ms, |e| e.max(now_ms)));

            let spell = causer.spells.entry(spell_idx).or_default();
            spell.ticks += 1;
            if is_damage {
                causer.damage += amount;
                spell.damage += amount;
            } else if is_max_health_reached {
                causer.overhealed += amount;
                spell.overhealed += amount;
            } else {
                causer.heal += amount;
                spell.heal += amount;
            }
        }

        if is_damage && affected_in_party && tick.affected_object_id != 0 {
            let affected = self.ensure_entity(tick.affected_object_id);
            affected.taken_damage += amount;
            affected.first_combat_ms =
                Some(affected.first_combat_ms.map_or(now_ms, |s| s.min(now_ms)));
            affected.last_combat_ms =
                Some(affected.last_combat_ms.map_or(now_ms, |e| e.max(now_ms)));
        }
    }

    fn handle_cluster_change(
        &mut self,
        index: &str,
        world_map_data_type: &str,
        island_name: &str,
        mists_dungeon_tier: i32,
        now_ms: u64,
    ) {
        if self.local_player.in_combat || self.combat_start_ms.is_some() {
            self.maybe_auto_snapshot_combat(now_ms);
            self.local_player.in_combat = false;
        }

        // Finalize any currently active dungeon run
        if let Some(prev) = self.dungeons.first_mut().filter(|d| d.is_active) {
            prev.is_active = false;
            let elapsed_s = ((now_ms.saturating_sub(prev.started_at_ms)) as f64 / 1000.0).max(1.0);
            prev.duration_seconds = elapsed_s;
            prev.fame_per_hour = prev.fame / (elapsed_s / 3600.0);
            self.dirty_history = true;
        }

        let cluster_meta = self.catalog.cluster(index).cloned();
        let name_fr = cluster_meta
            .as_ref()
            .map(|c| c.name_fr.clone())
            .filter(|s| !s.is_empty())
            .or_else(|| (!island_name.is_empty()).then(|| island_name.to_string()))
            .unwrap_or_else(|| format!("Zone {index}"));
        let name_en = cluster_meta
            .as_ref()
            .map(|c| c.name_en.clone())
            .unwrap_or_else(|| name_fr.clone());
        let kind = cluster_meta
            .as_ref()
            .map(|c| c.kind.clone())
            .filter(|s| !s.is_empty())
            .or_else(|| (!world_map_data_type.is_empty()).then(|| world_map_data_type.to_string()))
            .unwrap_or_else(|| "openWorld".into());
        let tier = cluster_meta
            .as_ref()
            .map(|c| c.tier)
            .unwrap_or_else(|| mists_dungeon_tier.clamp(4, 8) as u8);

        self.local_player.current_map_index = index.to_string();
        self.local_player.current_map_name_fr = name_fr.clone();

        let is_instanced = matches!(
            kind.as_str(),
            "mists"
                | "soloDungeon"
                | "groupDungeon"
                | "avalonianDungeon"
                | "hellgate"
                | "corruptedDungeon"
                | "abyssalDepths"
        ) || index.starts_with("MISTS")
            || index.starts_with("RD_")
            || index.starts_with("HELLGATE")
            || index.starts_with("CORRUPTED")
            || index.starts_with("ABYSSAL");

        if is_instanced {
            let id = self.alloc_id();
            self.dungeons.insert(
                0,
                DungeonRun {
                    id,
                    cluster_index: index.to_string(),
                    name_fr,
                    name_en,
                    kind,
                    tier,
                    is_active: true,
                    started_at_ms: now_ms,
                    duration_seconds: 0.0,
                    fame: 0.0,
                    fame_per_hour: 0.0,
                    silver: 0.0,
                    respec: 0.0,
                    might: 0.0,
                    favor: 0.0,
                    chests_opened: 0,
                    caged_wisps_seen: 0,
                    caged_wisps_opened: 0,
                },
            );
            if self.dungeons.len() > MAX_DUNGEON_RUNS {
                self.dungeons.truncate(MAX_DUNGEON_RUNS);
            }
            self.dirty_history = true;
        }
    }

    fn handle_harvest(
        &mut self,
        item_id: i32,
        standard_amount: i32,
        collector_bonus: i32,
        premium_bonus: i32,
        now_ms: u64,
    ) {
        let total = (standard_amount + collector_bonus + premium_bonus).max(0);
        if total == 0 {
            return;
        }
        let item = self.catalog.item(item_id).cloned();
        let unique_name = item
            .as_ref()
            .map(|i| i.unique_name.clone())
            .unwrap_or_else(|| format!("ITEM_{item_id}"));
        let name_fr = item
            .as_ref()
            .map(|i| i.name_fr.clone())
            .unwrap_or_else(|| unique_name.clone());
        let name_en = item
            .as_ref()
            .map(|i| i.name_en.clone())
            .unwrap_or_else(|| unique_name.clone());
        let tier = item.as_ref().map(|i| i.tier).unwrap_or(6);
        let enchantment = item.as_ref().map(|i| i.enchantment).unwrap_or(0);
        let unit_val = item.as_ref().map(|i| i.item_value).unwrap_or(120) * 14;
        let estimated_silver = (total as i64) * unit_val;

        let resource_kind = if unique_name.contains("WOOD") {
            self.gathering_totals.wood += total as i64;
            "wood"
        } else if unique_name.contains("ORE") {
            self.gathering_totals.ore += total as i64;
            "ore"
        } else if unique_name.contains("FIBER") {
            self.gathering_totals.fiber += total as i64;
            "fiber"
        } else if unique_name.contains("HIDE") {
            self.gathering_totals.hide += total as i64;
            "hide"
        } else if unique_name.contains("ROCK") {
            self.gathering_totals.rock += total as i64;
            "rock"
        } else if unique_name.contains("FISH") {
            self.gathering_totals.fish += total as i64;
            "fish"
        } else {
            "other"
        };

        self.gathering_totals.total_nodes += 1;
        self.gathering_totals.total_resources += total as i64;
        self.gathering_totals.total_estimated_silver += estimated_silver;

        let id = self.alloc_id();
        self.gathering_log.insert(
            0,
            GatheringEntry {
                id,
                timestamp_ms: now_ms,
                item_index: item_id,
                unique_name,
                name_fr,
                name_en,
                resource_kind: resource_kind.into(),
                tier,
                enchantment,
                standard_amount,
                collector_bonus,
                premium_bonus,
                total_amount: total,
                estimated_silver,
            },
        );
        if self.gathering_log.len() > MAX_GATHERING_LOG {
            self.gathering_log.truncate(MAX_GATHERING_LOG);
        }
        self.dirty_history = true;
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_loot_grabbed(
        &mut self,
        object_id: i64,
        looted_from_name: String,
        looted_by_name: String,
        is_silver: bool,
        mut item_index: i32,
        mut quantity: i32,
        now_ms: u64,
    ) {
        let mut estimated_silver = 0i64;
        if let Some(disc) = self.discovered_items.get(&object_id) {
            if item_index <= 0 {
                item_index = disc.item_id;
            }
            if quantity <= 0 {
                quantity = disc.quantity;
            }
            estimated_silver = disc.estimated_market_value;
        }

        // 2-second deduplication window for repeated pickup notifications
        let is_dup = self.loot_log.iter().take(16).any(|prev| {
            now_ms.saturating_sub(prev.timestamp_ms) <= LOOT_DEDUP_WINDOW_MS
                && prev.item_index == item_index
                && prev.quantity == quantity
                && prev.looted_by == looted_by_name
                && prev.looted_from == looted_from_name
        });
        if is_dup {
            return;
        }

        let item = self.catalog.item(item_index);
        let unique_name = if is_silver {
            "SILVER".into()
        } else {
            item.map(|i| i.unique_name.clone())
                .unwrap_or_else(|| format!("ITEM_{item_index}"))
        };
        let name_fr = if is_silver {
            "Argent".into()
        } else {
            item.map(|i| i.name_fr.clone())
                .unwrap_or_else(|| unique_name.clone())
        };
        let name_en = if is_silver {
            "Silver".into()
        } else {
            item.map(|i| i.name_en.clone())
                .unwrap_or_else(|| unique_name.clone())
        };
        if estimated_silver <= 0 {
            estimated_silver = if is_silver {
                quantity as i64
            } else {
                item.map(|i| i.item_value * 16).unwrap_or(1_000) * (quantity.max(1) as i64)
            };
        }

        let id = self.alloc_id();
        self.loot_log.insert(
            0,
            LootLogEntry {
                id,
                timestamp_ms: now_ms,
                looted_by: looted_by_name,
                looted_from: looted_from_name,
                item_index,
                unique_name,
                name_fr,
                name_en,
                quantity: quantity.max(1),
                is_silver,
                estimated_silver,
            },
        );
        if self.loot_log.len() > MAX_LOOT_LOG {
            self.loot_log.truncate(MAX_LOOT_LOG);
        }
        self.dirty_history = true;
    }

    fn handle_auction_entries(&mut self, is_requests: bool, raw_json_entries: &[String]) {
        for raw_str in raw_json_entries {
            let Ok(parsed) = serde_json::from_str::<RawAuctionJson>(raw_str) else {
                continue;
            };
            let item_name_fr = self
                .catalog
                .item_by_unique_name(&parsed.item_type_id)
                .map(|i| i.name_fr.clone())
                .unwrap_or_else(|| parsed.item_type_id.clone());

            let unit_silver = FixPoint::from_raw(parsed.unit_price_silver).as_f64();
            let total_silver = if parsed.total_price_silver > 0 {
                FixPoint::from_raw(parsed.total_price_silver).as_f64()
            } else {
                unit_silver * (parsed.amount.max(1) as f64)
            };

            self.market_orders.insert(
                0,
                MarketOrderEntry {
                    id: parsed.id,
                    is_request: is_requests,
                    item_unique_name: parsed.item_type_id,
                    item_name_fr,
                    amount: parsed.amount.max(1),
                    unit_price_silver: unit_silver,
                    total_price_silver: total_silver,
                    quality_level: parsed.quality_level.max(1),
                },
            );
        }
        if self.market_orders.len() > MAX_MARKET_ENTRIES {
            self.market_orders.truncate(MAX_MARKET_ENTRIES);
        }
    }

    fn handle_mail_response(&mut self, mail_id: i64, content: &str) {
        // Albion Market mail format: "UNIT_PRICE|TOTAL_PRICE|AMOUNT|ITEM_UNIQUE_NAME"
        let parts: Vec<&str> = content.split('|').collect();
        if parts.len() < 4 {
            return;
        }
        let unit_raw = parts[0].parse::<i64>().unwrap_or(0);
        let total_raw = parts[1].parse::<i64>().unwrap_or(0);
        let amount = parts[2].parse::<i32>().unwrap_or(1);
        let unique_name = parts[3].trim().to_string();
        let item_name_fr = self
            .catalog
            .item_by_unique_name(&unique_name)
            .map(|i| i.name_fr.clone())
            .unwrap_or_else(|| unique_name.clone());

        self.mail_notifications.insert(
            0,
            MailNotificationEntry {
                mail_id,
                item_unique_name: unique_name,
                item_name_fr,
                amount,
                unit_price_silver: FixPoint::from_raw(unit_raw).as_f64(),
                total_price_silver: FixPoint::from_raw(total_raw).as_f64(),
            },
        );
        if self.mail_notifications.len() > MAX_MARKET_ENTRIES {
            self.mail_notifications.truncate(MAX_MARKET_ENTRIES);
        }
    }

    /// Resets the current combat Damage/Heal/Taken meter while keeping saved snapshots.
    pub fn reset_meter(&mut self) {
        self.combat_start_ms = None;
        self.combat_end_ms = None;
        self.last_auto_snapshot_combat_ms = None;
        self.last_player_health.clear();
        for ent in self.entities.values_mut() {
            ent.damage = 0;
            ent.heal = 0;
            ent.overhealed = 0;
            ent.taken_damage = 0;
            ent.first_combat_ms = None;
            ent.last_combat_ms = None;
            ent.spells.clear();
        }
    }

    /// Resets session counters (Fame, Silver, ReSpec, Might, Favor, Factions, Kills/Deaths).
    pub fn reset_session(&mut self, now_ms: u64) {
        self.session_start_ms = now_ms;
        self.last_now_ms = now_ms;
        self.session = SessionStats {
            elapsed_seconds: 0.0,
            fame: 0.0,
            fame_per_hour: 0.0,
            zone_fame: 0.0,
            satchel_fame: 0.0,
            silver: 0.0,
            silver_per_hour: 0.0,
            guild_tax_paid: 0.0,
            cluster_tax_paid: 0.0,
            respec_points: 0.0,
            respec_per_hour: 0.0,
            respec_paid_silver: 0.0,
            might: 0.0,
            might_per_hour: 0.0,
            favor: 0.0,
            favor_per_hour: 0.0,
            season_points: 0,
            repair_costs: 0.0,
            kills: 0,
            deaths: 0,
            factions: Vec::new(),
        };
        for val in self.faction_map.values_mut() {
            *val = (0.0, 0.0, 0.0);
        }
    }

    /// Automatically archives a snapshot when exiting a meaningful fight (>= 500 output)
    /// if the current combat has not already been snapshotted.
    fn maybe_auto_snapshot_combat(&mut self, now_ms: u64) {
        let Some(start_ms) = self.combat_start_ms else {
            return;
        };
        if self.last_auto_snapshot_combat_ms == Some(start_ms) {
            return;
        }
        let (combat_seconds, total_damage, total_heal, total_taken, players) =
            self.compute_meter_players();
        if players.is_empty() || (total_damage + total_heal + total_taken) < 500 {
            return;
        }
        let auto_label = format!(
            "Auto • {} ({:.0}s)",
            self.local_player.current_map_name_fr, combat_seconds
        );
        let _ = self.take_snapshot(Some(auto_label), now_ms);
    }

    /// Saves the current combat meter state as a named snapshot.
    pub fn take_snapshot(
        &mut self,
        custom_label: Option<String>,
        now_ms: u64,
    ) -> Option<MeterSnapshot> {
        let (combat_seconds, total_damage, total_heal, total_taken, players) =
            self.compute_meter_players();
        if players.is_empty() {
            return None;
        }
        self.last_auto_snapshot_combat_ms = self.combat_start_ms;
        let id = format!("snap-{}", self.alloc_id());
        let label = custom_label
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| {
                format!(
                    "{} ({:.0}s)",
                    self.local_player.current_map_name_fr, combat_seconds
                )
            });
        let snap = MeterSnapshot {
            id,
            label,
            timestamp_ms: now_ms,
            map_name_fr: self.local_player.current_map_name_fr.clone(),
            combat_seconds,
            total_damage,
            total_heal,
            total_taken,
            players,
        };
        self.snapshots.insert(0, snap.clone());
        if self.snapshots.len() > MAX_SNAPSHOTS {
            self.snapshots.truncate(MAX_SNAPSHOTS);
        }
        self.dirty_history = true;
        Some(snap)
    }

    pub fn delete_snapshot(&mut self, id: &str) -> bool {
        let before = self.snapshots.len();
        self.snapshots.retain(|s| s.id != id);
        let deleted = self.snapshots.len() != before;
        if deleted {
            self.dirty_history = true;
        }
        deleted
    }

    /// Exports persisted history (snapshots, dungeons, gathering, loot) for disk storage.
    pub fn export_history(&self) -> PersistedHistory {
        PersistedHistory {
            snapshots: self.snapshots.clone(),
            dungeons: self.dungeons.clone(),
            gathering_totals: self.gathering_totals.clone(),
            gathering_log: self.gathering_log.clone(),
            loot_log: self.loot_log.clone(),
        }
    }

    /// Restores persisted history loaded from disk at startup.
    pub fn restore_history(&mut self, mut history: PersistedHistory) {
        history.snapshots.truncate(MAX_SNAPSHOTS);
        history.dungeons.truncate(MAX_DUNGEON_RUNS);
        history.gathering_log.truncate(MAX_GATHERING_LOG);
        history.loot_log.truncate(MAX_LOOT_LOG);
        for d in &mut history.dungeons {
            d.is_active = false;
        }
        self.snapshots = history.snapshots;
        self.dungeons = history.dungeons;
        self.gathering_totals = history.gathering_totals;
        self.gathering_log = history.gathering_log;
        self.loot_log = history.loot_log;
    }

    /// Exports the loot log as a RFC-4180 CSV string.
    pub fn export_loot_csv(&self) -> String {
        let mut out = String::from(
            "timestamp_ms,looted_by,looted_from,unique_name,name_fr,quantity,estimated_silver\n",
        );
        for row in &self.loot_log {
            out.push_str(&format!(
                "{},\"{}\",\"{}\",\"{}\",\"{}\",{},{}\n",
                row.timestamp_ms,
                row.looted_by.replace('"', "\"\""),
                row.looted_from.replace('"', "\"\""),
                row.unique_name,
                row.name_fr.replace('"', "\"\""),
                row.quantity,
                row.estimated_silver
            ));
        }
        out
    }

    fn compute_meter_players(&self) -> (f64, i64, i64, i64, Vec<DamageMeterPlayer>) {
        let global_combat_secs = match (self.combat_start_ms, self.combat_end_ms) {
            (Some(s), Some(e)) => ((e.saturating_sub(s) as f64) / 1000.0).max(1.0),
            _ => 0.0,
        };

        let mut total_damage = 0i64;
        let mut total_heal = 0i64;
        let mut total_taken = 0i64;

        for ent in self.entities.values() {
            if !ent.is_party && !ent.is_self {
                continue;
            }
            total_damage += ent.damage;
            total_heal += ent.heal;
            total_taken += ent.taken_damage;
        }

        let mut players = Vec::new();
        for ent in self.entities.values() {
            if (!ent.is_party && !ent.is_self)
                || (ent.damage == 0
                    && ent.heal == 0
                    && ent.overhealed == 0
                    && ent.taken_damage == 0)
            {
                continue;
            }

            let player_secs = match (ent.first_combat_ms, ent.last_combat_ms) {
                (Some(s), Some(e)) if e > s => ((e - s) as f64 / 1000.0).max(1.0),
                _ => global_combat_secs.max(1.0),
            };

            let weapon_item = self.catalog.item(ent.equipment.main_hand);
            let weapon_unique_name = weapon_item
                .map(|i| i.unique_name.clone())
                .unwrap_or_else(|| "UNARMED".into());
            let weapon_name_fr = weapon_item
                .map(|i| i.name_fr.clone())
                .unwrap_or_else(|| "Arme non inspectée".into());
            let weapon_name_en = weapon_item
                .map(|i| i.name_en.clone())
                .unwrap_or_else(|| "Uninspected Weapon".into());

            let dps = (ent.damage as f64) / player_secs;
            let hps = (ent.heal as f64) / player_secs;
            let total_raw_healing = ent.heal + ent.overhealed;
            let overheal_percent = if total_raw_healing > 0 {
                (100.0 / (total_raw_healing as f64)) * (ent.overhealed as f64)
            } else {
                0.0
            };
            let damage_percentage = if total_damage > 0 {
                (100.0 / (total_damage as f64)) * (ent.damage as f64)
            } else {
                0.0
            };
            let heal_percentage = if total_heal > 0 {
                (100.0 / (total_heal as f64)) * (ent.heal as f64)
            } else {
                0.0
            };
            let taken_damage_percentage = if total_taken > 0 {
                (100.0 / (total_taken as f64)) * (ent.taken_damage as f64)
            } else {
                0.0
            };

            let player_total_output = (ent.damage + ent.heal).max(1) as f64;
            let mut spells: Vec<SpellStat> = ent
                .spells
                .iter()
                .map(|(&sp_idx, acc)| {
                    let meta = self.catalog.spell(sp_idx);
                    let unique_name = meta
                        .map(|s| s.unique_name.clone())
                        .unwrap_or_else(|| format!("SPELL_{sp_idx}"));
                    let name_fr = meta
                        .map(|s| s.name_fr.clone())
                        .unwrap_or_else(|| format!("Sort #{sp_idx}"));
                    let name_en = meta
                        .map(|s| s.name_en.clone())
                        .unwrap_or_else(|| format!("Spell #{sp_idx}"));
                    let category = meta
                        .map(|s| s.category.clone())
                        .unwrap_or_else(|| "damage".into());
                    let combined = (acc.damage + acc.heal) as f64;
                    SpellStat {
                        spell_index: sp_idx,
                        unique_name,
                        name_fr,
                        name_en,
                        category,
                        ticks: acc.ticks,
                        damage: acc.damage,
                        heal: acc.heal,
                        overhealed: acc.overhealed,
                        share_percent: (combined / player_total_output) * 100.0,
                        dps: (acc.damage as f64) / player_secs,
                        hps: (acc.heal as f64) / player_secs,
                    }
                })
                .collect();
            spells.sort_by_key(|a| std::cmp::Reverse(a.damage + a.heal));

            players.push(DamageMeterPlayer {
                object_id: ent.object_id,
                guid: ent.guid.to_uuid_string(),
                name: ent.name.clone(),
                guild_name: ent.guild_name.clone(),
                is_self: ent.is_self,
                weapon_unique_name,
                weapon_name_fr,
                weapon_name_en,
                damage: ent.damage,
                heal: ent.heal,
                overhealed: ent.overhealed,
                taken_damage: ent.taken_damage,
                combat_seconds: player_secs,
                dps,
                hps,
                overheal_percent,
                damage_percentage,
                heal_percentage,
                taken_damage_percentage,
                spells,
            });
        }

        players.sort_by(|a, b| b.damage.cmp(&a.damage).then_with(|| b.heal.cmp(&a.heal)));
        (
            global_combat_secs,
            total_damage,
            total_heal,
            total_taken,
            players,
        )
    }

    /// Produces a complete, immutable [`AppSnapshot`] for the frontend.
    pub fn snapshot(&self, now_ms: u64) -> AppSnapshot {
        let current_ms = self.last_now_ms.max(now_ms);
        let elapsed_s =
            ((current_ms.saturating_sub(self.session_start_ms)) as f64 / 1000.0).max(1.0);
        let hours = elapsed_s / 3600.0;

        let (
            combat_seconds,
            total_party_damage,
            total_party_heal,
            total_party_taken,
            meter_players,
        ) = self.compute_meter_players();

        let factions: Vec<FactionCitySummary> = self
            .faction_map
            .iter()
            .map(|(city, &(coins, rank_pts, flag_pts))| FactionCitySummary {
                city: city.label_fr().to_string(),
                coins,
                rank_points: rank_pts,
                flag_points: flag_pts,
            })
            .collect();

        let session = SessionStats {
            elapsed_seconds: elapsed_s,
            fame: self.session.fame,
            fame_per_hour: self.session.fame / hours,
            zone_fame: self.session.zone_fame,
            satchel_fame: self.session.satchel_fame,
            silver: self.session.silver,
            silver_per_hour: self.session.silver / hours,
            guild_tax_paid: self.session.guild_tax_paid,
            cluster_tax_paid: self.session.cluster_tax_paid,
            respec_points: self.session.respec_points,
            respec_per_hour: self.session.respec_points / hours,
            respec_paid_silver: self.session.respec_paid_silver,
            might: self.session.might,
            might_per_hour: self.session.might / hours,
            favor: self.session.favor,
            favor_per_hour: self.session.favor / hours,
            season_points: self.session.season_points,
            repair_costs: self.session.repair_costs,
            kills: self.session.kills,
            deaths: self.session.deaths,
            factions,
        };

        let mut dungeons = self.dungeons.clone();
        if let Some(active) = dungeons.first_mut().filter(|d| d.is_active) {
            let d_secs =
                ((current_ms.saturating_sub(active.started_at_ms)) as f64 / 1000.0).max(1.0);
            active.duration_seconds = d_secs;
            active.fame_per_hour = active.fame / (d_secs / 3600.0);
        }

        let mut inspected_players = Vec::new();
        for ent in self.entities.values() {
            let slot_defs = [
                ("mainHand", ent.equipment.main_hand),
                ("offHand", ent.equipment.off_hand),
                ("head", ent.equipment.head),
                ("chest", ent.equipment.chest),
                ("shoes", ent.equipment.shoes),
                ("bag", ent.equipment.bag),
                ("cape", ent.equipment.cape),
                ("mount", ent.equipment.mount),
                ("potion", ent.equipment.potion),
                ("food", ent.equipment.buff_food),
            ];
            let mut slots = Vec::new();
            let mut estimated_loadout_value = 0i64;
            for (slot_name, item_idx) in slot_defs {
                if item_idx <= 0 {
                    continue;
                }
                if let Some(meta) = self.catalog.item(item_idx) {
                    let est = meta.item_value * 18;
                    estimated_loadout_value += est;
                    slots.push(EquippedSlotView {
                        slot: slot_name.into(),
                        item_index: item_idx,
                        unique_name: meta.unique_name.clone(),
                        name_fr: meta.name_fr.clone(),
                        name_en: meta.name_en.clone(),
                        tier: meta.tier,
                        enchantment: meta.enchantment,
                        item_value: est,
                    });
                }
            }
            inspected_players.push(InspectedPlayer {
                object_id: ent.object_id,
                guid: ent.guid.to_uuid_string(),
                name: ent.name.clone(),
                guild_name: ent.guild_name.clone(),
                is_party_member: ent.is_party,
                is_self: ent.is_self,
                estimated_loadout_value,
                slots,
            });
        }

        AppSnapshot {
            catalog_version: self.catalog.version.clone(),
            catalog_item_count: self.catalog.item_count(),
            catalog_spell_count: self.catalog.spell_count(),
            local_player: self.local_player.clone(),
            combat_seconds,
            total_party_damage,
            total_party_heal,
            total_party_taken,
            meter_players,
            snapshots: self.snapshots.clone(),
            session,
            dungeons,
            gathering_totals: self.gathering_totals.clone(),
            gathering_log: self.gathering_log.clone(),
            loot_log: self.loot_log.clone(),
            inspected_players,
            market_orders: self.market_orders.clone(),
            mail_notifications: self.mail_notifications.clone(),
            trades: self.trades.clone(),
            network: NetworkDiagnostics {
                packets_processed: self.photon.packets_processed,
                commands_processed: self.photon.commands_processed,
                telemetry_events_processed: self.telemetry_events_processed,
                crc_mismatches: self.photon.crc_mismatches,
                pending_fragments: self.photon.pending_fragment_count(),
            },
        }
    }

    /// Synthesizes a realistic sequence of wire-level Photon UDP datagrams (including CRC32 `0xCC`
    /// verification and multi-fragment `SendFragment` packets) and feeds them through [`Self::ingest_udp_payload`].
    pub fn run_wire_simulation_scenario(&mut self, base_ms: u64) -> usize {
        let g_self = Guid16([0x10; 16]);
        let g_healer = Guid16([0x20; 16]);
        let g_bow = Guid16([0x30; 16]);
        let g_tank = Guid16([0x40; 16]);
        let g_curse = Guid16([0x50; 16]);

        // 1. Fragmented JoinResponse packet
        let join_msg = encode_telemetry_event_to_photon(&AlbionTelemetryEvent::JoinResponse {
            user_object_id: 101,
            user_guid: g_self,
            username: "Aurelia".into(),
            map_index: "3005".into(),
            main_map_index: "3005".into(),
            guild_name: "Aegis".into(),
            alliance_name: "VANGUARD".into(),
            current_focus_points: 26_450.0,
            max_focus_points: 30_000.0,
            silver: FixPoint::from_f64(28_450_000.0),
            gold: FixPoint::from_f64(4_500.0),
            learning_points: FixPoint::from_f64(1_120.0),
            reputation: 15_400.0,
            is_respec_active: true,
        });
        let join_frags = build_photon_fragmented_packets(&join_msg, 64, 1, true);
        let mut total_events = 0usize;
        for frag in &join_frags {
            total_events += self.ingest_udp_payload(frag, base_ms);
        }

        // 2. Party + Character discovery + Dungeon entry
        let setup_events = [
            AlbionTelemetryEvent::PartyJoined {
                party_object_id: 900,
                members: vec![
                    (g_self, "Aurelia".into()),
                    (g_healer, "Seraphina".into()),
                    (g_bow, "Kaelen".into()),
                    (g_tank, "Thorne".into()),
                    (g_curse, "Vesper".into()),
                ],
            },
            AlbionTelemetryEvent::NewCharacter {
                object_id: 101,
                name: "Aurelia".into(),
                guid: g_self,
                guild_name: "Aegis".into(),
                equipment: CharacterEquipment {
                    main_hand: 1031, // Blazing Staff T8.2
                    off_hand: 0,
                    head: 1202,
                    chest: 1302,
                    shoes: 1402,
                    bag: 1501,
                    cape: 1600,
                    mount: 1700,
                    potion: 1801,
                    buff_food: 1901,
                },
            },
            AlbionTelemetryEvent::NewCharacter {
                object_id: 102,
                name: "Seraphina".into(),
                guid: g_healer,
                guild_name: "Aegis".into(),
                equipment: CharacterEquipment {
                    main_hand: 1061, // Hallowfall T8.2
                    off_hand: 1101,  // Mistcaller T8.2
                    head: 1202,
                    chest: 1302,
                    shoes: 1402,
                    bag: 1500,
                    cape: 1602,
                    mount: 1700,
                    potion: 1800,
                    buff_food: 1901,
                },
            },
            AlbionTelemetryEvent::NewCharacter {
                object_id: 103,
                name: "Kaelen".into(),
                guid: g_bow,
                guild_name: "Aegis".into(),
                equipment: CharacterEquipment {
                    main_hand: 1022, // Mistpiercer T8.2
                    off_hand: 0,
                    head: 1201,
                    chest: 1301,
                    shoes: 1401,
                    bag: 1500,
                    cape: 1600,
                    mount: 1701,
                    potion: 1801,
                    buff_food: 1900,
                },
            },
            AlbionTelemetryEvent::NewCharacter {
                object_id: 104,
                name: "Thorne".into(),
                guid: g_tank,
                guild_name: "Aegis".into(),
                equipment: CharacterEquipment {
                    main_hand: 1090, // Heavy Mace T8.1
                    off_hand: 0,
                    head: 1200,
                    chest: 1300,
                    shoes: 1400,
                    bag: 1500,
                    cape: 1601,
                    mount: 1700,
                    potion: 1800,
                    buff_food: 1900,
                },
            },
            AlbionTelemetryEvent::NewCharacter {
                object_id: 105,
                name: "Vesper".into(),
                guid: g_curse,
                guild_name: "Aegis".into(),
                equipment: CharacterEquipment {
                    main_hand: 0, // Will be inferred from Shadowcaller spell 2604!
                    off_hand: 1102,
                    head: 1201,
                    chest: 1301,
                    shoes: 1402,
                    bag: 1500,
                    cape: 1600,
                    mount: 1700,
                    potion: 1801,
                    buff_food: 1900,
                },
            },
            AlbionTelemetryEvent::ChangeClusterResponse {
                index: "MISTS_DUO_T8".into(),
                world_map_data_type: "mists".into(),
                island_name: String::new(),
                mists_dungeon_tier: 8,
            },
        ];
        let setup_msgs: Vec<_> = setup_events
            .iter()
            .map(encode_telemetry_event_to_photon)
            .collect();
        let pkt = build_photon_udp_packet(&setup_msgs, true, false, 20);
        total_events += self.ingest_udp_payload(&pkt, base_ms + 1_000);

        // 3. Combat rotation across 18 seconds
        let combat_steps: Vec<(u64, AlbionTelemetryEvent)> = vec![
            (
                base_ms + 2_000,
                AlbionTelemetryEvent::InCombatStateUpdate {
                    object_id: 101,
                    in_active_combat: true,
                    in_passive_combat: true,
                },
            ),
            (
                base_ms + 2_200,
                AlbionTelemetryEvent::ActiveSpellEffectsUpdate {
                    causer_id: 105,
                    spell_indices: vec![2604], // Infers Shadowcaller T8.2 on Vesper
                },
            ),
            (
                base_ms + 3_000,
                AlbionTelemetryEvent::HealthUpdates {
                    ticks: vec![
                        HealthTick {
                            affected_object_id: 5001,
                            timestamp: 1,
                            health_change: -1_840.0,
                            new_health_value: 8_160.0,
                            effect_type: EffectType::Magic,
                            effect_origin: EffectOrigin::Direct,
                            causer_id: 101,
                            causing_spell_index: 2404, // Flame Tornado
                        },
                        HealthTick {
                            affected_object_id: 5001,
                            timestamp: 2,
                            health_change: -920.0,
                            new_health_value: 7_240.0,
                            effect_type: EffectType::Magic,
                            effect_origin: EffectOrigin::OverTime,
                            causer_id: 101,
                            causing_spell_index: 2401, // Fire Bolt
                        },
                        HealthTick {
                            affected_object_id: 5001,
                            timestamp: 3,
                            health_change: -2_150.0,
                            new_health_value: 5_090.0,
                            effect_type: EffectType::Physical,
                            effect_origin: EffectOrigin::Direct,
                            causer_id: 103,
                            causing_spell_index: 2305, // Mistpiercer beam
                        },
                    ],
                },
            ),
            (
                base_ms + 6_500,
                AlbionTelemetryEvent::HealthUpdates {
                    ticks: vec![
                        HealthTick {
                            affected_object_id: 104,
                            timestamp: 4,
                            health_change: -1_420.0,
                            new_health_value: 2_580.0,
                            effect_type: EffectType::Physical,
                            effect_origin: EffectOrigin::Direct,
                            causer_id: 5001,
                            causing_spell_index: 0,
                        },
                        HealthTick {
                            affected_object_id: 104,
                            timestamp: 5,
                            health_change: 980.0,
                            new_health_value: 3_560.0,
                            effect_type: EffectType::Magic,
                            effect_origin: EffectOrigin::Direct,
                            causer_id: 102,
                            causing_spell_index: 2704, // Divine Intervention
                        },
                        HealthTick {
                            affected_object_id: 104,
                            timestamp: 6,
                            health_change: 440.0,
                            new_health_value: 4_000.0,
                            effect_type: EffectType::Magic,
                            effect_origin: EffectOrigin::Direct,
                            causer_id: 102,
                            causing_spell_index: 2701, // Generous Heal
                        },
                        // Next heal on 104 keeps health at 4_000.0 => Overheal!
                        HealthTick {
                            affected_object_id: 104,
                            timestamp: 7,
                            health_change: 520.0,
                            new_health_value: 4_000.0,
                            effect_type: EffectType::Magic,
                            effect_origin: EffectOrigin::Direct,
                            causer_id: 102,
                            causing_spell_index: 2702, // Holy Blessing
                        },
                    ],
                },
            ),
            (
                base_ms + 11_000,
                AlbionTelemetryEvent::HealthUpdates {
                    ticks: vec![
                        HealthTick {
                            affected_object_id: 5001,
                            timestamp: 8,
                            health_change: -1_680.0,
                            new_health_value: 3_410.0,
                            effect_type: EffectType::Magic,
                            effect_origin: EffectOrigin::OverTime,
                            causer_id: 105,
                            causing_spell_index: 2604, // Inner Shadow
                        },
                        HealthTick {
                            affected_object_id: 5001,
                            timestamp: 9,
                            health_change: -780.0,
                            new_health_value: 2_630.0,
                            effect_type: EffectType::Physical,
                            effect_origin: EffectOrigin::Direct,
                            causer_id: 104,
                            causing_spell_index: 3001, // Defensive Slam
                        },
                    ],
                },
            ),
            (
                base_ms + 18_000,
                AlbionTelemetryEvent::HealthUpdate(HealthTick {
                    affected_object_id: 5001,
                    timestamp: 10,
                    health_change: -2_630.0,
                    new_health_value: 0.0,
                    effect_type: EffectType::Magic,
                    effect_origin: EffectOrigin::Direct,
                    causer_id: 101,
                    causing_spell_index: 2403, // Pyroblast
                }),
            ),
            (
                base_ms + 18_100,
                AlbionTelemetryEvent::HealthUpdate(HealthTick {
                    affected_object_id: 101,
                    timestamp: 11,
                    health_change: -640.0,
                    new_health_value: 2_210.0,
                    effect_type: EffectType::Magic,
                    effect_origin: EffectOrigin::Direct,
                    causer_id: 5001,
                    causing_spell_index: 0,
                }),
            ),
            // Session rewards, chests, gathering, loot, market & mail
            (
                base_ms + 18_500,
                AlbionTelemetryEvent::Died {
                    died_player: "Malakor".into(),
                    died_guild: "Crimson".into(),
                    killed_by: "Aurelia".into(),
                    killed_by_guild: "Aegis".into(),
                },
            ),
            (
                base_ms + 19_000,
                AlbionTelemetryEvent::UpdateFame {
                    object_id: 101,
                    total_player_fame: FixPoint::from_f64(82_450_000.0),
                    fame_with_zone_multiplier: FixPoint::from_f64(64_800.0),
                    zone_fame: FixPoint::from_f64(42_000.0),
                    is_premium_bonus: true,
                    satchel_fame: FixPoint::from_f64(14_200.0),
                    bonus_factor: 1.55,
                },
            ),
            (
                base_ms + 19_200,
                AlbionTelemetryEvent::TakeSilver {
                    object_id: 101,
                    target_entity_id: 5001,
                    yield_pre_tax: FixPoint::from_f64(48_500.0),
                    guild_tax: FixPoint::from_f64(2_425.0),
                    cluster_tax: FixPoint::from_f64(1_200.0),
                    is_premium_bonus: true,
                },
            ),
            (
                base_ms + 19_400,
                AlbionTelemetryEvent::UpdateReSpecPoints {
                    current_total_respec_points: FixPoint::from_f64(1_240_000.0),
                    gained_respec_points: FixPoint::from_f64(31_400.0),
                    paid_silver: FixPoint::from_f64(28_260.0),
                },
            ),
            (
                base_ms + 19_600,
                AlbionTelemetryEvent::MightAndFavorReceived {
                    might: FixPoint::from_f64(1_840.0),
                    bonus_might: FixPoint::from_f64(420.0),
                    premium_might: FixPoint::from_f64(610.0),
                    favor: FixPoint::from_f64(920.0),
                    bonus_favor: FixPoint::from_f64(210.0),
                    premium_favor: FixPoint::from_f64(305.0),
                    total_favor: FixPoint::from_f64(14_850.0),
                },
            ),
            (
                base_ms + 19_800,
                AlbionTelemetryEvent::UpdateCurrency {
                    city_faction: CityFaction::Thetford,
                    gained_faction_coins: FixPoint::from_f64(2_450.0),
                    faction_rank_points: FixPoint::from_f64(4_900.0),
                    premium_bonus: FixPoint::from_f64(815.0),
                    total_player_faction_points: FixPoint::from_f64(98_400.0),
                },
            ),
            (
                base_ms + 20_000,
                AlbionTelemetryEvent::LootChestOpened { object_id: 7001 },
            ),
            (
                base_ms + 20_100,
                AlbionTelemetryEvent::MistsCagedWisp {
                    object_id: 7002,
                    opened: true,
                },
            ),
            (
                base_ms + 20_300,
                AlbionTelemetryEvent::NewDiscoveredItem {
                    object_id: 8001,
                    item_id: 1004, // Carving Sword T7.1
                    quantity: 1,
                    estimated_market_value: 445_000,
                    quality_level: 3,
                },
            ),
            (
                base_ms + 20_400,
                AlbionTelemetryEvent::OtherGrabbedLoot {
                    object_id: 8001,
                    looted_from_name: "Coffre des Brumes".into(),
                    looted_by_name: "Aurelia".into(),
                    is_silver: false,
                    item_index: 1004,
                    quantity: 1,
                },
            ),
            (
                base_ms + 20_600,
                AlbionTelemetryEvent::HarvestFinished {
                    user_object_id: 101,
                    harvest_object_id: 9001,
                    item_id: 4011, // Meteorite Ore T7.2
                    standard_amount: 4,
                    collector_bonus_amount: 2,
                    premium_bonus_amount: 2,
                },
            ),
            (
                base_ms + 20_800,
                AlbionTelemetryEvent::HarvestFinished {
                    user_object_id: 101,
                    harvest_object_id: 9002,
                    item_id: 4021, // Ghost Hemp T8.1
                    standard_amount: 3,
                    collector_bonus_amount: 1,
                    premium_bonus_amount: 2,
                },
            ),
            (
                base_ms + 21_000,
                AlbionTelemetryEvent::AuctionEntriesResponse {
                    is_requests: false,
                    raw_json_entries: vec![
                        r#"{"Id":901,"UnitPriceSilver":4450000000,"TotalPriceSilver":4450000000,"Amount":1,"ItemTypeId":"T7_2H_CLEAVER_HELL@1","QualityLevel":3}"#.into(),
                        r#"{"Id":902,"UnitPriceSilver":14800000000,"TotalPriceSilver":14800000000,"Amount":1,"ItemTypeId":"T8_MAIN_HOLYSTAFF_AVALON@2","QualityLevel":4}"#.into(),
                    ],
                },
            ),
            (
                base_ms + 21_200,
                AlbionTelemetryEvent::ReadMailResponse {
                    mail_id: 501,
                    content: "4450000000|4450000000|1|T7_2H_CLEAVER_HELL@1".into(),
                },
            ),
        ];

        for (seq_idx, (ts, ev)) in combat_steps.into_iter().enumerate() {
            let msg = encode_telemetry_event_to_photon(&ev);
            let pkt = build_photon_udp_packet(&[msg], true, seq_idx % 2 == 1, 100 + seq_idx as i32);
            total_events += self.ingest_udp_payload(&pkt, ts);
        }

        total_events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMBEDDED: &str = include_str!("../../../data/catalog.json");

    #[test]
    fn wire_simulation_computes_damage_heal_overheal_fame_and_loot() {
        let catalog = Catalog::from_json_str(EMBEDDED).unwrap();
        let mut engine = AnalyzerEngine::new(catalog, 1_000_000);

        let processed = engine.run_wire_simulation_scenario(1_000_000);
        assert!(processed >= 18);

        let snap = engine.snapshot(1_025_000);
        assert_eq!(snap.local_player.username, "Aurelia");
        assert_eq!(snap.meter_players.len(), 5);

        // Aurelia top damage: 1840 + 920 + 2630 = 5390
        let aurelia = snap
            .meter_players
            .iter()
            .find(|p| p.name == "Aurelia")
            .unwrap();
        assert_eq!(aurelia.damage, 5_390);
        assert_eq!(aurelia.taken_damage, 640);
        assert!(aurelia.dps > 300.0);

        // Seraphina healing: 980 + 440 = 1420 heal, 520 overheal => 520 / 1940 = 26.804%
        let seraphina = snap
            .meter_players
            .iter()
            .find(|p| p.name == "Seraphina")
            .unwrap();
        assert_eq!(seraphina.heal, 1_420);
        assert_eq!(seraphina.overhealed, 520);
        assert!((seraphina.overheal_percent - 26.8041).abs() < 0.01);

        // Vesper weapon inferred from spell 2604 (Shadowcaller T8.2)
        let vesper = snap
            .meter_players
            .iter()
            .find(|p| p.name == "Vesper")
            .unwrap();
        assert_eq!(vesper.weapon_unique_name, "T8_MAIN_CURSEDSTAFF_AVALON@2");

        // Session stats
        assert_eq!(snap.session.fame, 64_800.0);
        assert_eq!(snap.session.kills, 1);
        assert_eq!(snap.dungeons.len(), 1);
        assert_eq!(snap.dungeons[0].chests_opened, 1);
        assert_eq!(snap.dungeons[0].caged_wisps_opened, 1);
        assert_eq!(snap.gathering_totals.total_nodes, 2);
        assert_eq!(snap.gathering_totals.ore, 8);
        assert_eq!(snap.gathering_totals.fiber, 6);
        assert_eq!(snap.loot_log.len(), 1);
        assert_eq!(snap.market_orders.len(), 2);
        assert_eq!(snap.mail_notifications.len(), 1);
    }

    #[test]
    fn deduplicates_identical_loot_events_within_two_seconds() {
        let catalog = Catalog::from_json_str(EMBEDDED).unwrap();
        let mut engine = AnalyzerEngine::new(catalog, 10_000);

        let ev = AlbionTelemetryEvent::OtherGrabbedLoot {
            object_id: 50,
            looted_from_name: "Boss".into(),
            looted_by_name: "Aurelia".into(),
            is_silver: false,
            item_index: 1003,
            quantity: 1,
        };
        engine.ingest_telemetry_event(ev.clone(), 10_500);
        engine.ingest_telemetry_event(ev.clone(), 11_800); // +1.3s -> ignored as duplicate
        engine.ingest_telemetry_event(ev, 13_000); // +2.5s -> recorded

        let snap = engine.snapshot(14_000);
        assert_eq!(snap.loot_log.len(), 2);
        let csv = engine.export_loot_csv();
        assert!(csv.contains("T8_2H_DUALSWORD@2"));
    }

    #[test]
    fn exports_and_restores_persisted_session_history() {
        let catalog = Catalog::from_json_str(EMBEDDED).unwrap();
        let mut engine = AnalyzerEngine::new(catalog.clone(), 1_000);
        engine.run_wire_simulation_scenario(1_000);
        let saved_snap = engine.take_snapshot(Some("Boss Brumes".into()), 20_000);
        assert!(saved_snap.is_some());

        let exported = engine.export_history();
        assert_eq!(exported.snapshots.len(), 1);
        assert_eq!(exported.dungeons.len(), 1);
        assert_eq!(exported.loot_log.len(), 1);

        let mut fresh = AnalyzerEngine::new(catalog, 50_000);
        fresh.restore_history(exported);
        let restored = fresh.snapshot(50_000);
        assert_eq!(restored.snapshots.len(), 1);
        assert_eq!(restored.snapshots[0].label, "Boss Brumes");
        assert_eq!(restored.dungeons.len(), 1);
        assert!(!restored.dungeons[0].is_active);
        assert_eq!(restored.loot_log.len(), 1);
    }

    #[test]
    fn auto_snapshots_combat_on_exit_without_duplicates() {
        let catalog = Catalog::from_json_str(EMBEDDED).unwrap();
        let mut engine = AnalyzerEngine::new(catalog, 1_000);
        engine.run_wire_simulation_scenario(1_000);
        assert!(engine.take_dirty_history());
        assert!(!engine.take_dirty_history());

        // Leaving combat should archive an Auto snapshot once
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::InCombatStateUpdate {
                object_id: 101,
                in_active_combat: false,
                in_passive_combat: false,
            },
            25_000,
        );
        assert!(engine.take_dirty_history());
        let snap = engine.snapshot(25_500);
        assert_eq!(snap.snapshots.len(), 1);
        assert!(snap.snapshots[0].label.starts_with("Auto • "));
    }
}
