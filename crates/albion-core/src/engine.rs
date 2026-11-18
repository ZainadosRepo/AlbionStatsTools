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
use std::collections::{BTreeMap, HashMap, HashSet};

const MAX_LOOT_LOG: usize = 250;
const MAX_GATHERING_LOG: usize = 200;
const MAX_DUNGEON_RUNS: usize = 60;
const MAX_MARKET_ENTRIES: usize = 150;
const MAX_SNAPSHOTS: usize = 30;
const MAX_ARCHIVED_SESSIONS: usize = 30;
const MAX_SESSION_ZONES: usize = 100;
const LOOT_DEDUP_WINDOW_MS: u64 = 2_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpellStat {
    pub spell_index: i32,
    pub unique_name: String,
    pub name_fr: String,
    pub name_en: String,
    pub category: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_sprite: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionZoneSummary {
    pub cluster_index: String,
    pub name_fr: String,
    pub name_en: String,
    pub kind: String,
    pub tier: u8,
    pub entered_at_ms: u64,
    pub duration_seconds: f64,
    pub fame: f64,
    pub silver: f64,
    pub gathering_nodes: u32,
    pub gathering_resources: i64,
    pub kills: u32,
    pub deaths: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchivedSession {
    pub id: String,
    pub label: String,
    pub started_at_ms: u64,
    pub ended_at_ms: u64,
    pub duration_seconds: f64,
    pub map_name_fr: String,
    pub fame: f64,
    pub fame_per_hour: f64,
    pub silver: f64,
    pub silver_per_hour: f64,
    pub respec_points: f64,
    pub might: f64,
    pub favor: f64,
    pub kills: u32,
    pub deaths: u32,
    #[serde(default)]
    pub zone_fame: f64,
    #[serde(default)]
    pub satchel_fame: f64,
    #[serde(default)]
    pub respec_paid_silver: f64,
    #[serde(default)]
    pub guild_tax_paid: f64,
    #[serde(default)]
    pub cluster_tax_paid: f64,
    #[serde(default)]
    pub repair_costs: f64,
    #[serde(default)]
    pub season_points: i32,
    #[serde(default)]
    pub factions: Vec<FactionCitySummary>,
    #[serde(default)]
    pub zones: Vec<SessionZoneSummary>,
    #[serde(default)]
    pub combat_players: Vec<DamageMeterPlayer>,
    #[serde(default)]
    pub dungeons: Vec<DungeonRun>,
    #[serde(default)]
    pub gathering_totals: GatheringTotals,
    #[serde(default)]
    pub gathering_log: Vec<GatheringEntry>,
    #[serde(default)]
    pub loot_log: Vec<LootLogEntry>,
    #[serde(default)]
    pub trades: Vec<TradeLogEntry>,
}

/// Complete immutable snapshot returned to the UI on poll or command completion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSnapshot {
    pub catalog_version: String,
    pub catalog_item_count: usize,
    pub catalog_spell_count: usize,
    pub catalog_recipe_count: usize,
    #[serde(default)]
    pub catalog_cluster_count: usize,
    #[serde(default)]
    pub catalog_weapon_spell_count: usize,
    pub local_player: LocalPlayerProfile,
    pub combat_seconds: f64,
    pub total_party_damage: i64,
    pub total_party_heal: i64,
    pub total_party_taken: i64,
    pub meter_players: Vec<DamageMeterPlayer>,
    pub snapshots: Vec<MeterSnapshot>,
    pub session: SessionStats,
    pub archived_sessions: Vec<ArchivedSession>,
    pub dungeons: Vec<DungeonRun>,
    pub gathering_totals: GatheringTotals,
    pub gathering_log: Vec<GatheringEntry>,
    pub loot_log: Vec<LootLogEntry>,
    pub inspected_players: Vec<InspectedPlayer>,
    pub market_orders: Vec<MarketOrderEntry>,
    pub mail_notifications: Vec<MailNotificationEntry>,
    pub trades: Vec<TradeLogEntry>,
    #[serde(default)]
    pub live_session_zones: Vec<SessionZoneSummary>,
    #[serde(default)]
    pub live_session_combat_players: Vec<DamageMeterPlayer>,
    #[serde(default)]
    pub live_session_dungeons: Vec<DungeonRun>,
    #[serde(default)]
    pub live_session_gathering_totals: GatheringTotals,
    #[serde(default)]
    pub live_session_gathering_log: Vec<GatheringEntry>,
    #[serde(default)]
    pub live_session_loot_log: Vec<LootLogEntry>,
    #[serde(default)]
    pub live_session_trades: Vec<TradeLogEntry>,
    pub network: NetworkDiagnostics,
}

/// Subset of session history persisted to disk across application restarts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedHistory {
    #[serde(default)]
    pub snapshots: Vec<MeterSnapshot>,
    #[serde(default)]
    pub archived_sessions: Vec<ArchivedSession>,
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
    weapon_specificity: u8,
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
    local_player_guid: Guid16,
    awaiting_cluster_respawn: bool,
    entities: BTreeMap<i64, InternalEntity>,
    guid_to_object_id: HashMap<Guid16, i64>,
    party_guids: HashMap<Guid16, String>,
    pending_equipment: HashMap<i64, (CharacterEquipment, Vec<i32>, u64)>,
    last_known_equipment: HashMap<Guid16, (CharacterEquipment, u8)>,
    last_player_health: HashMap<i64, f64>,
    discovered_items: HashMap<i64, DiscoveredItem>,
    opened_chests: HashSet<i64>,
    opened_caged_wisps: HashSet<i64>,

    snapshots: Vec<MeterSnapshot>,
    session: SessionStats,
    archived_sessions: Vec<ArchivedSession>,
    faction_map: BTreeMap<CityFaction, (f64, f64, f64)>,
    dungeons: Vec<DungeonRun>,
    gathering_totals: GatheringTotals,
    gathering_log: Vec<GatheringEntry>,
    loot_log: Vec<LootLogEntry>,
    market_orders: Vec<MarketOrderEntry>,
    mail_notifications: Vec<MailNotificationEntry>,
    trades: Vec<TradeLogEntry>,

    live_session_zones: Vec<SessionZoneSummary>,
    live_session_combat: BTreeMap<String, InternalEntity>,
    live_session_dungeons: Vec<DungeonRun>,
    live_session_gathering_totals: GatheringTotals,
    live_session_gathering_log: Vec<GatheringEntry>,
    live_session_loot_log: Vec<LootLogEntry>,
    live_session_trades: Vec<TradeLogEntry>,
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
                current_map_index: String::new(),
                current_map_name_fr: "En attente…".into(),
                silver: 0.0,
                gold: 0.0,
                focus_points: 0.0,
                max_focus_points: 30_000.0,
                learning_points: 0.0,
                reputation: 0.0,
                is_respec_active: false,
                in_combat: false,
            },
            local_player_guid: Guid16::default(),
            awaiting_cluster_respawn: false,
            entities: BTreeMap::new(),
            guid_to_object_id: HashMap::new(),
            party_guids: HashMap::new(),
            pending_equipment: HashMap::new(),
            last_known_equipment: HashMap::new(),
            last_player_health: HashMap::new(),
            discovered_items: HashMap::new(),
            opened_chests: HashSet::new(),
            opened_caged_wisps: HashSet::new(),
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
            archived_sessions: Vec::new(),
            faction_map,
            dungeons: Vec::new(),
            gathering_totals: GatheringTotals::default(),
            gathering_log: Vec::new(),
            loot_log: Vec::new(),
            market_orders: Vec::new(),
            mail_notifications: Vec::new(),
            trades: Vec::new(),
            live_session_zones: Vec::new(),
            live_session_combat: BTreeMap::new(),
            live_session_dungeons: Vec::new(),
            live_session_gathering_totals: GatheringTotals::default(),
            live_session_gathering_log: Vec::new(),
            live_session_loot_log: Vec::new(),
            live_session_trades: Vec::new(),
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
                let (map_name_fr, map_name_en, map_kind, map_tier, _) =
                    self.resolve_cluster_metadata(&map_index, "", "", 0);

                let prev_id = self
                    .guid_to_object_id
                    .get(&user_guid)
                    .copied()
                    .filter(|&id| id != 0 && user_guid != Guid16::default())
                    .unwrap_or(self.local_player.object_id);
                if prev_id != 0 && prev_id != user_object_id {
                    self.migrate_entity_id(prev_id, user_object_id);
                }
                self.deduplicate_self_and_guid(user_object_id, user_guid);
                self.awaiting_cluster_respawn = false;

                if user_guid != Guid16::default() {
                    self.local_player_guid = user_guid;
                }
                let final_username = if !Self::is_placeholder_player_name(&username) {
                    username
                } else if !Self::is_placeholder_player_name(&self.local_player.username) {
                    self.local_player.username.clone()
                } else {
                    format!("Joueur #{user_object_id}")
                };

                self.local_player = LocalPlayerProfile {
                    object_id: user_object_id,
                    username: final_username.clone(),
                    guild_name: guild_name.clone(),
                    alliance_name,
                    current_map_index: map_index.clone(),
                    current_map_name_fr: map_name_fr.clone(),
                    silver: silver.as_f64(),
                    gold: gold.as_f64(),
                    focus_points: current_focus_points,
                    max_focus_points,
                    learning_points: learning_points.as_f64(),
                    reputation,
                    is_respec_active,
                    in_combat: false,
                };

                if user_guid != Guid16::default() {
                    self.party_guids.insert(user_guid, final_username.clone());
                    self.guid_to_object_id.insert(user_guid, user_object_id);
                }

                let cached_equip = if user_guid != Guid16::default() {
                    self.last_known_equipment.get(&user_guid).cloned()
                } else {
                    None
                };
                let pending = self.take_valid_pending_equipment(user_object_id, now_ms);

                let ent = self.ensure_entity(user_object_id);
                if user_guid != Guid16::default() {
                    ent.guid = user_guid;
                }
                ent.name = final_username;
                if !guild_name.is_empty() {
                    ent.guild_name = guild_name;
                }
                ent.is_party = true;
                ent.is_self = true;
                if ent.equipment == CharacterEquipment::default() {
                    if let Some((equip, spec)) = cached_equip {
                        ent.equipment = equip;
                        ent.weapon_specificity = spec;
                    }
                }
                if let Some((p_equip, p_spells)) = pending {
                    self.apply_pending_to_entity(user_object_id, p_equip, p_spells);
                }
                self.sync_live_combat_entity(user_object_id);
                if !map_index.trim().is_empty() {
                    self.record_live_zone(
                        &map_index,
                        &map_name_fr,
                        &map_name_en,
                        &map_kind,
                        map_tier,
                        now_ms,
                        true,
                    );
                }
            }
            AlbionTelemetryEvent::NewCharacter {
                object_id,
                name,
                guid,
                guild_name,
                equipment,
            } => {
                let matches_self_guid = guid != Guid16::default() && guid == self.local_player_guid;
                let matches_self_id =
                    self.local_player.object_id != 0 && object_id == self.local_player.object_id;
                let matches_self_respawn_name = self.awaiting_cluster_respawn
                    && !Self::is_placeholder_player_name(&name)
                    && !Self::is_placeholder_player_name(&self.local_player.username)
                    && name.eq_ignore_ascii_case(&self.local_player.username);
                let is_self = matches_self_guid || matches_self_id || matches_self_respawn_name;

                if is_self {
                    let old_self_id = self.local_player.object_id;
                    if old_self_id != 0 && old_self_id != object_id {
                        self.migrate_entity_id(old_self_id, object_id);
                    }
                    self.local_player.object_id = object_id;
                    let self_guid = if guid != Guid16::default() {
                        self.local_player_guid = guid;
                        guid
                    } else {
                        self.local_player_guid
                    };
                    self.deduplicate_self_and_guid(object_id, self_guid);
                    self.awaiting_cluster_respawn = false;
                } else if guid != Guid16::default() && self.party_guids.contains_key(&guid) {
                    if let Some(&old_id) = self.guid_to_object_id.get(&guid) {
                        if old_id != 0 && old_id != object_id {
                            self.migrate_entity_id(old_id, object_id);
                        }
                    }
                }

                if guid != Guid16::default() {
                    self.guid_to_object_id.insert(guid, object_id);
                }

                let is_party =
                    is_self || (guid != Guid16::default() && self.party_guids.contains_key(&guid));

                let mut resolved_name = if !Self::is_placeholder_player_name(&name) {
                    Some(name.clone())
                } else {
                    None
                };
                if resolved_name.is_none() && guid != Guid16::default() {
                    if let Some(known) = self.party_guids.get(&guid) {
                        if !Self::is_placeholder_player_name(known) {
                            resolved_name = Some(known.clone());
                        }
                    }
                }

                if is_self {
                    if let Some(ref real_name) = resolved_name {
                        self.local_player.username = real_name.clone();
                        if guid != Guid16::default() {
                            self.party_guids.insert(guid, real_name.clone());
                        }
                    } else if guid != Guid16::default()
                        && !Self::is_placeholder_player_name(&self.local_player.username)
                    {
                        self.party_guids
                            .insert(guid, self.local_player.username.clone());
                    }
                    if !guild_name.is_empty() {
                        self.local_player.guild_name = guild_name.clone();
                    }
                }

                let pending = self.take_valid_pending_equipment(object_id, now_ms);
                let cached_equip = if (is_party || is_self) && guid != Guid16::default() {
                    self.last_known_equipment.get(&guid).cloned()
                } else {
                    None
                };

                let self_username_fallback =
                    if is_self && !Self::is_placeholder_player_name(&self.local_player.username) {
                        Some(self.local_player.username.clone())
                    } else {
                        None
                    };

                {
                    let ent = self.ensure_entity(object_id);
                    if guid != Guid16::default() {
                        ent.guid = guid;
                    }
                    if let Some(real_name) = resolved_name {
                        ent.name = real_name;
                    } else if let Some(fallback_name) = self_username_fallback {
                        ent.name = fallback_name;
                    }
                    if !guild_name.is_empty() {
                        ent.guild_name = guild_name;
                    }
                    ent.is_party = is_party;
                    ent.is_self = is_self;
                    if equipment != CharacterEquipment::default() {
                        let prev_mh = ent.equipment.main_hand;
                        let prev_spec = ent.weapon_specificity;
                        ent.equipment = equipment;
                        if ent.equipment.main_hand > 0 {
                            ent.weapon_specificity = 4;
                        } else if prev_mh > 0 && prev_spec > 0 && prev_spec < 4 {
                            ent.equipment.main_hand = prev_mh;
                            ent.weapon_specificity = prev_spec;
                        } else {
                            ent.weapon_specificity = 0;
                        }
                    } else if ent.equipment == CharacterEquipment::default() {
                        if let Some((equip, spec)) = cached_equip {
                            ent.equipment = equip;
                            ent.weapon_specificity = spec;
                        }
                    }
                }
                if let Some((p_equip, p_spells)) = pending {
                    self.apply_pending_to_entity(object_id, p_equip, p_spells);
                }
                if is_party || is_self {
                    self.remember_entity_equipment(object_id);
                    self.sync_live_combat_entity(object_id);
                }
            }
            AlbionTelemetryEvent::CharacterEquipmentChanged {
                object_id,
                equipment,
                equipped_spells,
            } => {
                if object_id == self.local_player.object_id && object_id != 0 {
                    let _ = self.ensure_entity(object_id);
                }
                if self.entities.contains_key(&object_id) {
                    self.apply_equipment_update_to_entity(object_id, equipment, &equipped_spells);
                } else if object_id != 0 {
                    self.prune_stale_pending_equipment(now_ms);
                    self.pending_equipment
                        .insert(object_id, (equipment, equipped_spells, now_ms));
                }
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
                            if Self::is_placeholder_player_name(&ent.name)
                                && !Self::is_placeholder_player_name(&username)
                            {
                                ent.name = username;
                            }
                        }
                        self.sync_live_combat_entity(obj_id);
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
                        if Self::is_placeholder_player_name(&ent.name)
                            && !Self::is_placeholder_player_name(&username)
                        {
                            ent.name = username;
                        }
                    }
                    self.sync_live_combat_entity(obj_id);
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
                // Weapon inference (`DetectUsedWeapon`) restricted strictly to party/self
                let is_party_or_self = (causer_id == self.local_player.object_id
                    && self.local_player.object_id != 0)
                    || self
                        .entities
                        .get(&causer_id)
                        .map(|e| e.is_party || e.is_self)
                        .unwrap_or(false);
                if is_party_or_self {
                    if let Some((weapon, spec)) =
                        self.catalog.infer_best_weapon_from_spells(&spell_indices)
                    {
                        let weapon_idx = weapon.index;
                        let ent = self.ensure_entity(causer_id);
                        if ent.weapon_specificity < 4 && spec > ent.weapon_specificity {
                            ent.equipment.main_hand = weapon_idx;
                            ent.weapon_specificity = spec;
                        }
                        self.remember_entity_equipment(causer_id);
                        self.sync_live_combat_entity(causer_id);
                    }
                }
            }
            AlbionTelemetryEvent::InCombatStateUpdate {
                object_id,
                in_active_combat,
                in_passive_combat,
            } => {
                if self.local_player.object_id == 0 && object_id != 0 {
                    self.adopt_local_object_id(object_id);
                }
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
                object_id,
                fame_with_zone_multiplier,
                zone_fame,
                satchel_fame,
                ..
            } => {
                if self.local_player.object_id == 0 && object_id != 0 {
                    self.adopt_local_object_id(object_id);
                }
                if self.is_local_object(object_id) {
                    let gained = fame_with_zone_multiplier.as_f64().max(0.0);
                    self.session.fame += gained;
                    self.session.zone_fame += zone_fame.as_f64().max(0.0);
                    self.session.satchel_fame += satchel_fame.as_f64().max(0.0);
                    if let Some(run) = self.dungeons.first_mut().filter(|d| d.is_active) {
                        run.fame += gained;
                    }
                    if let Some(run) = self
                        .live_session_dungeons
                        .first_mut()
                        .filter(|d| d.is_active)
                    {
                        run.fame += gained;
                    }
                    if let Some(zone) = self.ensure_live_zone(now_ms) {
                        zone.fame += gained;
                    }
                }
            }
            AlbionTelemetryEvent::TakeSilver {
                object_id,
                yield_pre_tax,
                guild_tax,
                cluster_tax,
                ..
            } => {
                if self.local_player.object_id == 0 && object_id != 0 {
                    self.adopt_local_object_id(object_id);
                }
                if self.is_local_object(object_id) {
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
                    if let Some(run) = self
                        .live_session_dungeons
                        .first_mut()
                        .filter(|d| d.is_active)
                    {
                        run.silver += net;
                    }
                    if let Some(zone) = self.ensure_live_zone(now_ms) {
                        zone.silver += net;
                    }
                }
            }
            AlbionTelemetryEvent::UpdateMoney {
                object_id,
                current_player_silver,
            } => {
                if self.local_player.object_id == 0 && object_id != 0 {
                    self.adopt_local_object_id(object_id);
                } else if self.awaiting_cluster_respawn
                    && object_id != 0
                    && object_id != self.local_player.object_id
                {
                    // UpdateMoney is a private server->self wallet packet; safe signal for new cluster ID
                    self.migrate_local_player_id(object_id);
                }
                if self.is_local_object(object_id) {
                    self.local_player.silver = current_player_silver.as_f64();
                }
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
                if let Some(run) = self
                    .live_session_dungeons
                    .first_mut()
                    .filter(|d| d.is_active)
                {
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
                if let Some(run) = self
                    .live_session_dungeons
                    .first_mut()
                    .filter(|d| d.is_active)
                {
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
                        if let Some(zone) = self.ensure_live_zone(now_ms) {
                            zone.deaths += 1;
                        }
                    } else if killed_by.eq_ignore_ascii_case(&self.local_player.username) {
                        self.session.kills += 1;
                        if let Some(zone) = self.ensure_live_zone(now_ms) {
                            zone.kills += 1;
                        }
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
            AlbionTelemetryEvent::LootChestOpened { object_id } => {
                if self.opened_chests.insert(object_id) {
                    if let Some(run) = self.dungeons.first_mut().filter(|d| d.is_active) {
                        run.chests_opened += 1;
                    }
                    if let Some(run) = self
                        .live_session_dungeons
                        .first_mut()
                        .filter(|d| d.is_active)
                    {
                        run.chests_opened += 1;
                    }
                }
            }
            AlbionTelemetryEvent::MistsWispCageOpened { object_id } => {
                if self.opened_caged_wisps.insert(object_id) {
                    if let Some(run) = self.dungeons.first_mut().filter(|d| d.is_active) {
                        run.caged_wisps_opened += 1;
                    }
                    if let Some(run) = self
                        .live_session_dungeons
                        .first_mut()
                        .filter(|d| d.is_active)
                    {
                        run.caged_wisps_opened += 1;
                    }
                }
            }
            AlbionTelemetryEvent::HarvestFinished {
                user_object_id,
                item_id,
                standard_amount,
                collector_bonus_amount,
                premium_bonus_amount,
                ..
            } => {
                if self.local_player.object_id == 0 && user_object_id != 0 {
                    self.adopt_local_object_id(user_object_id);
                }
                if self.is_local_object(user_object_id) {
                    self.handle_harvest(
                        item_id,
                        standard_amount,
                        collector_bonus_amount,
                        premium_bonus_amount,
                        now_ms,
                    );
                }
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
                if self.is_party_or_self_looter(&looted_by_name) {
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
                let entry = TradeLogEntry {
                    id,
                    timestamp_ms: now_ms,
                    player1_silver: player1_silver.as_f64(),
                    player2_silver: player2_silver.as_f64(),
                    sales_tax: sales_tax.as_f64(),
                    item_count,
                };
                self.trades.insert(0, entry.clone());
                if self.trades.len() > MAX_MARKET_ENTRIES {
                    self.trades.truncate(MAX_MARKET_ENTRIES);
                }
                self.live_session_trades.insert(0, entry);
                if self.live_session_trades.len() > MAX_MARKET_ENTRIES {
                    self.live_session_trades.truncate(MAX_MARKET_ENTRIES);
                }
            }
            AlbionTelemetryEvent::Leave { object_id } => {
                self.last_player_health.remove(&object_id);
                self.discovered_items.remove(&object_id);
                self.pending_equipment.remove(&object_id);
                let non_party_guid = self
                    .entities
                    .get(&object_id)
                    .filter(|e| !e.is_party && !e.is_self)
                    .map(|e| e.guid);
                if let Some(guid) = non_party_guid {
                    self.entities.remove(&object_id);
                    if self.guid_to_object_id.get(&guid) == Some(&object_id) {
                        self.guid_to_object_id.remove(&guid);
                    }
                }
            }
            AlbionTelemetryEvent::NewLoot { .. } => {}
        }
    }

    fn is_placeholder_player_name(name: &str) -> bool {
        let trimmed = name.trim();
        trimmed.is_empty()
            || trimmed == "PA"
            || trimmed.starts_with("Joueur #")
            || trimmed.starts_with("En attente")
    }

    fn session_combat_key(object_id: i64, guid: Guid16) -> String {
        if guid != Guid16::default() {
            guid.to_uuid_string()
        } else {
            format!("id:{object_id}")
        }
    }

    fn merge_internal_entity(dst: &mut InternalEntity, src: InternalEntity) {
        if dst.guid == Guid16::default() && src.guid != Guid16::default() {
            dst.guid = src.guid;
        }
        if Self::is_placeholder_player_name(&dst.name)
            && !Self::is_placeholder_player_name(&src.name)
        {
            dst.name = src.name;
        }
        if dst.guild_name.is_empty() && !src.guild_name.is_empty() {
            dst.guild_name = src.guild_name;
        }
        dst.is_party |= src.is_party;
        dst.is_self |= src.is_self;
        if dst.equipment == CharacterEquipment::default()
            && src.equipment != CharacterEquipment::default()
        {
            dst.equipment = src.equipment;
            dst.weapon_specificity = src.weapon_specificity;
        } else if src.equipment.main_hand > 0
            && (dst.equipment.main_hand == 0 || src.weapon_specificity > dst.weapon_specificity)
        {
            dst.equipment.main_hand = src.equipment.main_hand;
            dst.weapon_specificity = src.weapon_specificity;
        }
        dst.damage += src.damage;
        dst.heal += src.heal;
        dst.overhealed += src.overhealed;
        dst.taken_damage += src.taken_damage;
        dst.first_combat_ms = match (dst.first_combat_ms, src.first_combat_ms) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        dst.last_combat_ms = match (dst.last_combat_ms, src.last_combat_ms) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        for (sp_idx, acc) in src.spells {
            let entry = dst.spells.entry(sp_idx).or_default();
            entry.ticks += acc.ticks;
            entry.damage += acc.damage;
            entry.heal += acc.heal;
            entry.overhealed += acc.overhealed;
        }
    }

    fn migrate_entity_id(&mut self, old_id: i64, new_id: i64) {
        if old_id == 0 || new_id == 0 || old_id == new_id {
            return;
        }
        if let Some(mut old_ent) = self.entities.remove(&old_id) {
            old_ent.object_id = new_id;
            if let Some(new_ent) = self.entities.get_mut(&new_id) {
                Self::merge_internal_entity(new_ent, old_ent);
            } else {
                self.entities.insert(new_id, old_ent);
            }
        }
        if let Some(hp) = self.last_player_health.remove(&old_id) {
            self.last_player_health.entry(new_id).or_insert(hp);
        }
        for mapped in self.guid_to_object_id.values_mut() {
            if *mapped == old_id {
                *mapped = new_id;
            }
        }
        let old_sc_key = format!("id:{old_id}");
        if let Some(mut old_sc) = self.live_session_combat.remove(&old_sc_key) {
            let target_guid = self
                .entities
                .get(&new_id)
                .map(|e| e.guid)
                .unwrap_or(old_sc.guid);
            old_sc.object_id = new_id;
            let target_key = Self::session_combat_key(new_id, target_guid);
            if let Some(dst) = self.live_session_combat.get_mut(&target_key) {
                Self::merge_internal_entity(dst, old_sc);
            } else {
                self.live_session_combat.insert(target_key, old_sc);
            }
        }
    }

    fn deduplicate_self_and_guid(&mut self, keep_object_id: i64, self_guid: Guid16) {
        let dup_ids: Vec<i64> = self
            .entities
            .iter()
            .filter_map(|(&id, ent)| {
                let same_guid = self_guid != Guid16::default() && ent.guid == self_guid;
                (id != keep_object_id && (ent.is_self || same_guid)).then_some(id)
            })
            .collect();
        for old_id in dup_ids {
            self.migrate_entity_id(old_id, keep_object_id);
        }
        for ent in self.entities.values_mut() {
            ent.is_self = ent.object_id == keep_object_id && keep_object_id != 0;
            if ent.is_self {
                ent.is_party = true;
            }
        }
    }

    fn migrate_local_player_id(&mut self, new_object_id: i64) {
        if new_object_id == 0 {
            return;
        }
        let old_id = self.local_player.object_id;
        if old_id != 0 && old_id != new_object_id {
            self.migrate_entity_id(old_id, new_object_id);
        }
        self.local_player.object_id = new_object_id;
        let self_guid = self.local_player_guid;
        self.deduplicate_self_and_guid(new_object_id, self_guid);
        self.awaiting_cluster_respawn = false;

        let username_fallback = if !Self::is_placeholder_player_name(&self.local_player.username) {
            Some(self.local_player.username.clone())
        } else {
            None
        };

        let (guid_found, name_to_adopt, guild_to_adopt) = {
            let ent = self.ensure_entity(new_object_id);
            ent.is_self = true;
            ent.is_party = true;
            if ent.guid == Guid16::default() && self_guid != Guid16::default() {
                ent.guid = self_guid;
            }
            if !Self::is_placeholder_player_name(&ent.name) {
                (
                    ent.guid,
                    Some(ent.name.clone()),
                    Some(ent.guild_name.clone()),
                )
            } else {
                if let Some(fallback_name) = username_fallback {
                    ent.name = fallback_name;
                }
                (ent.guid, None, None)
            }
        };

        if self.local_player_guid == Guid16::default() && guid_found != Guid16::default() {
            self.local_player_guid = guid_found;
        }
        if let (Some(name), Some(guild)) = (name_to_adopt, guild_to_adopt) {
            self.local_player.username = name.clone();
            if !guild.is_empty() {
                self.local_player.guild_name = guild;
            }
            if self.local_player_guid != Guid16::default() {
                self.party_guids.insert(self.local_player_guid, name);
            }
        } else if self.local_player.username.starts_with("En attente") {
            self.local_player.username = format!("Joueur #{new_object_id}");
        }
        if self.local_player_guid != Guid16::default() {
            self.guid_to_object_id
                .insert(self.local_player_guid, new_object_id);
        }
        if let Some((p_equip, p_spells)) =
            self.take_valid_pending_equipment(new_object_id, self.last_now_ms)
        {
            self.apply_pending_to_entity(new_object_id, p_equip, p_spells);
        } else if self.local_player_guid != Guid16::default() {
            let cached = self
                .last_known_equipment
                .get(&self.local_player_guid)
                .cloned();
            if let (Some((equip, spec)), Some(ent)) =
                (cached, self.entities.get_mut(&new_object_id))
            {
                if ent.equipment == CharacterEquipment::default() {
                    ent.equipment = equip;
                    ent.weapon_specificity = spec;
                }
            }
        }
        self.remember_entity_equipment(new_object_id);
        self.sync_live_combat_entity(new_object_id);
    }

    fn adopt_local_object_id(&mut self, object_id: i64) {
        self.migrate_local_player_id(object_id);
    }

    fn prune_stale_pending_equipment(&mut self, now_ms: u64) {
        self.pending_equipment
            .retain(|_, (_, _, ts)| now_ms.saturating_sub(*ts) <= 30_000);
    }

    fn take_valid_pending_equipment(
        &mut self,
        object_id: i64,
        now_ms: u64,
    ) -> Option<(CharacterEquipment, Vec<i32>)> {
        let (equip, spells, ts) = self.pending_equipment.remove(&object_id)?;
        if now_ms.saturating_sub(ts) <= 30_000 {
            Some((equip, spells))
        } else {
            None
        }
    }

    fn apply_pending_to_entity(
        &mut self,
        object_id: i64,
        equipment: CharacterEquipment,
        equipped_spells: Vec<i32>,
    ) {
        let best_from_spells = self
            .catalog
            .infer_best_weapon_from_spells(&equipped_spells)
            .map(|(w, s)| (w.index, s));
        if let Some(ent) = self.entities.get_mut(&object_id) {
            if equipment != CharacterEquipment::default() {
                if equipment.main_hand > 0 {
                    ent.equipment = equipment;
                    ent.weapon_specificity = 4;
                } else {
                    let prev_mh = ent.equipment.main_hand;
                    let prev_spec = ent.weapon_specificity;
                    ent.equipment = equipment;
                    if let Some((w_idx, spec)) = best_from_spells {
                        ent.equipment.main_hand = w_idx;
                        ent.weapon_specificity = spec;
                    } else if prev_mh > 0 && prev_spec > 0 && prev_spec < 4 {
                        ent.equipment.main_hand = prev_mh;
                        ent.weapon_specificity = prev_spec;
                    }
                }
            } else if ent.weapon_specificity < 4 {
                if let Some((w_idx, spec)) = best_from_spells {
                    if spec > ent.weapon_specificity {
                        ent.equipment.main_hand = w_idx;
                        ent.weapon_specificity = spec;
                    }
                }
            }
        }
    }

    fn apply_equipment_update_to_entity(
        &mut self,
        object_id: i64,
        equipment: CharacterEquipment,
        equipped_spells: &[i32],
    ) {
        let best_from_spells = self
            .catalog
            .infer_best_weapon_from_spells(equipped_spells)
            .map(|(w, s)| (w.index, s));
        let mut is_tracked = false;
        if let Some(ent) = self.entities.get_mut(&object_id) {
            let prev_mh = ent.equipment.main_hand;
            let prev_spec = ent.weapon_specificity;
            if equipment != CharacterEquipment::default() {
                ent.equipment = equipment;
                if ent.equipment.main_hand > 0 {
                    ent.weapon_specificity = 4;
                } else if let Some((w_idx, spec)) = best_from_spells {
                    ent.equipment.main_hand = w_idx;
                    ent.weapon_specificity = spec;
                } else if prev_mh > 0
                    && prev_spec > 0
                    && prev_spec < 4
                    && equipped_spells.is_empty()
                {
                    ent.equipment.main_hand = prev_mh;
                    ent.weapon_specificity = prev_spec;
                } else {
                    ent.weapon_specificity = 0;
                }
            } else if ent.weapon_specificity < 4 {
                if let Some((w_idx, spec)) = best_from_spells {
                    if spec > ent.weapon_specificity {
                        ent.equipment.main_hand = w_idx;
                        ent.weapon_specificity = spec;
                    }
                }
            }
            is_tracked = ent.is_party || ent.is_self;
        }
        if is_tracked {
            self.remember_entity_equipment(object_id);
            self.sync_live_combat_entity(object_id);
        }
    }

    fn remember_entity_equipment(&mut self, object_id: i64) {
        if let Some(ent) = self.entities.get(&object_id) {
            if (ent.is_party || ent.is_self)
                && ent.guid != Guid16::default()
                && ent.equipment != CharacterEquipment::default()
            {
                self.last_known_equipment
                    .insert(ent.guid, (ent.equipment.clone(), ent.weapon_specificity));
            }
        }
    }

    fn sync_live_combat_entity(&mut self, object_id: i64) {
        let Some(src) = self.entities.get(&object_id) else {
            return;
        };
        if !src.is_party && !src.is_self {
            return;
        }
        let guid = src.guid;
        let name = src.name.clone();
        let guild_name = src.guild_name.clone();
        let is_party = src.is_party;
        let is_self = src.is_self;
        let main_hand = src.equipment.main_hand;
        let weapon_specificity = src.weapon_specificity;

        let target_key = Self::session_combat_key(object_id, guid);
        if guid != Guid16::default() {
            let legacy_key = format!("id:{object_id}");
            if let Some(mut legacy_ent) = self.live_session_combat.remove(&legacy_key) {
                legacy_ent.guid = guid;
                if let Some(existing) = self.live_session_combat.get_mut(&target_key) {
                    Self::merge_internal_entity(existing, legacy_ent);
                } else {
                    self.live_session_combat
                        .insert(target_key.clone(), legacy_ent);
                }
            }
        }
        if let Some(dst) = self.live_session_combat.get_mut(&target_key) {
            dst.object_id = object_id;
            if guid != Guid16::default() {
                dst.guid = guid;
            }
            if !Self::is_placeholder_player_name(&name) {
                dst.name = name;
            }
            if !guild_name.is_empty() {
                dst.guild_name = guild_name;
            }
            dst.is_party |= is_party;
            dst.is_self |= is_self;
            if main_hand > 0
                && (dst.equipment.main_hand == 0 || weapon_specificity >= dst.weapon_specificity)
            {
                dst.equipment.main_hand = main_hand;
                dst.weapon_specificity = weapon_specificity;
            }
        }
    }

    #[inline]
    fn is_local_object(&self, object_id: i64) -> bool {
        self.local_player.object_id == 0
            || object_id == 0
            || object_id == self.local_player.object_id
    }

    fn is_party_or_self_looter(&self, name: &str) -> bool {
        if self.local_player.object_id == 0 && self.party_guids.is_empty() {
            return true;
        }
        if name.is_empty() {
            return false;
        }
        if !self.local_player.username.is_empty()
            && name.eq_ignore_ascii_case(&self.local_player.username)
        {
            return true;
        }
        if self
            .party_guids
            .values()
            .any(|member| !member.is_empty() && member.eq_ignore_ascii_case(name))
        {
            return true;
        }
        self.entities.values().any(|ent| {
            (ent.is_party || ent.is_self)
                && !ent.name.is_empty()
                && ent.name.eq_ignore_ascii_case(name)
        })
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
                weapon_specificity: 0,
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

        let mut causer_in_party = self
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
            if self.local_player.object_id == 0
                && tick.causer_id != 0
                && self.party_guids.is_empty()
            {
                self.adopt_local_object_id(tick.causer_id);
                causer_in_party = true;
            } else {
                return;
            }
        }

        self.combat_start_ms = Some(self.combat_start_ms.map_or(now_ms, |s| s.min(now_ms)));
        self.combat_end_ms = Some(self.combat_end_ms.map_or(now_ms, |e| e.max(now_ms)));

        if causer_in_party && tick.causer_id != 0 {
            let weapon_inferred = self
                .catalog
                .infer_weapon_from_spell_with_specificity(spell_idx)
                .map(|(w, spec)| (w.index, spec));
            let causer_snapshot = {
                let causer = self.ensure_entity(tick.causer_id);
                if causer.weapon_specificity < 4 {
                    if let Some((w_id, spec)) = weapon_inferred {
                        if spec > causer.weapon_specificity {
                            causer.equipment.main_hand = w_id;
                            causer.weapon_specificity = spec;
                        }
                    }
                }
                causer.first_combat_ms =
                    Some(causer.first_combat_ms.map_or(now_ms, |s| s.min(now_ms)));
                causer.last_combat_ms =
                    Some(causer.last_combat_ms.map_or(now_ms, |e| e.max(now_ms)));

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
                (
                    causer.guid,
                    causer.name.clone(),
                    causer.guild_name.clone(),
                    causer.is_party,
                    causer.is_self,
                    causer.equipment.clone(),
                    causer.weapon_specificity,
                )
            };

            let (c_guid, c_name, c_guild, c_party, c_self, c_equip, c_spec) = causer_snapshot;
            let sc_key = Self::session_combat_key(tick.causer_id, c_guid);
            let sc_ent = self
                .live_session_combat
                .entry(sc_key)
                .or_insert_with(|| InternalEntity {
                    object_id: tick.causer_id,
                    guid: c_guid,
                    name: c_name.clone(),
                    guild_name: c_guild.clone(),
                    is_party: c_party,
                    is_self: c_self,
                    equipment: c_equip.clone(),
                    weapon_specificity: c_spec,
                    damage: 0,
                    heal: 0,
                    overhealed: 0,
                    taken_damage: 0,
                    first_combat_ms: None,
                    last_combat_ms: None,
                    spells: BTreeMap::new(),
                });
            sc_ent.object_id = tick.causer_id;
            if !Self::is_placeholder_player_name(&c_name) {
                sc_ent.name = c_name;
            }
            if !c_guild.is_empty() {
                sc_ent.guild_name = c_guild;
            }
            sc_ent.is_party |= c_party;
            sc_ent.is_self |= c_self;
            if c_equip.main_hand > 0
                && (sc_ent.equipment.main_hand == 0 || c_spec >= sc_ent.weapon_specificity)
            {
                sc_ent.equipment.main_hand = c_equip.main_hand;
                sc_ent.weapon_specificity = c_spec;
            }
            sc_ent.first_combat_ms = Some(sc_ent.first_combat_ms.map_or(now_ms, |s| s.min(now_ms)));
            sc_ent.last_combat_ms = Some(sc_ent.last_combat_ms.map_or(now_ms, |e| e.max(now_ms)));
            let sc_spell = sc_ent.spells.entry(spell_idx).or_default();
            sc_spell.ticks += 1;
            if is_damage {
                sc_ent.damage += amount;
                sc_spell.damage += amount;
            } else if is_max_health_reached {
                sc_ent.overhealed += amount;
                sc_spell.overhealed += amount;
            } else {
                sc_ent.heal += amount;
                sc_spell.heal += amount;
            }
        }

        if is_damage && affected_in_party && tick.affected_object_id != 0 {
            let affected_snapshot = {
                let affected = self.ensure_entity(tick.affected_object_id);
                affected.taken_damage += amount;
                affected.first_combat_ms =
                    Some(affected.first_combat_ms.map_or(now_ms, |s| s.min(now_ms)));
                affected.last_combat_ms =
                    Some(affected.last_combat_ms.map_or(now_ms, |e| e.max(now_ms)));
                (
                    affected.guid,
                    affected.name.clone(),
                    affected.guild_name.clone(),
                    affected.is_party,
                    affected.is_self,
                    affected.equipment.clone(),
                    affected.weapon_specificity,
                )
            };

            let (a_guid, a_name, a_guild, a_party, a_self, a_equip, a_spec) = affected_snapshot;
            let sc_key = Self::session_combat_key(tick.affected_object_id, a_guid);
            let sc_ent = self
                .live_session_combat
                .entry(sc_key)
                .or_insert_with(|| InternalEntity {
                    object_id: tick.affected_object_id,
                    guid: a_guid,
                    name: a_name.clone(),
                    guild_name: a_guild.clone(),
                    is_party: a_party,
                    is_self: a_self,
                    equipment: a_equip,
                    weapon_specificity: a_spec,
                    damage: 0,
                    heal: 0,
                    overhealed: 0,
                    taken_damage: 0,
                    first_combat_ms: None,
                    last_combat_ms: None,
                    spells: BTreeMap::new(),
                });
            sc_ent.object_id = tick.affected_object_id;
            if !Self::is_placeholder_player_name(&a_name) {
                sc_ent.name = a_name;
            }
            if !a_guild.is_empty() {
                sc_ent.guild_name = a_guild;
            }
            sc_ent.is_party |= a_party;
            sc_ent.is_self |= a_self;
            sc_ent.taken_damage += amount;
            sc_ent.first_combat_ms = Some(sc_ent.first_combat_ms.map_or(now_ms, |s| s.min(now_ms)));
            sc_ent.last_combat_ms = Some(sc_ent.last_combat_ms.map_or(now_ms, |e| e.max(now_ms)));
        }
    }

    fn is_instanced_cluster_kind(kind: &str, index: &str) -> bool {
        let upper_index = index.to_ascii_uppercase();
        matches!(
            kind,
            "mists"
                | "soloDungeon"
                | "groupDungeon"
                | "avalonianDungeon"
                | "hellgate"
                | "corruptedDungeon"
                | "abyssalDepths"
        ) || upper_index.starts_with("MISTS")
            || upper_index.starts_with("RD_")
            || upper_index.starts_with("HELLGATE")
            || upper_index.starts_with("CORRUPTED")
            || upper_index.starts_with("ABYSSAL")
            || upper_index.contains("@MISTS")
            || upper_index.contains("@RANDOMDUNGEON")
            || upper_index.contains("@CORRUPTEDDUNGEON")
            || upper_index.contains("@HELLCLUSTER")
            || upper_index.contains("@HELLDUNGEON")
            || upper_index.contains("@MISTSDUNGEON")
    }

    fn extract_tier_hint(s: &str) -> Option<u8> {
        let bytes = s.as_bytes();
        for i in 0..bytes.len().saturating_sub(1) {
            if (bytes[i] == b'T' || bytes[i] == b't') && (b'4'..=b'8').contains(&bytes[i + 1]) {
                let prev_ok = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
                let next_ok = i + 2 >= bytes.len() || !bytes[i + 2].is_ascii_alphanumeric();
                if prev_ok && next_ok {
                    return Some(bytes[i + 1] - b'0');
                }
            }
        }
        None
    }

    fn resolve_cluster_metadata(
        &self,
        index: &str,
        world_map_data_type: &str,
        island_name: &str,
        mists_dungeon_tier: i32,
    ) -> (String, String, String, u8, bool) {
        if let Some(c) = self.catalog.cluster(index) {
            if !c.name_fr.is_empty() {
                let is_instanced = Self::is_instanced_cluster_kind(&c.kind, index);
                return (
                    c.name_fr.clone(),
                    c.name_en.clone(),
                    c.kind.clone(),
                    c.tier,
                    is_instanced,
                );
            }
        }

        let upper_index = index.to_ascii_uppercase();
        let upper_type = world_map_data_type.to_ascii_uppercase();
        let has_token = |tok: &str| upper_index.contains(tok) || upper_type.contains(tok);

        let (kind, fallback_fr, fallback_en, instanced_hint) =
            if has_token("@HELLDUNGEON") || has_token("ABYSSAL") {
                (
                    "abyssalDepths".to_string(),
                    "Profondeurs Abyssales".to_string(),
                    "Abyssal Depths".to_string(),
                    true,
                )
            } else if has_token("@HELLCLUSTER") || has_token("HELLGATE") {
                (
                    "hellgate".to_string(),
                    "Hellgate".to_string(),
                    "Hellgate".to_string(),
                    true,
                )
            } else if has_token("@CORRUPTEDDUNGEON") || has_token("CORRUPTED") {
                (
                    "corruptedDungeon".to_string(),
                    "Donjon Corrompu".to_string(),
                    "Corrupted Dungeon".to_string(),
                    true,
                )
            } else if has_token("@MISTSDUNGEON") || has_token("MISTSDUNGEON") {
                (
                    "mists".to_string(),
                    "Donjon des Brumes".to_string(),
                    "Mists Dungeon".to_string(),
                    true,
                )
            } else if has_token("@MISTS")
                || upper_index.starts_with("MISTS")
                || upper_type == "MISTS"
                || upper_type.contains("MISTS_")
                || upper_type.contains("_MISTS")
            {
                let (fr, en) = if has_token("DUO") {
                    ("Brumes (Duo)", "Mists (Duo)")
                } else {
                    ("Brumes", "Mists")
                };
                ("mists".to_string(), fr.to_string(), en.to_string(), true)
            } else if has_token("@RANDOMDUNGEON")
                || upper_index.starts_with("RD_")
                || has_token("RANDOMDUNGEON")
                || upper_type.contains("_RD_")
            {
                if has_token("AVALON") {
                    (
                        "avalonianDungeon".to_string(),
                        "Donjon Avalonien".to_string(),
                        "Avalonian Dungeon".to_string(),
                        true,
                    )
                } else if has_token("GROUP") || has_token("VET") {
                    (
                        "groupDungeon".to_string(),
                        "Donjon de Groupe".to_string(),
                        "Group Dungeon".to_string(),
                        true,
                    )
                } else {
                    (
                        "soloDungeon".to_string(),
                        "Donjon aléatoire".to_string(),
                        "Random Dungeon".to_string(),
                        true,
                    )
                }
            } else if has_token("@ISLAND") || !island_name.trim().is_empty() {
                let label = if !island_name.trim().is_empty() {
                    island_name.trim().to_string()
                } else {
                    "Île personnelle".to_string()
                };
                ("island".to_string(), label.clone(), label, false)
            } else if has_token("@HIDEOUT") {
                (
                    "hideout".to_string(),
                    "Repaire".to_string(),
                    "Hideout".to_string(),
                    false,
                )
            } else if has_token("@EXPEDITION") {
                (
                    "expedition".to_string(),
                    "Expédition".to_string(),
                    "Expedition".to_string(),
                    false,
                )
            } else if has_token("@ARENA") {
                (
                    "arena".to_string(),
                    "Arène".to_string(),
                    "Arena".to_string(),
                    false,
                )
            } else if has_token("@DRAGONAREA") {
                (
                    "openWorld".to_string(),
                    "Zone Dragon".to_string(),
                    "Dragon Area".to_string(),
                    false,
                )
            } else {
                let fallback_kind = if !world_map_data_type.trim().is_empty() {
                    world_map_data_type.trim().to_string()
                } else {
                    "openWorld".to_string()
                };
                let label = if !island_name.trim().is_empty() {
                    island_name.trim().to_string()
                } else if !index.trim().is_empty() {
                    format!("Zone {index}")
                } else {
                    "Zone inconnue".to_string()
                };
                let instanced = Self::is_instanced_cluster_kind(&fallback_kind, index);
                (fallback_kind, label.clone(), label, instanced)
            };

        let tier = if (4..=8).contains(&mists_dungeon_tier) {
            mists_dungeon_tier as u8
        } else {
            Self::extract_tier_hint(world_map_data_type)
                .or_else(|| Self::extract_tier_hint(index))
                .unwrap_or(if instanced_hint { 6 } else { 4 })
        };

        let is_instanced = instanced_hint || Self::is_instanced_cluster_kind(&kind, index);
        (fallback_fr, fallback_en, kind, tier, is_instanced)
    }

    #[allow(clippy::too_many_arguments)]
    fn record_live_zone(
        &mut self,
        index: &str,
        name_fr: &str,
        name_en: &str,
        kind: &str,
        tier: u8,
        now_ms: u64,
        from_join: bool,
    ) {
        let trimmed = index.trim();
        if trimmed.is_empty() {
            return;
        }
        if let Some(cur) = self.live_session_zones.first_mut() {
            if cur.cluster_index == trimmed {
                if !from_join {
                    if cur.name_fr.starts_with("Zone ") || !name_fr.starts_with("Zone ") {
                        cur.name_fr = name_fr.to_string();
                        cur.name_en = name_en.to_string();
                    }
                    cur.kind = kind.to_string();
                    cur.tier = tier;
                }
                return;
            }
            cur.duration_seconds =
                ((now_ms.saturating_sub(cur.entered_at_ms)) as f64 / 1000.0).max(1.0);
        }
        self.live_session_zones.insert(
            0,
            SessionZoneSummary {
                cluster_index: trimmed.to_string(),
                name_fr: name_fr.to_string(),
                name_en: name_en.to_string(),
                kind: kind.to_string(),
                tier,
                entered_at_ms: now_ms,
                duration_seconds: 0.0,
                fame: 0.0,
                silver: 0.0,
                gathering_nodes: 0,
                gathering_resources: 0,
                kills: 0,
                deaths: 0,
            },
        );
        if self.live_session_zones.len() > MAX_SESSION_ZONES {
            self.live_session_zones.truncate(MAX_SESSION_ZONES);
        }
    }

    fn ensure_live_zone(&mut self, _now_ms: u64) -> Option<&mut SessionZoneSummary> {
        if self.live_session_zones.is_empty() {
            let idx = if !self.local_player.current_map_index.trim().is_empty() {
                self.local_player.current_map_index.clone()
            } else {
                "UNKNOWN".to_string()
            };
            let (resolved_fr, resolved_en, resolved_kind, resolved_tier, _) = if idx != "UNKNOWN" {
                self.resolve_cluster_metadata(&idx, "", "", 0)
            } else {
                let label = if self
                    .local_player
                    .current_map_name_fr
                    .starts_with("En attente")
                {
                    "Zone en cours".to_string()
                } else {
                    self.local_player.current_map_name_fr.clone()
                };
                (label.clone(), label, "openWorld".to_string(), 4, false)
            };
            self.live_session_zones.push(SessionZoneSummary {
                cluster_index: idx,
                name_fr: resolved_fr,
                name_en: resolved_en,
                kind: resolved_kind,
                tier: resolved_tier,
                entered_at_ms: self.session_start_ms,
                duration_seconds: 0.0,
                fame: 0.0,
                silver: 0.0,
                gathering_nodes: 0,
                gathering_resources: 0,
                kills: 0,
                deaths: 0,
            });
        }
        self.live_session_zones.first_mut()
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

        // Purge transient non-party entities and ground items from the previous zone
        self.discovered_items.clear();
        self.opened_chests.clear();
        self.opened_caged_wisps.clear();
        self.last_player_health.clear();
        self.pending_equipment.clear();
        self.entities.retain(|_, e| e.is_party || e.is_self);
        self.guid_to_object_id
            .retain(|_, obj_id| self.entities.contains_key(obj_id));
        self.awaiting_cluster_respawn = true;

        // Finalize any currently active dungeon run (global + live session)
        if let Some(prev) = self.dungeons.first_mut().filter(|d| d.is_active) {
            prev.is_active = false;
            let elapsed_s = ((now_ms.saturating_sub(prev.started_at_ms)) as f64 / 1000.0).max(1.0);
            prev.duration_seconds = elapsed_s;
            prev.fame_per_hour = prev.fame / (elapsed_s / 3600.0);
            self.dirty_history = true;
        }
        if let Some(prev) = self
            .live_session_dungeons
            .first_mut()
            .filter(|d| d.is_active)
        {
            prev.is_active = false;
            let elapsed_s = ((now_ms.saturating_sub(prev.started_at_ms)) as f64 / 1000.0).max(1.0);
            prev.duration_seconds = elapsed_s;
            prev.fame_per_hour = prev.fame / (elapsed_s / 3600.0);
        }

        let (name_fr, name_en, kind, tier, is_instanced) = self.resolve_cluster_metadata(
            index,
            world_map_data_type,
            island_name,
            mists_dungeon_tier,
        );

        self.local_player.current_map_index = index.to_string();
        self.local_player.current_map_name_fr = name_fr.clone();
        self.record_live_zone(index, &name_fr, &name_en, &kind, tier, now_ms, false);

        if is_instanced {
            let id = self.alloc_id();
            let run = DungeonRun {
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
                caged_wisps_opened: 0,
            };
            self.dungeons.insert(0, run.clone());
            if self.dungeons.len() > MAX_DUNGEON_RUNS {
                self.dungeons.truncate(MAX_DUNGEON_RUNS);
            }
            self.live_session_dungeons.insert(0, run);
            if self.live_session_dungeons.len() > MAX_DUNGEON_RUNS {
                self.live_session_dungeons.truncate(MAX_DUNGEON_RUNS);
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

        let resource_kind = Self::classify_resource_kind(&unique_name);
        Self::add_gathering_kind_amount(&mut self.gathering_totals, resource_kind, total as i64);
        Self::add_gathering_kind_amount(
            &mut self.live_session_gathering_totals,
            resource_kind,
            total as i64,
        );

        self.gathering_totals.total_nodes += 1;
        self.gathering_totals.total_resources += total as i64;
        self.gathering_totals.total_estimated_silver += estimated_silver;
        self.live_session_gathering_totals.total_nodes += 1;
        self.live_session_gathering_totals.total_resources += total as i64;
        self.live_session_gathering_totals.total_estimated_silver += estimated_silver;

        if let Some(zone) = self.ensure_live_zone(now_ms) {
            zone.gathering_nodes += 1;
            zone.gathering_resources += total as i64;
        }

        let id = self.alloc_id();
        let entry = GatheringEntry {
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
        };
        self.gathering_log.insert(0, entry.clone());
        if self.gathering_log.len() > MAX_GATHERING_LOG {
            self.gathering_log.truncate(MAX_GATHERING_LOG);
        }
        self.live_session_gathering_log.insert(0, entry);
        if self.live_session_gathering_log.len() > MAX_GATHERING_LOG {
            self.live_session_gathering_log.truncate(MAX_GATHERING_LOG);
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
        if !is_silver {
            if let Some(disc) = self.discovered_items.get(&object_id) {
                if item_index <= 0 {
                    item_index = disc.item_id;
                }
                if quantity <= 0 {
                    quantity = disc.quantity;
                }
                estimated_silver = disc.estimated_market_value;
            }
        }

        // 2-second deduplication window for repeated pickup notifications
        let is_dup = self.loot_log.iter().take(16).any(|prev| {
            now_ms.saturating_sub(prev.timestamp_ms) <= LOOT_DEDUP_WINDOW_MS
                && prev.is_silver == is_silver
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
            "Silver".into()
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
                quantity.max(1) as i64
            } else {
                item.map(|i| i.item_value * 16).unwrap_or(1_000) * (quantity.max(1) as i64)
            };
        }

        let id = self.alloc_id();
        let entry = LootLogEntry {
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
        };
        self.loot_log.insert(0, entry.clone());
        if self.loot_log.len() > MAX_LOOT_LOG {
            self.loot_log.truncate(MAX_LOOT_LOG);
        }
        self.live_session_loot_log.insert(0, entry);
        if self.live_session_loot_log.len() > MAX_LOOT_LOG {
            self.live_session_loot_log.truncate(MAX_LOOT_LOG);
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
        let parts: Vec<&str> = content.split('|').collect();
        if parts.len() < 4 {
            return;
        }
        // Live Albion market mail format: "QUANTITY|UNIQUE_ITEM_NAME|TOTAL_PRICE|UNIT_PRICE"
        // Legacy/simulation format: "UNIT_PRICE|TOTAL_PRICE|AMOUNT|ITEM_UNIQUE_NAME"
        let (unit_raw, total_raw, amount, unique_name) = match (
            parts[0].trim().parse::<i32>(),
            parts[1].trim(),
            parts[2].trim().parse::<i64>(),
            parts[3].trim().parse::<i64>(),
        ) {
            (Ok(qty), item, Ok(total), Ok(unit))
                if qty > 0 && item.bytes().any(|b| b.is_ascii_alphabetic()) =>
            {
                (unit, total, qty, item.to_string())
            }
            _ => (
                parts[0].trim().parse::<i64>().unwrap_or(0),
                parts[1].trim().parse::<i64>().unwrap_or(0),
                parts[2].trim().parse::<i32>().unwrap_or(1).max(1),
                parts[3].trim().to_string(),
            ),
        };
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

    fn classify_resource_kind(unique_name: &str) -> &'static str {
        if unique_name.contains("WOOD") {
            "wood"
        } else if unique_name.contains("ORE") {
            "ore"
        } else if unique_name.contains("FIBER") {
            "fiber"
        } else if unique_name.contains("HIDE") {
            "hide"
        } else if unique_name.contains("ROCK") {
            "rock"
        } else if unique_name.contains("FISH") {
            "fish"
        } else {
            "other"
        }
    }

    fn add_gathering_kind_amount(totals: &mut GatheringTotals, kind: &str, delta: i64) {
        match kind {
            "wood" => totals.wood = (totals.wood + delta).max(0),
            "ore" => totals.ore = (totals.ore + delta).max(0),
            "fiber" => totals.fiber = (totals.fiber + delta).max(0),
            "hide" => totals.hide = (totals.hide + delta).max(0),
            "rock" => totals.rock = (totals.rock + delta).max(0),
            "fish" => totals.fish = (totals.fish + delta).max(0),
            _ => {}
        }
    }

    fn refresh_gathering_list(
        catalog: &Catalog,
        entries: &mut [GatheringEntry],
        totals: &mut GatheringTotals,
    ) {
        for g in entries.iter_mut() {
            if let Some(item) = catalog.item(g.item_index) {
                let prev_kind = g.resource_kind.clone();
                let new_kind = Self::classify_resource_kind(&item.unique_name);
                let amt = g.total_amount.max(0) as i64;
                if prev_kind != new_kind && amt > 0 {
                    Self::add_gathering_kind_amount(totals, &prev_kind, -amt);
                    Self::add_gathering_kind_amount(totals, new_kind, amt);
                }
                if g.unique_name.starts_with("ITEM_") && item.item_value > 0 && amt > 0 {
                    let new_est = amt * (item.item_value * 14);
                    totals.total_estimated_silver =
                        (totals.total_estimated_silver + new_est - g.estimated_silver).max(0);
                    g.estimated_silver = new_est;
                }
                g.unique_name = item.unique_name.clone();
                g.name_fr = item.name_fr.clone();
                g.name_en = item.name_en.clone();
                g.resource_kind = new_kind.to_string();
                g.tier = item.tier;
                g.enchantment = item.enchantment;
            }
        }
    }

    /// Refreshes cached cluster/item/weapon metadata across already-recorded entities & logs
    /// after the catalog finishes syncing or loading from cache.
    pub fn refresh_catalog_bindings(&mut self) {
        if !self.local_player.current_map_index.trim().is_empty() {
            if let Some(c) = self.catalog.cluster(&self.local_player.current_map_index) {
                if !c.name_fr.is_empty() {
                    self.local_player.current_map_name_fr = c.name_fr.clone();
                }
            }
        }

        for run in self
            .dungeons
            .iter_mut()
            .chain(self.live_session_dungeons.iter_mut())
        {
            if let Some(c) = self.catalog.cluster(&run.cluster_index) {
                if !c.name_fr.is_empty()
                    && (run.name_fr.starts_with("Zone ")
                        || run.name_fr == run.cluster_index
                        || run.name_fr == "Zone inconnue")
                {
                    run.name_fr = c.name_fr.clone();
                    run.name_en = c.name_en.clone();
                    run.kind = c.kind.clone();
                    run.tier = c.tier;
                }
            }
        }

        for zone in self.live_session_zones.iter_mut().chain(
            self.archived_sessions
                .iter_mut()
                .flat_map(|s| s.zones.iter_mut()),
        ) {
            if let Some(c) = self.catalog.cluster(&zone.cluster_index) {
                if !c.name_fr.is_empty()
                    && (zone.name_fr.starts_with("Zone ")
                        || zone.name_fr == zone.cluster_index
                        || zone.name_fr == "Zone inconnue")
                {
                    zone.name_fr = c.name_fr.clone();
                    zone.name_en = c.name_en.clone();
                    zone.kind = c.kind.clone();
                    zone.tier = c.tier;
                }
            }
        }

        Self::refresh_gathering_list(
            &self.catalog,
            &mut self.gathering_log,
            &mut self.gathering_totals,
        );
        Self::refresh_gathering_list(
            &self.catalog,
            &mut self.live_session_gathering_log,
            &mut self.live_session_gathering_totals,
        );

        for l in self
            .loot_log
            .iter_mut()
            .chain(self.live_session_loot_log.iter_mut())
        {
            if !l.is_silver {
                if let Some(item) = self.catalog.item(l.item_index) {
                    l.unique_name = item.unique_name.clone();
                    l.name_fr = item.name_fr.clone();
                    l.name_en = item.name_en.clone();
                }
            }
        }

        for ent in self
            .entities
            .values_mut()
            .chain(self.live_session_combat.values_mut())
        {
            if ent.weapon_specificity < 4 {
                let spell_keys: Vec<i32> = ent.spells.keys().copied().collect();
                if let Some((weapon, spec)) =
                    self.catalog.infer_best_weapon_from_spells(&spell_keys)
                {
                    if spec > ent.weapon_specificity {
                        ent.equipment.main_hand = weapon.index;
                        ent.weapon_specificity = spec;
                    }
                }
            }
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

    /// Resets live session counters (Fame, Silver, ReSpec, Might, Favor, Factions, Kills/Deaths)
    /// and live-session detail accumulators.
    pub fn reset_session_counters(&mut self, now_ms: u64) {
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
        self.live_session_zones.clear();
        self.live_session_combat.clear();
        self.live_session_dungeons.clear();
        self.live_session_gathering_totals = GatheringTotals::default();
        self.live_session_gathering_log.clear();
        self.live_session_loot_log.clear();
        self.live_session_trades.clear();
    }

    /// Resets session counters and purges live instances, gathering log, and loot history.
    pub fn reset_session(&mut self, now_ms: u64) {
        self.reset_session_counters(now_ms);
        self.dungeons.clear();
        self.opened_chests.clear();
        self.opened_caged_wisps.clear();
        self.gathering_totals = GatheringTotals::default();
        self.gathering_log.clear();
        self.loot_log.clear();
        self.market_orders.clear();
        self.mail_notifications.clear();
        self.trades.clear();
        self.dirty_history = true;
    }

    /// Clears all recorded dungeon, mist, and corrupted runs.
    pub fn clear_dungeons(&mut self) {
        self.dungeons.clear();
        self.live_session_dungeons.clear();
        self.opened_chests.clear();
        self.opened_caged_wisps.clear();
        self.dirty_history = true;
    }

    /// Clears the gathering totals and harvest event log.
    pub fn clear_gathering(&mut self) {
        self.gathering_totals = GatheringTotals::default();
        self.gathering_log.clear();
        self.live_session_gathering_totals = GatheringTotals::default();
        self.live_session_gathering_log.clear();
        self.dirty_history = true;
    }

    /// Clears the party loot history log.
    pub fn clear_loot(&mut self) {
        self.loot_log.clear();
        self.live_session_loot_log.clear();
        self.dirty_history = true;
    }

    /// Clears captured market orders, mail, and trade transactions.
    pub fn clear_market_data(&mut self) {
        self.market_orders.clear();
        self.mail_notifications.clear();
        self.trades.clear();
        self.live_session_trades.clear();
    }

    /// Purges all session data, combat meter, snapshots, instances, and history.
    pub fn clear_all_history(&mut self, now_ms: u64) {
        self.reset_meter();
        self.reset_session(now_ms);
        self.snapshots.clear();
        self.archived_sessions.clear();
        if self.local_player.username == "Aurelia" {
            self.local_player.username = "En attente de connexion…".into();
            self.local_player.object_id = 0;
            self.local_player.guild_name.clear();
            self.local_player.alliance_name.clear();
            self.local_player_guid = Guid16::default();
            self.awaiting_cluster_respawn = false;
        }
        self.dirty_history = true;
    }

    /// Thoroughly purges any simulation or mock records (Aurelia, Coffre des Brumes, mock dungeons/gathering)
    /// that might have lingered in persisted history from earlier test versions.
    pub fn purge_mock_data(&mut self) {
        let initial_dungeons = self.dungeons.len();
        let initial_loot = self.loot_log.len();
        let initial_gather = self.gathering_log.len();
        let initial_snaps = self.snapshots.len();
        let initial_archived = self.archived_sessions.len();

        let retain_dungeon = |d: &DungeonRun| {
            d.cluster_index != "MISTS_DUO_T8"
                && !d.name_fr.contains("Brumes Supérieures (Duo T8)")
                && !d.name_fr.contains("Brumes Supérieures")
        };
        let retain_loot = |l: &LootLogEntry| {
            l.looted_by != "Aurelia"
                && l.looted_from != "Coffre des Brumes"
                && !l.looted_by.contains("Aurelia")
        };
        let retain_gather = |g: &GatheringEntry| {
            !(g.item_index == 4011 || g.item_index == 4021)
                && !(g.name_fr.contains("Chanvre fantôme") && g.tier == 8)
                && !(g.name_fr.contains("Minerai de météorite") && g.tier == 7)
        };

        self.dungeons.retain(retain_dungeon);
        self.live_session_dungeons.retain(retain_dungeon);
        self.loot_log.retain(retain_loot);
        self.live_session_loot_log.retain(retain_loot);
        self.gathering_log.retain(retain_gather);
        self.live_session_gathering_log.retain(retain_gather);
        if self.gathering_log.is_empty() {
            self.gathering_totals = GatheringTotals::default();
        }
        if self.live_session_gathering_log.is_empty() {
            self.live_session_gathering_totals = GatheringTotals::default();
        }
        self.snapshots
            .retain(|s| s.label != "Boss Brumes" && !s.players.iter().any(|p| p.name == "Aurelia"));
        self.archived_sessions
            .retain(|a| a.label != "Soirée Brumes");
        if self.local_player.username == "Aurelia" {
            self.local_player.username = "En attente de connexion…".into();
            self.local_player.object_id = 0;
            self.local_player.guild_name.clear();
            self.local_player.alliance_name.clear();
            self.local_player_guid = Guid16::default();
            self.awaiting_cluster_respawn = false;
        }
        let is_real_ent = |ent: &InternalEntity| {
            ent.name != "Aurelia"
                && ent.name != "Seraphina"
                && ent.name != "Kaelen"
                && ent.name != "Thorne"
                && ent.name != "Vesper"
        };
        self.entities.retain(|_, ent| is_real_ent(ent));
        self.live_session_combat.retain(|_, ent| is_real_ent(ent));

        if self.dungeons.len() != initial_dungeons
            || self.loot_log.len() != initial_loot
            || self.gathering_log.len() != initial_gather
            || self.snapshots.len() != initial_snaps
            || self.archived_sessions.len() != initial_archived
        {
            self.dirty_history = true;
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

    pub fn has_session_activity(&self) -> bool {
        self.session.fame > 0.0
            || self.session.silver > 0.0
            || self.session.respec_points > 0.0
            || self.session.might > 0.0
            || self.session.favor > 0.0
            || self.session.kills > 0
            || self.session.deaths > 0
            || self.live_session_gathering_totals.total_nodes > 0
            || !self.live_session_combat.is_empty()
            || !self.live_session_dungeons.is_empty()
            || !self.live_session_loot_log.is_empty()
            || !self.live_session_trades.is_empty()
            || self
                .faction_map
                .values()
                .any(|&(c, r, f)| c > 0.0 || r > 0.0 || f > 0.0)
    }

    /// Archives the current live session (metrics, factions, zones, combats, dungeons, gathering, loot, trades)
    /// into `archived_sessions` (persisted to disk) and resets live session accumulators for the next session.
    pub fn archive_session(
        &mut self,
        custom_label: Option<String>,
        now_ms: u64,
    ) -> Option<ArchivedSession> {
        if !self.has_session_activity() {
            return None;
        }
        let ended_at_ms = self.last_now_ms.max(now_ms);
        let duration_seconds =
            ((ended_at_ms.saturating_sub(self.session_start_ms)) as f64 / 1000.0).max(1.0);
        let hours = duration_seconds / 3600.0;
        let id = format!("sess-{}", self.alloc_id());
        let label = custom_label
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| self.local_player.current_map_name_fr.clone());

        let factions = self.build_faction_summaries();
        let zones = self.finalize_live_zones_snapshot(ended_at_ms);
        let mut dungeons = self.live_session_dungeons.clone();
        if let Some(active) = dungeons.first_mut().filter(|d| d.is_active) {
            active.is_active = false;
            let elapsed_s =
                ((ended_at_ms.saturating_sub(active.started_at_ms)) as f64 / 1000.0).max(1.0);
            active.duration_seconds = elapsed_s;
            active.fame_per_hour = active.fame / (elapsed_s / 3600.0);
        }
        let (_, _, _, _, combat_players) = self.compute_live_session_combat_players();

        let archived = ArchivedSession {
            id,
            label,
            started_at_ms: self.session_start_ms,
            ended_at_ms,
            duration_seconds,
            map_name_fr: self.local_player.current_map_name_fr.clone(),
            fame: self.session.fame,
            fame_per_hour: self.session.fame / hours,
            silver: self.session.silver,
            silver_per_hour: self.session.silver / hours,
            respec_points: self.session.respec_points,
            might: self.session.might,
            favor: self.session.favor,
            kills: self.session.kills,
            deaths: self.session.deaths,
            zone_fame: self.session.zone_fame,
            satchel_fame: self.session.satchel_fame,
            respec_paid_silver: self.session.respec_paid_silver,
            guild_tax_paid: self.session.guild_tax_paid,
            cluster_tax_paid: self.session.cluster_tax_paid,
            repair_costs: self.session.repair_costs,
            season_points: self.session.season_points,
            factions,
            zones,
            combat_players,
            dungeons,
            gathering_totals: self.live_session_gathering_totals.clone(),
            gathering_log: self.live_session_gathering_log.clone(),
            loot_log: self.live_session_loot_log.clone(),
            trades: self.live_session_trades.clone(),
        };
        self.archived_sessions.insert(0, archived.clone());
        if self.archived_sessions.len() > MAX_ARCHIVED_SESSIONS {
            self.archived_sessions.truncate(MAX_ARCHIVED_SESSIONS);
        }
        self.reset_session_counters(ended_at_ms);
        self.dirty_history = true;
        Some(archived)
    }

    pub fn delete_archived_session(&mut self, id: &str) -> bool {
        let before = self.archived_sessions.len();
        self.archived_sessions.retain(|s| s.id != id);
        let deleted = self.archived_sessions.len() != before;
        if deleted {
            self.dirty_history = true;
        }
        deleted
    }

    /// Exports persisted history (snapshots, archived sessions, dungeons, gathering, loot) for disk storage.
    pub fn export_history(&self) -> PersistedHistory {
        PersistedHistory {
            snapshots: self.snapshots.clone(),
            archived_sessions: self.archived_sessions.clone(),
            dungeons: self.dungeons.clone(),
            gathering_totals: self.gathering_totals.clone(),
            gathering_log: self.gathering_log.clone(),
            loot_log: self.loot_log.clone(),
        }
    }

    /// Restores persisted history loaded from disk at startup.
    pub fn restore_history(&mut self, mut history: PersistedHistory) {
        history.snapshots.truncate(MAX_SNAPSHOTS);
        history.archived_sessions.truncate(MAX_ARCHIVED_SESSIONS);
        history.dungeons.truncate(MAX_DUNGEON_RUNS);
        history.gathering_log.truncate(MAX_GATHERING_LOG);
        history.loot_log.truncate(MAX_LOOT_LOG);
        for d in &mut history.dungeons {
            d.is_active = false;
        }
        for s in &mut history.archived_sessions {
            for d in &mut s.dungeons {
                d.is_active = false;
            }
        }
        self.snapshots = history.snapshots;
        self.archived_sessions = history.archived_sessions;
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

    fn build_faction_summaries(&self) -> Vec<FactionCitySummary> {
        self.faction_map
            .iter()
            .map(|(city, &(coins, rank_pts, flag_pts))| FactionCitySummary {
                city: city.label_fr().to_string(),
                coins,
                rank_points: rank_pts,
                flag_points: flag_pts,
            })
            .collect()
    }

    fn finalize_live_zones_snapshot(&self, current_ms: u64) -> Vec<SessionZoneSummary> {
        let mut zones = self.live_session_zones.clone();
        if let Some(active) = zones.first_mut() {
            if active.duration_seconds <= 0.0 {
                active.duration_seconds =
                    ((current_ms.saturating_sub(active.entered_at_ms)) as f64 / 1000.0).max(1.0);
            }
        }
        zones
    }

    fn compute_live_session_combat_players(&self) -> (f64, i64, i64, i64, Vec<DamageMeterPlayer>) {
        let min_s = self
            .live_session_combat
            .values()
            .filter_map(|e| e.first_combat_ms)
            .min();
        let max_e = self
            .live_session_combat
            .values()
            .filter_map(|e| e.last_combat_ms)
            .max();
        let fallback_secs = match (min_s, max_e) {
            (Some(s), Some(e)) => ((e.saturating_sub(s) as f64) / 1000.0).max(1.0),
            _ => 0.0,
        };
        self.compute_meter_players_from(self.live_session_combat.values(), fallback_secs)
    }

    fn compute_meter_players(&self) -> (f64, i64, i64, i64, Vec<DamageMeterPlayer>) {
        let global_combat_secs = match (self.combat_start_ms, self.combat_end_ms) {
            (Some(s), Some(e)) => ((e.saturating_sub(s) as f64) / 1000.0).max(1.0),
            _ => 0.0,
        };
        self.compute_meter_players_from(self.entities.values(), global_combat_secs)
    }

    fn compute_meter_players_from<'a, I>(
        &self,
        entities_iter: I,
        global_combat_secs: f64,
    ) -> (f64, i64, i64, i64, Vec<DamageMeterPlayer>)
    where
        I: Iterator<Item = &'a InternalEntity> + Clone,
    {
        let mut total_damage = 0i64;
        let mut total_heal = 0i64;
        let mut total_taken = 0i64;

        for ent in entities_iter.clone() {
            if !ent.is_party && !ent.is_self {
                continue;
            }
            total_damage += ent.damage;
            total_heal += ent.heal;
            total_taken += ent.taken_damage;
        }

        let mut players = Vec::new();
        for ent in entities_iter {
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
                    let ui_sprite = meta.and_then(|s| s.ui_sprite.clone());
                    let combined = (acc.damage + acc.heal) as f64;
                    SpellStat {
                        spell_index: sp_idx,
                        unique_name,
                        name_fr,
                        name_en,
                        category,
                        ui_sprite,
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

        let factions = self.build_faction_summaries();

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

        let live_session_zones = self.finalize_live_zones_snapshot(current_ms);
        let mut live_session_dungeons = self.live_session_dungeons.clone();
        if let Some(active) = live_session_dungeons.first_mut().filter(|d| d.is_active) {
            let d_secs =
                ((current_ms.saturating_sub(active.started_at_ms)) as f64 / 1000.0).max(1.0);
            active.duration_seconds = d_secs;
            active.fame_per_hour = active.fame / (d_secs / 3600.0);
        }
        let (_, _, _, _, live_session_combat_players) = self.compute_live_session_combat_players();

        let mut inspected_players = Vec::new();
        for ent in self.entities.values() {
            if !ent.is_party && !ent.is_self {
                continue;
            }
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
                } else {
                    slots.push(EquippedSlotView {
                        slot: slot_name.into(),
                        item_index: item_idx,
                        unique_name: format!("ITEM_{item_idx}"),
                        name_fr: format!("Objet #{item_idx}"),
                        name_en: format!("Item #{item_idx}"),
                        tier: 0,
                        enchantment: 0,
                        item_value: 0,
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
        inspected_players.sort_by_key(|p| (!p.is_self, p.object_id));

        AppSnapshot {
            catalog_version: self.catalog.version.clone(),
            catalog_item_count: self.catalog.item_count(),
            catalog_spell_count: self.catalog.spell_count(),
            catalog_recipe_count: self.catalog.recipe_count(),
            catalog_cluster_count: self.catalog.cluster_count(),
            catalog_weapon_spell_count: self.catalog.weapon_spell_count(),
            local_player: self.local_player.clone(),
            combat_seconds,
            total_party_damage,
            total_party_heal,
            total_party_taken,
            meter_players,
            snapshots: self.snapshots.clone(),
            session,
            archived_sessions: self.archived_sessions.clone(),
            dungeons,
            gathering_totals: self.gathering_totals.clone(),
            gathering_log: self.gathering_log.clone(),
            loot_log: self.loot_log.clone(),
            inspected_players,
            market_orders: self.market_orders.clone(),
            mail_notifications: self.mail_notifications.clone(),
            trades: self.trades.clone(),
            live_session_zones,
            live_session_combat_players,
            live_session_dungeons,
            live_session_gathering_totals: self.live_session_gathering_totals.clone(),
            live_session_gathering_log: self.live_session_gathering_log.clone(),
            live_session_loot_log: self.live_session_loot_log.clone(),
            live_session_trades: self.live_session_trades.clone(),
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
                AlbionTelemetryEvent::MistsWispCageOpened { object_id: 7002 },
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
        let saved_sess = engine.archive_session(Some("Soirée Brumes".into()), 25_000);
        assert!(saved_sess.is_some());
        assert!(!engine.has_session_activity());

        let exported = engine.export_history();
        assert_eq!(exported.snapshots.len(), 1);
        assert_eq!(exported.archived_sessions.len(), 1);
        assert_eq!(exported.dungeons.len(), 1);
        assert_eq!(exported.loot_log.len(), 1);

        let mut fresh = AnalyzerEngine::new(catalog, 50_000);
        fresh.restore_history(exported);
        let restored = fresh.snapshot(50_000);
        assert_eq!(restored.snapshots.len(), 1);
        assert_eq!(restored.snapshots[0].label, "Boss Brumes");
        assert_eq!(restored.archived_sessions.len(), 1);
        assert_eq!(restored.archived_sessions[0].label, "Soirée Brumes");
        assert!(restored.archived_sessions[0].fame > 0.0);
        assert_eq!(restored.dungeons.len(), 1);
        assert!(!restored.dungeons[0].is_active);
        assert_eq!(restored.loot_log.len(), 1);

        let sess_id = restored.archived_sessions[0].id.clone();
        assert!(fresh.delete_archived_session(&sess_id));
        assert!(fresh.snapshot(50_000).archived_sessions.is_empty());
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

    #[test]
    fn enforces_strict_party_isolation_and_ignores_enemy_or_radar_events() {
        use crate::photon::PhotonMessage;
        use crate::protocol16::{EventDataPayload, ParameterTable, Protocol16Value};

        let catalog = Catalog::from_json_str(EMBEDDED).unwrap();
        let mut engine = AnalyzerEngine::new(catalog, 1_000);

        let g_self = Guid16([0xAA; 16]);
        let g_enemy = Guid16([0xBB; 16]);
        let g_ally = Guid16([0xCC; 16]);

        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::JoinResponse {
                user_object_id: 101,
                user_guid: g_self,
                username: "Aurelia".into(),
                map_index: "3005".into(),
                main_map_index: "3005".into(),
                guild_name: "Aegis".into(),
                alliance_name: String::new(),
                current_focus_points: 10_000.0,
                max_focus_points: 30_000.0,
                silver: FixPoint::from_f64(1_000_000.0),
                gold: FixPoint::from_f64(0.0),
                learning_points: FixPoint::from_f64(0.0),
                reputation: 0.0,
                is_respec_active: false,
            },
            1_100,
        );
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::ChangeClusterResponse {
                index: "MISTS_SOLO_T8".into(),
                world_map_data_type: "mists".into(),
                island_name: String::new(),
                mists_dungeon_tier: 8,
            },
            1_200,
        );

        // Nearby hostile/third-party player packets arriving over UDP
        let third_party_events = [
            AlbionTelemetryEvent::NewCharacter {
                object_id: 999,
                name: "GankerX".into(),
                guid: g_enemy,
                guild_name: "Hostile".into(),
                equipment: CharacterEquipment {
                    main_hand: 1004,
                    chest: 1302,
                    potion: 1801,
                    buff_food: 1901,
                    ..CharacterEquipment::default()
                },
            },
            AlbionTelemetryEvent::ActiveSpellEffectsUpdate {
                causer_id: 888, // Off-screen unknown caster
                spell_indices: vec![2604],
            },
            AlbionTelemetryEvent::HealthUpdate(HealthTick {
                affected_object_id: 5000,
                timestamp: 1,
                health_change: -1_500.0,
                new_health_value: 3_500.0,
                effect_type: EffectType::Physical,
                effect_origin: EffectOrigin::Direct,
                causer_id: 999,
                causing_spell_index: 2404,
            }),
            AlbionTelemetryEvent::HarvestFinished {
                user_object_id: 999,
                harvest_object_id: 9001,
                item_id: 4011,
                standard_amount: 4,
                collector_bonus_amount: 1,
                premium_bonus_amount: 1,
            },
            AlbionTelemetryEvent::TakeSilver {
                object_id: 999,
                target_entity_id: 5000,
                yield_pre_tax: FixPoint::from_f64(25_000.0),
                guild_tax: FixPoint::from_f64(0.0),
                cluster_tax: FixPoint::from_f64(0.0),
                is_premium_bonus: false,
            },
            AlbionTelemetryEvent::UpdateFame {
                object_id: 999,
                total_player_fame: FixPoint::from_f64(100_000.0),
                fame_with_zone_multiplier: FixPoint::from_f64(15_000.0),
                zone_fame: FixPoint::from_f64(10_000.0),
                is_premium_bonus: false,
                satchel_fame: FixPoint::from_f64(0.0),
                bonus_factor: 1.0,
            },
            AlbionTelemetryEvent::OtherGrabbedLoot {
                object_id: 8001,
                looted_from_name: "Coffre".into(),
                looted_by_name: "GankerX".into(),
                is_silver: false,
                item_index: 1004,
                quantity: 1,
            },
        ];
        for (idx, ev) in third_party_events.into_iter().enumerate() {
            engine.ingest_telemetry_event(ev, 2_000 + (idx as u64) * 100);
        }

        // Raw EV_NEW_MISTS_CAGED_WISP (500) and Move (code 3) packets must be ignored at decode
        let mut wisp_spawn_params = ParameterTable::new();
        wisp_spawn_params.insert(0, Protocol16Value::Long(7777));
        wisp_spawn_params.insert(252, Protocol16Value::Short(500));
        let wisp_spawn_msg = PhotonMessage::EventData(EventDataPayload {
            code: 1,
            parameters: wisp_spawn_params,
        });
        let move_msg = PhotonMessage::EventData(EventDataPayload {
            code: 3,
            parameters: ParameterTable::new(),
        });
        let raw_pkt = build_photon_udp_packet(&[wisp_spawn_msg, move_msg], true, false, 77);
        assert_eq!(engine.ingest_udp_payload(&raw_pkt, 3_000), 0);

        let snap = engine.snapshot(3_500);
        assert_eq!(snap.inspected_players.len(), 1);
        assert_eq!(snap.inspected_players[0].name, "Aurelia");
        assert!(snap.meter_players.is_empty());
        assert!(snap.gathering_log.is_empty());
        assert_eq!(snap.gathering_totals.total_nodes, 0);
        assert_eq!(snap.session.silver, 0.0);
        assert_eq!(snap.session.fame, 0.0);
        assert!(snap.loot_log.is_empty());
        assert_eq!(snap.dungeons[0].caged_wisps_opened, 0);

        // Nearby player joins party -> becomes visible in Party Inspect, then leaves -> hidden again
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::NewCharacter {
                object_id: 202,
                name: "FriendlyAlly".into(),
                guid: g_ally,
                guild_name: "Aegis".into(),
                equipment: CharacterEquipment {
                    main_hand: 1061,
                    ..CharacterEquipment::default()
                },
            },
            4_000,
        );
        assert_eq!(engine.snapshot(4_100).inspected_players.len(), 1);

        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::PartyPlayerJoined {
                party_object_id: 900,
                user_guid: g_ally,
                username: "FriendlyAlly".into(),
            },
            4_200,
        );
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::OtherGrabbedLoot {
                object_id: 8002,
                looted_from_name: "Coffre".into(),
                looted_by_name: "FriendlyAlly".into(),
                is_silver: false,
                item_index: 1004,
                quantity: 1,
            },
            4_300,
        );
        let snap_grouped = engine.snapshot(4_400);
        assert_eq!(snap_grouped.inspected_players.len(), 2);
        assert!(snap_grouped
            .inspected_players
            .iter()
            .any(|p| p.name == "FriendlyAlly"));
        assert_eq!(snap_grouped.loot_log.len(), 1);

        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::PartyPlayerLeft {
                party_object_id: 900,
                user_guid: g_ally,
            },
            4_500,
        );
        assert_eq!(engine.snapshot(4_600).inspected_players.len(), 1);
    }

    #[test]
    fn purges_mock_and_session_data_and_infers_mid_session_character() {
        let catalog = Catalog::from_json_str(EMBEDDED).unwrap();
        let mut engine = AnalyzerEngine::new(catalog, 1_000);

        // 1. Ingest simulation scenario (which generates Aurelia, dungeons, harvest, loot)
        engine.run_wire_simulation_scenario(1_000);
        let snap = engine.snapshot(2_000);
        assert!(!snap.dungeons.is_empty());
        assert!(!snap.gathering_log.is_empty());
        assert!(!snap.loot_log.is_empty());
        assert!(snap.session.fame > 0.0);

        // 2. Individual clear operations
        engine.clear_dungeons();
        assert!(engine.snapshot(2_100).dungeons.is_empty());

        engine.clear_gathering();
        let snap_g = engine.snapshot(2_200);
        assert!(snap_g.gathering_log.is_empty());
        assert_eq!(snap_g.gathering_totals.total_nodes, 0);

        engine.clear_loot();
        assert!(engine.snapshot(2_300).loot_log.is_empty());

        // 3. Clear all history completely
        engine.clear_all_history(3_000);
        let snap_empty = engine.snapshot(3_100);
        assert_eq!(snap_empty.session.fame, 0.0);
        assert_eq!(snap_empty.session.silver, 0.0);
        assert!(snap_empty.dungeons.is_empty());
        assert!(snap_empty.gathering_log.is_empty());
        assert!(snap_empty.loot_log.is_empty());

        // 4. Mid-session character adoption without JoinResponse
        let mut fresh = AnalyzerEngine::new(Catalog::from_json_str(EMBEDDED).unwrap(), 4_000);
        assert_eq!(fresh.local_player.object_id, 0);
        fresh.ingest_telemetry_event(
            AlbionTelemetryEvent::InCombatStateUpdate {
                object_id: 4242,
                in_active_combat: true,
                in_passive_combat: false,
            },
            4_000,
        );
        assert_eq!(fresh.local_player.object_id, 4242);

        // Combat tick from adopted causer is recorded
        fresh.ingest_telemetry_event(
            AlbionTelemetryEvent::HealthUpdate(HealthTick {
                affected_object_id: 8888,
                timestamp: 4_050,
                health_change: -850.0,
                new_health_value: 2_000.0,
                effect_type: EffectType::Physical,
                effect_origin: EffectOrigin::Direct,
                causer_id: 4242,
                causing_spell_index: 2404,
            }),
            4_050,
        );
        let snap_combat = fresh.snapshot(4_100);
        assert_eq!(snap_combat.total_party_damage, 850);
        assert_eq!(snap_combat.meter_players.len(), 1);
        assert!(snap_combat.meter_players[0].is_self);

        // Harvesting without prior JoinResponse adopts local character
        let mut harvest_engine =
            AnalyzerEngine::new(Catalog::from_json_str(EMBEDDED).unwrap(), 5_000);
        assert_eq!(harvest_engine.local_player.object_id, 0);
        harvest_engine.ingest_telemetry_event(
            AlbionTelemetryEvent::HarvestFinished {
                user_object_id: 5555,
                harvest_object_id: 9999,
                item_id: 4011, // Meteorite Ore T7.2
                standard_amount: 3,
                collector_bonus_amount: 0,
                premium_bonus_amount: 1,
            },
            5_050,
        );
        assert_eq!(harvest_engine.local_player.object_id, 5555);
        assert_eq!(harvest_engine.snapshot(5_100).gathering_totals.ore, 4);
    }

    #[test]
    fn resolves_dynamic_instance_cluster_ids_and_tier_hints() {
        let catalog = Catalog::from_json_str(EMBEDDED).unwrap();
        let mut engine = AnalyzerEngine::new(catalog, 1_000);

        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::JoinResponse {
                user_object_id: 10,
                user_guid: Guid16([0x11; 16]),
                username: "Scout".into(),
                map_index: "@MISTS@5581349013".into(),
                main_map_index: "@MISTS@5581349013".into(),
                guild_name: String::new(),
                alliance_name: String::new(),
                current_focus_points: 0.0,
                max_focus_points: 30_000.0,
                silver: FixPoint::from_f64(0.0),
                gold: FixPoint::from_f64(0.0),
                learning_points: FixPoint::from_f64(0.0),
                reputation: 0.0,
                is_respec_active: false,
            },
            1_100,
        );
        assert_eq!(
            engine.snapshot(1_150).local_player.current_map_name_fr,
            "Brumes"
        );

        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::ChangeClusterResponse {
                index: "@MISTS@5581349013".into(),
                world_map_data_type: "T8_MISTS_DEFAULT".into(),
                island_name: String::new(),
                mists_dungeon_tier: 0,
            },
            1_200,
        );
        let snap_mists = engine.snapshot(1_250);
        assert_eq!(snap_mists.dungeons.len(), 1);
        assert_eq!(snap_mists.dungeons[0].kind, "mists");
        assert_eq!(snap_mists.dungeons[0].tier, 8);
        assert_eq!(snap_mists.dungeons[0].name_fr, "Brumes");
        assert!(snap_mists.dungeons[0].is_active);

        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::JoinResponse {
                user_object_id: 10,
                user_guid: Guid16([0x11; 16]),
                username: "Scout".into(),
                map_index: "@RANDOMDUNGEON@999".into(),
                main_map_index: String::new(),
                guild_name: String::new(),
                alliance_name: String::new(),
                current_focus_points: 0.0,
                max_focus_points: 30_000.0,
                silver: FixPoint::from_f64(0.0),
                gold: FixPoint::from_f64(0.0),
                learning_points: FixPoint::from_f64(0.0),
                reputation: 0.0,
                is_respec_active: false,
            },
            1_300,
        );
        assert_eq!(
            engine.snapshot(1_350).local_player.current_map_name_fr,
            "Donjon aléatoire"
        );

        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::ChangeClusterResponse {
                index: "@RANDOMDUNGEON@123456".into(),
                world_map_data_type: "HIGHLAND_RD_GROUP_T7".into(),
                island_name: String::new(),
                mists_dungeon_tier: 0,
            },
            1_400,
        );
        let snap_rd = engine.snapshot(1_450);
        assert_eq!(snap_rd.dungeons.len(), 2);
        assert_eq!(snap_rd.dungeons[0].kind, "groupDungeon");
        assert_eq!(snap_rd.dungeons[0].tier, 7);
        assert_eq!(snap_rd.dungeons[0].name_fr, "Donjon de Groupe");
        assert!(!snap_rd.dungeons[1].is_active);
    }

    #[test]
    fn deduplicates_chest_and_wisp_updates_and_clears_sets() {
        let catalog = Catalog::from_json_str(EMBEDDED).unwrap();
        let mut engine = AnalyzerEngine::new(catalog, 1_000);

        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::ChangeClusterResponse {
                index: "@MISTS@1001".into(),
                world_map_data_type: "T8_MISTS_DEFAULT".into(),
                island_name: String::new(),
                mists_dungeon_tier: 8,
            },
            1_100,
        );
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::LootChestOpened { object_id: 7001 },
            1_200,
        );
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::LootChestOpened { object_id: 7001 },
            1_250,
        );
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::LootChestOpened { object_id: 7002 },
            1_300,
        );
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::MistsWispCageOpened { object_id: 8001 },
            1_350,
        );
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::MistsWispCageOpened { object_id: 8001 },
            1_400,
        );

        let snap1 = engine.snapshot(1_450);
        assert_eq!(snap1.dungeons[0].chests_opened, 2);
        assert_eq!(snap1.dungeons[0].caged_wisps_opened, 1);

        // Switching cluster clears deduplication sets so identical object IDs can count in the new run
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::ChangeClusterResponse {
                index: "@MISTS@1002".into(),
                world_map_data_type: "T8_MISTS_DEFAULT".into(),
                island_name: String::new(),
                mists_dungeon_tier: 8,
            },
            2_000,
        );
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::LootChestOpened { object_id: 7001 },
            2_100,
        );
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::MistsWispCageOpened { object_id: 8001 },
            2_150,
        );

        let snap2 = engine.snapshot(2_200);
        assert_eq!(snap2.dungeons[0].chests_opened, 1);
        assert_eq!(snap2.dungeons[0].caged_wisps_opened, 1);

        engine.clear_dungeons();
        assert!(engine.opened_chests.is_empty());
        assert!(engine.opened_caged_wisps.is_empty());

        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::ChangeClusterResponse {
                index: "@MISTS@1003".into(),
                world_map_data_type: "T8_MISTS_DEFAULT".into(),
                island_name: String::new(),
                mists_dungeon_tier: 8,
            },
            3_000,
        );
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::LootChestOpened { object_id: 7001 },
            3_100,
        );
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::MistsWispCageOpened { object_id: 8001 },
            3_150,
        );
        engine.reset_session(3_200);
        assert!(engine.opened_chests.is_empty());
        assert!(engine.opened_caged_wisps.is_empty());
    }

    #[test]
    fn parses_live_and_legacy_market_mail_formats() {
        let catalog = Catalog::from_json_str(EMBEDDED).unwrap();
        let mut engine = AnalyzerEngine::new(catalog, 1_000);

        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::ReadMailResponse {
                mail_id: 901,
                content: "2|T7_2H_CLEAVER_HELL@1|8900000000|4450000000".into(),
            },
            1_100,
        );
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::ReadMailResponse {
                mail_id: 902,
                content: "4450000000|4450000000|1|T7_2H_CLEAVER_HELL@1".into(),
            },
            1_200,
        );

        let snap = engine.snapshot(1_300);
        assert_eq!(snap.mail_notifications.len(), 2);

        let legacy = &snap.mail_notifications[0];
        assert_eq!(legacy.mail_id, 902);
        assert_eq!(legacy.item_unique_name, "T7_2H_CLEAVER_HELL@1");
        assert_eq!(legacy.amount, 1);
        assert!((legacy.unit_price_silver - 445_000.0).abs() < 0.01);
        assert!((legacy.total_price_silver - 445_000.0).abs() < 0.01);

        let live = &snap.mail_notifications[1];
        assert_eq!(live.mail_id, 901);
        assert_eq!(live.item_unique_name, "T7_2H_CLEAVER_HELL@1");
        assert_eq!(live.amount, 2);
        assert!((live.unit_price_silver - 445_000.0).abs() < 0.01);
        assert!((live.total_price_silver - 890_000.0).abs() < 0.01);
    }

    #[test]
    fn handles_cluster_transition_guid_migration_and_avoids_duplication() {
        let catalog = Catalog::from_json_str(EMBEDDED).unwrap();
        let mut engine = AnalyzerEngine::new(catalog, 1_000);
        let my_guid = Guid16([0x42; 16]);

        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::JoinResponse {
                user_object_id: 101,
                user_guid: my_guid,
                username: "Zainados".into(),
                map_index: "0200".into(),
                main_map_index: "0200".into(),
                guild_name: "Instinct".into(),
                alliance_name: String::new(),
                current_focus_points: 10_000.0,
                max_focus_points: 30_000.0,
                silver: FixPoint::from_f64(50_000.0),
                gold: FixPoint::from_f64(0.0),
                learning_points: FixPoint::from_f64(0.0),
                reputation: 0.0,
                is_respec_active: false,
            },
            1_100,
        );
        // Placeholder `"PA"` spawn name must never overwrite real `"Zainados"`
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::NewCharacter {
                object_id: 101,
                name: "PA".into(),
                guid: my_guid,
                guild_name: "Instinct".into(),
                equipment: CharacterEquipment {
                    main_hand: 1003,
                    ..Default::default()
                },
            },
            1_150,
        );
        assert_eq!(engine.snapshot(1_160).local_player.username, "Zainados");

        // Cluster transition clears non-party transient state and awaits new object_id
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::ChangeClusterResponse {
                index: "0203".into(),
                world_map_data_type: "SWAMP".into(),
                island_name: String::new(),
                mists_dungeon_tier: 0,
            },
            2_000,
        );

        // Equipment event can arrive slightly before NewCharacter on respawn
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::CharacterEquipmentChanged {
                object_id: 456,
                equipment: CharacterEquipment {
                    off_hand: 1102,
                    ..Default::default()
                },
                equipped_spells: vec![2104], // Infers 1003 (`T8_2H_DUALSWORD@2`)
            },
            2_050,
        );

        // NewCharacter spawns local player with new object_id=456, same GUID, and placeholder name `"PA"`
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::NewCharacter {
                object_id: 456,
                name: "PA".into(),
                guid: my_guid,
                guild_name: "Instinct".into(),
                equipment: CharacterEquipment::default(),
            },
            2_100,
        );

        assert_eq!(engine.local_player.object_id, 456);
        let snap1 = engine.snapshot(2_150);
        assert_eq!(snap1.local_player.username, "Zainados");
        assert_eq!(snap1.inspected_players.len(), 1);
        assert_eq!(snap1.inspected_players[0].object_id, 456);
        assert_eq!(snap1.inspected_players[0].name, "Zainados");
        assert!(snap1.inspected_players[0]
            .slots
            .iter()
            .any(|s| s.slot == "mainHand" && s.item_index == 1003));

        // Post-zone HarvestFinished, UpdateFame, TakeSilver on new_id=456 all count normally
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::HarvestFinished {
                user_object_id: 456,
                harvest_object_id: 9001,
                item_id: 4011,
                standard_amount: 3,
                collector_bonus_amount: 1,
                premium_bonus_amount: 1,
            },
            2_200,
        );
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::UpdateFame {
                object_id: 456,
                total_player_fame: FixPoint::from_f64(100_000.0),
                fame_with_zone_multiplier: FixPoint::from_f64(1_250.0),
                zone_fame: FixPoint::from_f64(250.0),
                is_premium_bonus: true,
                satchel_fame: FixPoint::from_f64(0.0),
                bonus_factor: 1.0,
            },
            2_250,
        );
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::TakeSilver {
                object_id: 456,
                target_entity_id: 9002,
                yield_pre_tax: FixPoint::from_f64(800.0),
                guild_tax: FixPoint::from_f64(80.0),
                cluster_tax: FixPoint::from_f64(0.0),
                is_premium_bonus: false,
            },
            2_300,
        );

        let snap2 = engine.snapshot(2_350);
        assert_eq!(snap2.gathering_totals.ore, 5);
        assert!((snap2.session.fame - 1_250.0).abs() < 0.01);
        assert!((snap2.session.silver - 720.0).abs() < 0.01);
        assert_eq!(snap2.live_session_zones[0].cluster_index, "0203");
        assert_eq!(snap2.live_session_zones[0].gathering_resources, 5);

        // Next zone change where UpdateMoney arrives before NewCharacter also migrates 456 -> 789
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::ChangeClusterResponse {
                index: "0205".into(),
                world_map_data_type: "SWAMP".into(),
                island_name: String::new(),
                mists_dungeon_tier: 0,
            },
            3_000,
        );
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::UpdateMoney {
                object_id: 789,
                current_player_silver: FixPoint::from_f64(51_000.0),
            },
            3_100,
        );
        assert_eq!(engine.local_player.object_id, 789);
        assert_eq!(engine.snapshot(3_120).inspected_players.len(), 1);
    }

    #[test]
    fn upgrades_weapon_specificity_and_refreshes_catalog_bindings() {
        let mut catalog = Catalog::from_json_str(EMBEDDED).unwrap();
        let mut engine = AnalyzerEngine::new(catalog.clone(), 1_000);
        let my_guid = Guid16([0x55; 16]);

        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::JoinResponse {
                user_object_id: 10,
                user_guid: my_guid,
                username: "Fighter".into(),
                map_index: "0203".into(),
                main_map_index: "0203".into(),
                guild_name: String::new(),
                alliance_name: String::new(),
                current_focus_points: 0.0,
                max_focus_points: 30_000.0,
                silver: FixPoint::from_f64(0.0),
                gold: FixPoint::from_f64(0.0),
                learning_points: FixPoint::from_f64(0.0),
                reputation: 0.0,
                is_respec_active: false,
            },
            1_050,
        );
        // Before world.json is merged, unknown cluster 0203 falls back to "Zone 0203"
        assert_eq!(
            engine.snapshot(1_060).local_player.current_map_name_fr,
            "Zone 0203"
        );

        // Shared sword Q spell (2101) gives specificity 2 (not overriding with another generic spell)
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::ActiveSpellEffectsUpdate {
                causer_id: 10,
                spell_indices: vec![2101],
            },
            1_100,
        );
        // Signature Dual Swords E spell (2104, specificity 3) upgrades main_hand to 1003 (`T8_2H_DUALSWORD@2`)
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::ActiveSpellEffectsUpdate {
                causer_id: 10,
                spell_indices: vec![2104],
            },
            1_150,
        );
        // Subsequent shared Q spell (2101, specificity 2) does not downgrade the weapon
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::ActiveSpellEffectsUpdate {
                causer_id: 10,
                spell_indices: vec![2101],
            },
            1_200,
        );
        let snap = engine.snapshot(1_250);
        assert_eq!(
            snap.inspected_players[0]
                .slots
                .iter()
                .find(|s| s.slot == "mainHand")
                .map(|s| s.item_index),
            Some(1003)
        );

        // Uncatalogued resource (item 8888) falls back to ITEM_8888 before catalog import
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::HarvestFinished {
                user_object_id: 10,
                harvest_object_id: 7777,
                item_id: 8888,
                standard_amount: 2,
                collector_bonus_amount: 0,
                premium_bonus_amount: 0,
            },
            1_300,
        );
        assert_eq!(
            engine.snapshot(1_320).gathering_log[0].unique_name,
            "ITEM_8888"
        );

        // Now simulate catalog enrichment via ao-bin-dumps + refresh_catalog_bindings()
        catalog
            .merge_ao_bin_dumps_world(r#"[{"Index":"0203","UniqueName":"Willowsigh Marsh"}]"#)
            .unwrap();
        catalog
            .merge_ao_bin_dumps_items(
                r#"[{"Index":"8888","UniqueName":"T5_WOOD","LocalizedNames":{"FR-FR":"Bûches de pin","EN-US":"Pine Logs"}}]"#,
            )
            .unwrap();
        engine.catalog = catalog;
        engine.refresh_catalog_bindings();

        let healed = engine.snapshot(1_400);
        assert_eq!(healed.local_player.current_map_name_fr, "Willowsigh Marsh");
        assert_eq!(healed.live_session_zones[0].name_fr, "Willowsigh Marsh");
        assert_eq!(healed.gathering_log[0].unique_name, "T5_WOOD");
        assert_eq!(healed.gathering_log[0].name_fr, "Bûches de pin");
        assert_eq!(healed.gathering_log[0].resource_kind, "wood");
        assert_eq!(healed.gathering_totals.wood, 2);
    }

    #[test]
    fn records_scaled_silver_loot_without_ground_item_collision() {
        let catalog = Catalog::from_json_str(EMBEDDED).unwrap();
        let mut engine = AnalyzerEngine::new(catalog, 1_000);

        // Discovered item shares object_id=500 with a later Silver pickup
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::NewDiscoveredItem {
                object_id: 500,
                item_id: 1003,
                quantity: 1,
                estimated_market_value: 250_000,
                quality_level: 2,
            },
            1_100,
        );
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::OtherGrabbedLoot {
                object_id: 500,
                looted_from_name: "@MOB".into(),
                looted_by_name: "Zainados".into(),
                is_silver: true,
                item_index: 0,
                quantity: 325,
            },
            1_200,
        );
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::OtherGrabbedLoot {
                object_id: 501,
                looted_from_name: "@MOB".into(),
                looted_by_name: "Zainados".into(),
                is_silver: true,
                item_index: 0,
                quantity: 358,
            },
            1_300,
        );
        // An actual item with colliding quantity=325 and index=0 -> resolves to discovered_items[500] (1003) and is NOT deduped against Silver!
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::OtherGrabbedLoot {
                object_id: 500,
                looted_from_name: "@MOB".into(),
                looted_by_name: "Zainados".into(),
                is_silver: false,
                item_index: 0,
                quantity: 325,
            },
            1_400,
        );

        let snap = engine.snapshot(1_500);
        assert_eq!(snap.loot_log.len(), 3);
        assert_eq!(snap.loot_log[0].item_index, 1003);
        assert!(!snap.loot_log[0].is_silver);
        assert_eq!(snap.loot_log[1].unique_name, "SILVER");
        assert_eq!(snap.loot_log[1].quantity, 358);
        assert!(snap.loot_log[1].is_silver);
        assert_eq!(snap.loot_log[2].unique_name, "SILVER");
        assert_eq!(snap.loot_log[2].quantity, 325);
        assert!(snap.loot_log[2].is_silver);
    }

    #[test]
    fn archives_rich_session_details_and_deserializes_legacy_archives() {
        let catalog = Catalog::from_json_str(EMBEDDED).unwrap();
        let mut engine = AnalyzerEngine::new(catalog, 1_000);
        let my_guid = Guid16([0x77; 16]);

        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::JoinResponse {
                user_object_id: 10,
                user_guid: my_guid,
                username: "Explorer".into(),
                map_index: "0200".into(),
                main_map_index: "0200".into(),
                guild_name: "Instinct".into(),
                alliance_name: String::new(),
                current_focus_points: 5_000.0,
                max_focus_points: 30_000.0,
                silver: FixPoint::from_f64(10_000.0),
                gold: FixPoint::from_f64(0.0),
                learning_points: FixPoint::from_f64(0.0),
                reputation: 0.0,
                is_respec_active: false,
            },
            1_100,
        );
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::ChangeClusterResponse {
                index: "@MISTS@500".into(),
                world_map_data_type: "T7_MISTS_DEFAULT".into(),
                island_name: String::new(),
                mists_dungeon_tier: 7,
            },
            2_000,
        );
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::HealthUpdate(HealthTick {
                affected_object_id: 9000,
                timestamp: 2_500,
                health_change: -1_200.0,
                new_health_value: 800.0,
                effect_type: EffectType::Physical,
                effect_origin: EffectOrigin::Direct,
                causer_id: 10,
                causing_spell_index: 2104,
            }),
            2_500,
        );
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::HarvestFinished {
                user_object_id: 10,
                harvest_object_id: 9001,
                item_id: 4011,
                standard_amount: 4,
                collector_bonus_amount: 1,
                premium_bonus_amount: 1,
            },
            2_600,
        );
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::OtherGrabbedLoot {
                object_id: 9002,
                looted_from_name: "Boss".into(),
                looted_by_name: "Explorer".into(),
                is_silver: true,
                item_index: 0,
                quantity: 450,
            },
            2_700,
        );
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::UpdateCurrency {
                city_faction: CityFaction::Thetford,
                gained_faction_coins: FixPoint::from_f64(150.0),
                faction_rank_points: FixPoint::from_f64(25.0),
                premium_bonus: FixPoint::from_f64(0.0),
                total_player_faction_points: FixPoint::from_f64(1_000.0),
            },
            2_800,
        );
        engine.ingest_telemetry_event(
            AlbionTelemetryEvent::TradeFinished {
                player1_silver: FixPoint::from_f64(5_000.0),
                player2_silver: FixPoint::from_f64(0.0),
                sales_tax: FixPoint::from_f64(125.0),
                item_count: 2,
            },
            2_900,
        );

        let snap = engine.snapshot(3_000);
        assert_eq!(snap.live_session_zones.len(), 2);
        assert_eq!(snap.live_session_combat_players.len(), 1);
        assert_eq!(snap.live_session_dungeons.len(), 1);
        assert_eq!(snap.live_session_gathering_totals.ore, 6);
        assert_eq!(snap.live_session_loot_log.len(), 1);
        assert_eq!(snap.live_session_trades.len(), 1);

        let archived = engine
            .archive_session(Some("Session Brumes".into()), 4_000)
            .expect("session should archive");
        assert_eq!(archived.label, "Session Brumes");
        assert_eq!(archived.zones.len(), 2);
        assert_eq!(archived.combat_players.len(), 1);
        assert_eq!(archived.combat_players[0].spells.len(), 1);
        assert_eq!(archived.dungeons.len(), 1);
        assert!(!archived.dungeons[0].is_active);
        assert_eq!(archived.gathering_totals.ore, 6);
        assert_eq!(archived.gathering_log.len(), 1);
        assert_eq!(archived.loot_log.len(), 1);
        assert_eq!(archived.trades.len(), 1);
        assert!(archived
            .factions
            .iter()
            .any(|f| f.city == "Thetford" && (f.coins - 150.0).abs() < 0.01));

        // Legacy archive JSON (missing extended detail fields) must deserialize cleanly
        let legacy_json = r#"{
            "id":"sess-legacy",
            "label":"Ancienne session",
            "startedAtMs":1000,
            "endedAtMs":2000,
            "durationSeconds":600.0,
            "mapNameFr":"Thetford",
            "fame":12000.0,
            "famePerHour":72000.0,
            "silver":5000.0,
            "silverPerHour":30000.0,
            "respecPoints":100.0,
            "might":0.0,
            "favor":0.0,
            "kills":1,
            "deaths":0
        }"#;
        let legacy: ArchivedSession = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(legacy.label, "Ancienne session");
        assert!(legacy.zones.is_empty());
        assert!(legacy.combat_players.is_empty());
        assert!(legacy.gathering_log.is_empty());
    }
}
