//! O(1) Albion Online `EventCode` (`param[252]`) and `OperationCode` (`param[253]`) decoder.

use crate::photon::PhotonMessage;
use crate::protocol16::{
    EventDataPayload, FixPoint, Guid16, OperationRequestPayload, OperationResponsePayload,
    ParameterTable, Protocol16Value,
};
use serde::{Deserialize, Serialize};

// EventCode constants (parameter key 252)
pub const EV_LEAVE: i16 = 1;
pub const EV_HEALTH_UPDATE: i16 = 6;
pub const EV_HEALTH_UPDATES: i16 = 7;
pub const EV_ACTIVE_SPELL_EFFECTS_UPDATE: i16 = 11;
pub const EV_NEW_CHARACTER: i16 = 29;
pub const EV_NEW_EQUIPMENT_ITEM: i16 = 30;
pub const EV_NEW_SIMPLE_ITEM: i16 = 32;
pub const EV_NEW_FURNITURE_ITEM: i16 = 33;
pub const EV_NEW_JOURNAL_ITEM: i16 = 35;
pub const EV_HARVEST_FINISHED: i16 = 61;
pub const EV_TAKE_SILVER: i16 = 62;
pub const EV_UPDATE_MONEY: i16 = 81;
pub const EV_UPDATE_FAME: i16 = 82;
pub const EV_UPDATE_RESPEC_POINTS: i16 = 84;
pub const EV_CHARACTER_EQUIPMENT_CHANGED: i16 = 90;
pub const EV_NEW_LOOT: i16 = 98;
pub const EV_DIED: i16 = 165;
pub const EV_TRADE_FINISHED: i16 = 191;
pub const EV_PARTY_JOINED: i16 = 223;
pub const EV_PARTY_DISBANDED: i16 = 224;
pub const EV_PARTY_PLAYER_JOINED: i16 = 225;
pub const EV_PARTY_PLAYER_LEFT: i16 = 227;
pub const EV_IN_COMBAT_STATE_UPDATE_ENUM: i16 = 259;
pub const EV_IN_COMBAT_STATE_UPDATE: i16 = 274;
pub const EV_OTHER_GRABBED_LOOT: i16 = 275;
pub const EV_UPDATE_CURRENCY: i16 = 290;
pub const EV_RECEIVED_GVG_SEASON_POINTS: i16 = 381;
pub const EV_LOOT_CHEST_OPENED: i16 = 390;
pub const EV_UPDATE_FACTION_STANDING: i16 = 407;
pub const EV_MISTS_WISP_CAGE_OPENED: i16 = 501;
pub const EV_MIGHT_AND_FAVOR_RECEIVED: i16 = 514;

// OperationCode constants (parameter key 253)
pub const OP_JOIN: i16 = 2;
pub const OP_CHANGE_CLUSTER: i16 = 35;
pub const OP_AUCTION_GET_OFFERS: i16 = 75;
pub const OP_AUCTION_GET_REQUESTS: i16 = 76;
pub const OP_REPAIR_EQUIPMENT_ITEM: i16 = 102;
pub const OP_READ_MAIL: i16 = 170;

/// Damage/heal effect type in `HealthUpdateEvent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EffectType {
    None,
    Physical,
    Magic,
}

impl EffectType {
    pub fn from_code(code: i32) -> Self {
        match code {
            1 => Self::Physical,
            2 => Self::Magic,
            _ => Self::None,
        }
    }
}

/// Direct vs DoT/HoT origin in `HealthUpdateEvent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EffectOrigin {
    Unknown,
    Direct,
    OverTime,
}

impl EffectOrigin {
    pub fn from_code(code: i32) -> Self {
        match code {
            1 => Self::Direct,
            2 => Self::OverTime,
            _ => Self::Unknown,
        }
    }
}

/// Royal / Outland city factions in Albion Online.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CityFaction {
    Unknown,
    Martlock,
    Lymhurst,
    Bridgewatch,
    FortSterling,
    Thetford,
    Caerleon,
}

impl CityFaction {
    pub fn from_u8(code: u8) -> Self {
        match code {
            1 => Self::Martlock,
            2 => Self::Lymhurst,
            3 => Self::Bridgewatch,
            4 => Self::FortSterling,
            5 => Self::Thetford,
            6 => Self::Caerleon,
            _ => Self::Unknown,
        }
    }

    pub fn label_fr(self) -> &'static str {
        match self {
            Self::Martlock => "Martlock",
            Self::Lymhurst => "Lymhurst",
            Self::Bridgewatch => "Bridgewatch",
            Self::FortSterling => "Fort Sterling",
            Self::Thetford => "Thetford",
            Self::Caerleon => "Caerleon",
            Self::Unknown => "Inconnue",
        }
    }
}

/// 10-slot character equipment array (`0=MainHand .. 9=BuffFood`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CharacterEquipment {
    pub main_hand: i32,
    pub off_hand: i32,
    pub head: i32,
    pub chest: i32,
    pub shoes: i32,
    pub bag: i32,
    pub cape: i32,
    pub mount: i32,
    pub potion: i32,
    pub buff_food: i32,
}

impl CharacterEquipment {
    pub fn from_slice(items: &[i32]) -> Self {
        Self {
            main_hand: items.first().copied().unwrap_or(0),
            off_hand: items.get(1).copied().unwrap_or(0),
            head: items.get(2).copied().unwrap_or(0),
            chest: items.get(3).copied().unwrap_or(0),
            shoes: items.get(4).copied().unwrap_or(0),
            bag: items.get(5).copied().unwrap_or(0),
            cape: items.get(6).copied().unwrap_or(0),
            mount: items.get(7).copied().unwrap_or(0),
            potion: items.get(8).copied().unwrap_or(0),
            buff_food: items.get(9).copied().unwrap_or(0),
        }
    }

    pub fn to_vec(&self) -> Vec<i32> {
        vec![
            self.main_hand,
            self.off_hand,
            self.head,
            self.chest,
            self.shoes,
            self.bag,
            self.cape,
            self.mount,
            self.potion,
            self.buff_food,
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthTick {
    pub affected_object_id: i64,
    pub timestamp: i64,
    pub health_change: f64,
    pub new_health_value: f64,
    pub effect_type: EffectType,
    pub effect_origin: EffectOrigin,
    pub causer_id: i64,
    pub causing_spell_index: i32,
}

/// Strongly typed Albion Online telemetry event decoded in `O(1)` from a [`PhotonMessage`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AlbionTelemetryEvent {
    Leave {
        object_id: i64,
    },
    HealthUpdate(HealthTick),
    HealthUpdates {
        ticks: Vec<HealthTick>,
    },
    ActiveSpellEffectsUpdate {
        causer_id: i64,
        spell_indices: Vec<i32>,
    },
    NewCharacter {
        object_id: i64,
        name: String,
        guid: Guid16,
        guild_name: String,
        equipment: CharacterEquipment,
    },
    CharacterEquipmentChanged {
        object_id: i64,
        equipment: CharacterEquipment,
    },
    NewDiscoveredItem {
        object_id: i64,
        item_id: i32,
        quantity: i32,
        estimated_market_value: i64,
        quality_level: i16,
    },
    HarvestFinished {
        user_object_id: i64,
        harvest_object_id: i64,
        item_id: i32,
        standard_amount: i32,
        collector_bonus_amount: i32,
        premium_bonus_amount: i32,
    },
    TakeSilver {
        object_id: i64,
        target_entity_id: i64,
        yield_pre_tax: FixPoint,
        guild_tax: FixPoint,
        cluster_tax: FixPoint,
        is_premium_bonus: bool,
    },
    UpdateMoney {
        object_id: i64,
        current_player_silver: FixPoint,
    },
    UpdateFame {
        object_id: i64,
        total_player_fame: FixPoint,
        fame_with_zone_multiplier: FixPoint,
        zone_fame: FixPoint,
        is_premium_bonus: bool,
        satchel_fame: FixPoint,
        bonus_factor: f32,
    },
    UpdateReSpecPoints {
        current_total_respec_points: FixPoint,
        gained_respec_points: FixPoint,
        paid_silver: FixPoint,
    },
    NewLoot {
        object_id: i64,
        loot_body: String,
    },
    Died {
        died_player: String,
        died_guild: String,
        killed_by: String,
        killed_by_guild: String,
    },
    TradeFinished {
        player1_silver: FixPoint,
        player2_silver: FixPoint,
        sales_tax: FixPoint,
        item_count: usize,
    },
    PartyJoined {
        party_object_id: i64,
        members: Vec<(Guid16, String)>,
    },
    PartyDisbanded {
        party_object_id: i64,
    },
    PartyPlayerJoined {
        party_object_id: i64,
        user_guid: Guid16,
        username: String,
    },
    PartyPlayerLeft {
        party_object_id: i64,
        user_guid: Guid16,
    },
    InCombatStateUpdate {
        object_id: i64,
        in_active_combat: bool,
        in_passive_combat: bool,
    },
    OtherGrabbedLoot {
        object_id: i64,
        looted_from_name: String,
        looted_by_name: String,
        is_silver: bool,
        item_index: i32,
        quantity: i32,
    },
    UpdateCurrency {
        city_faction: CityFaction,
        gained_faction_coins: FixPoint,
        faction_rank_points: FixPoint,
        premium_bonus: FixPoint,
        total_player_faction_points: FixPoint,
    },
    UpdateFactionStanding {
        city_faction: CityFaction,
        gained_flag_points: FixPoint,
        bonus_premium: FixPoint,
        total_flag_points: FixPoint,
    },
    ReceivedGvgSeasonPoints {
        season_points: i32,
    },
    LootChestOpened {
        object_id: i64,
    },
    MistsWispCageOpened {
        object_id: i64,
    },
    MightAndFavorReceived {
        might: FixPoint,
        bonus_might: FixPoint,
        premium_might: FixPoint,
        favor: FixPoint,
        bonus_favor: FixPoint,
        premium_favor: FixPoint,
        total_favor: FixPoint,
    },
    JoinResponse {
        user_object_id: i64,
        user_guid: Guid16,
        username: String,
        map_index: String,
        main_map_index: String,
        guild_name: String,
        alliance_name: String,
        current_focus_points: f64,
        max_focus_points: f64,
        silver: FixPoint,
        gold: FixPoint,
        learning_points: FixPoint,
        reputation: f64,
        is_respec_active: bool,
    },
    ChangeClusterResponse {
        index: String,
        world_map_data_type: String,
        island_name: String,
        mists_dungeon_tier: i32,
    },
    AuctionEntriesResponse {
        is_requests: bool,
        raw_json_entries: Vec<String>,
    },
    RepairEquipmentRequest {
        slot_id: i32,
        repair_cost: FixPoint,
    },
    ReadMailResponse {
        mail_id: i64,
        content: String,
    },
}

/// Decodes a [`PhotonMessage`] into an [`AlbionTelemetryEvent`] in `O(1)` via `match`.
pub fn decode_photon_message(msg: &PhotonMessage) -> Option<AlbionTelemetryEvent> {
    match msg {
        PhotonMessage::EventData(ev) => decode_event_data(ev),
        PhotonMessage::OperationRequest(req) => decode_operation_request(req),
        PhotonMessage::OperationResponse(resp) => decode_operation_response(resp),
    }
}

fn decode_event_data(ev: &EventDataPayload) -> Option<AlbionTelemetryEvent> {
    // High-frequency Move events use outer code == 3 without key 252
    if ev.code == 3 {
        return None;
    }
    let p = &ev.parameters;
    let event_code = p.get_i16(252)?;

    match event_code {
        EV_LEAVE => Some(AlbionTelemetryEvent::Leave {
            object_id: p.get_i64(0)?,
        }),
        EV_HEALTH_UPDATE => Some(AlbionTelemetryEvent::HealthUpdate(HealthTick {
            affected_object_id: p.get_i64(0)?,
            timestamp: p.get_i64(1).unwrap_or(0),
            health_change: p.get_f64(2).unwrap_or(0.0),
            new_health_value: p.get_f64(3).unwrap_or(0.0),
            effect_type: EffectType::from_code(p.get_i32(4).unwrap_or(0)),
            effect_origin: EffectOrigin::from_code(p.get_i32(5).unwrap_or(0)),
            causer_id: p.get_i64(6).unwrap_or(0),
            causing_spell_index: p.get_i32(7).unwrap_or(0),
        })),
        EV_HEALTH_UPDATES => {
            let affected_object_id = p.get_i64(0)?;
            let timestamps = p.get_i64_vec(1).unwrap_or_default();
            let health_changes = p.get_f64_vec(2)?;
            let new_health_values = p.get_f64_vec(3).unwrap_or_default();
            let effect_types = p.get_i32_vec(4).unwrap_or_default();
            let effect_origins = p.get_i32_vec(5).unwrap_or_default();
            let causer_ids = p.get_i64_vec(6).unwrap_or_default();
            let spell_indices = p.get_i32_vec(7).unwrap_or_default();

            let mut ticks = Vec::with_capacity(health_changes.len());
            for (idx, &health_change) in health_changes.iter().enumerate() {
                ticks.push(HealthTick {
                    affected_object_id,
                    timestamp: timestamps.get(idx).copied().unwrap_or(0),
                    health_change,
                    new_health_value: new_health_values.get(idx).copied().unwrap_or(0.0),
                    effect_type: EffectType::from_code(effect_types.get(idx).copied().unwrap_or(0)),
                    effect_origin: EffectOrigin::from_code(
                        effect_origins.get(idx).copied().unwrap_or(0),
                    ),
                    causer_id: causer_ids.get(idx).copied().unwrap_or(0),
                    causing_spell_index: spell_indices.get(idx).copied().unwrap_or(0),
                });
            }
            Some(AlbionTelemetryEvent::HealthUpdates { ticks })
        }
        EV_ACTIVE_SPELL_EFFECTS_UPDATE => {
            let causer_id = p.get_i64(0)?;
            let spell_indices = p
                .get_i32_vec(1)
                .or_else(|| p.get_i32(1).map(|s| vec![s]))
                .unwrap_or_default();
            Some(AlbionTelemetryEvent::ActiveSpellEffectsUpdate {
                causer_id,
                spell_indices,
            })
        }
        EV_NEW_CHARACTER => {
            let object_id = p.get_i64(0)?;
            let name = p.get_str(1).unwrap_or_default().to_string();
            let guid = p.get_guid(7).unwrap_or_default();
            let guild_name = p.get_str(8).unwrap_or_default().to_string();
            let equip_slice = p.get_i32_vec(40).unwrap_or_default();
            Some(AlbionTelemetryEvent::NewCharacter {
                object_id,
                name,
                guid,
                guild_name,
                equipment: CharacterEquipment::from_slice(&equip_slice),
            })
        }
        EV_CHARACTER_EQUIPMENT_CHANGED => {
            let object_id = p.get_i64(0)?;
            let equip_slice = p.get_i32_vec(2).unwrap_or_default();
            Some(AlbionTelemetryEvent::CharacterEquipmentChanged {
                object_id,
                equipment: CharacterEquipment::from_slice(&equip_slice),
            })
        }
        EV_NEW_EQUIPMENT_ITEM
        | EV_NEW_SIMPLE_ITEM
        | EV_NEW_FURNITURE_ITEM
        | EV_NEW_JOURNAL_ITEM => {
            let object_id = p.get_i64(0)?;
            let item_id = p.get_i32(1)?;
            let quantity = p.get_i32(2).unwrap_or(1);
            let estimated_market_value = p.get_i64(4).unwrap_or(0);
            let quality_level = p.get_i16(6).unwrap_or(1);
            Some(AlbionTelemetryEvent::NewDiscoveredItem {
                object_id,
                item_id,
                quantity,
                estimated_market_value,
                quality_level,
            })
        }
        EV_HARVEST_FINISHED => Some(AlbionTelemetryEvent::HarvestFinished {
            user_object_id: p.get_i64(0)?,
            harvest_object_id: p.get_i64(3).unwrap_or(0),
            item_id: p.get_i32(4)?,
            standard_amount: p.get_i32(5).unwrap_or(0),
            collector_bonus_amount: p.get_i32(6).unwrap_or(0),
            premium_bonus_amount: p.get_i32(7).unwrap_or(0),
        }),
        EV_TAKE_SILVER => Some(AlbionTelemetryEvent::TakeSilver {
            object_id: p.get_i64(0)?,
            target_entity_id: p.get_i64(2).unwrap_or(0),
            yield_pre_tax: p.get_fixpoint(3).unwrap_or_default(),
            guild_tax: p.get_fixpoint(5).unwrap_or_default(),
            cluster_tax: p.get_fixpoint(6).unwrap_or_default(),
            is_premium_bonus: p.get_bool(7).unwrap_or(false),
        }),
        EV_UPDATE_MONEY => Some(AlbionTelemetryEvent::UpdateMoney {
            object_id: p.get_i64(0)?,
            current_player_silver: p.get_fixpoint(1)?,
        }),
        EV_UPDATE_FAME => Some(AlbionTelemetryEvent::UpdateFame {
            object_id: p.get_i64(0).unwrap_or(0),
            total_player_fame: p.get_fixpoint(1).unwrap_or_default(),
            fame_with_zone_multiplier: p.get_fixpoint(2)?,
            zone_fame: p.get_fixpoint(3).unwrap_or_default(),
            is_premium_bonus: p.get_bool(5).unwrap_or(false),
            satchel_fame: p.get_fixpoint(10).unwrap_or_default(),
            bonus_factor: p.get_f32(17).unwrap_or(0.0) + 1.0,
        }),
        EV_UPDATE_RESPEC_POINTS => {
            let total_raw = p
                .get_i64_vec(0)
                .and_then(|v| v.get(1).copied())
                .or_else(|| p.get_i64(0))
                .unwrap_or(0);
            Some(AlbionTelemetryEvent::UpdateReSpecPoints {
                current_total_respec_points: FixPoint::from_raw(total_raw),
                gained_respec_points: p.get_fixpoint(2).unwrap_or_default(),
                paid_silver: p.get_fixpoint(3).unwrap_or_default(),
            })
        }
        EV_NEW_LOOT => Some(AlbionTelemetryEvent::NewLoot {
            object_id: p.get_i64(0)?,
            loot_body: p.get_str(3).unwrap_or_default().to_string(),
        }),
        EV_DIED => Some(AlbionTelemetryEvent::Died {
            died_player: p.get_str(2).unwrap_or_default().to_string(),
            died_guild: p.get_str(3).unwrap_or_default().to_string(),
            killed_by: p.get_str(10).unwrap_or_default().to_string(),
            killed_by_guild: p.get_str(11).unwrap_or_default().to_string(),
        }),
        EV_TRADE_FINISHED => {
            let p1_items = p.get_i64_vec(8).map(|v| v.len()).unwrap_or(0);
            let p2_items = p.get_i64_vec(9).map(|v| v.len()).unwrap_or(0);
            Some(AlbionTelemetryEvent::TradeFinished {
                player1_silver: p.get_fixpoint(10).unwrap_or_default(),
                player2_silver: p.get_fixpoint(11).unwrap_or_default(),
                sales_tax: p.get_fixpoint(12).unwrap_or_default(),
                item_count: p1_items + p2_items,
            })
        }
        EV_PARTY_JOINED => {
            let party_object_id = p.get_i64(0).unwrap_or(0);
            let guids = p
                .get_guid_vec(4)
                .or_else(|| p.get_guid_vec(5))
                .unwrap_or_default();
            let names = p
                .get_string_vec(5)
                .or_else(|| p.get_string_vec(6))
                .unwrap_or_default();
            let members = guids.into_iter().zip(names).collect();
            Some(AlbionTelemetryEvent::PartyJoined {
                party_object_id,
                members,
            })
        }
        EV_PARTY_DISBANDED => Some(AlbionTelemetryEvent::PartyDisbanded {
            party_object_id: p.get_i64(0).unwrap_or(0),
        }),
        EV_PARTY_PLAYER_JOINED => Some(AlbionTelemetryEvent::PartyPlayerJoined {
            party_object_id: p.get_i64(0).unwrap_or(0),
            user_guid: p.get_guid(1)?,
            username: p.get_str(2).unwrap_or_default().to_string(),
        }),
        EV_PARTY_PLAYER_LEFT => Some(AlbionTelemetryEvent::PartyPlayerLeft {
            party_object_id: p.get_i64(0).unwrap_or(0),
            user_guid: p.get_guid(1)?,
        }),
        EV_IN_COMBAT_STATE_UPDATE | EV_IN_COMBAT_STATE_UPDATE_ENUM => {
            Some(AlbionTelemetryEvent::InCombatStateUpdate {
                object_id: p.get_i64(0)?,
                in_active_combat: p.get_bool(1).unwrap_or(false),
                in_passive_combat: p.get_bool(2).unwrap_or(false),
            })
        }
        EV_OTHER_GRABBED_LOOT => Some(AlbionTelemetryEvent::OtherGrabbedLoot {
            object_id: p.get_i64(0).unwrap_or(0),
            looted_from_name: p.get_str(1).unwrap_or_default().to_string(),
            looted_by_name: p.get_str(2).unwrap_or_default().to_string(),
            is_silver: p.get_bool(3).unwrap_or(false),
            item_index: p.get_i32(4).unwrap_or(0),
            quantity: p.get_i32(5).unwrap_or(0),
        }),
        EV_UPDATE_CURRENCY => Some(AlbionTelemetryEvent::UpdateCurrency {
            city_faction: CityFaction::from_u8(p.get_u8(2).unwrap_or(0)),
            gained_faction_coins: p.get_fixpoint(3).unwrap_or_default(),
            faction_rank_points: p.get_fixpoint(4).unwrap_or_default(),
            premium_bonus: p.get_fixpoint(8).unwrap_or_default(),
            total_player_faction_points: p.get_fixpoint(9).unwrap_or_default(),
        }),
        EV_UPDATE_FACTION_STANDING => Some(AlbionTelemetryEvent::UpdateFactionStanding {
            city_faction: CityFaction::from_u8(p.get_u8(0).unwrap_or(0)),
            gained_flag_points: p.get_fixpoint(1).unwrap_or_default(),
            bonus_premium: p.get_fixpoint(2).unwrap_or_default(),
            total_flag_points: p.get_fixpoint(3).unwrap_or_default(),
        }),
        EV_RECEIVED_GVG_SEASON_POINTS => Some(AlbionTelemetryEvent::ReceivedGvgSeasonPoints {
            season_points: p.get_i32(1)?,
        }),
        EV_LOOT_CHEST_OPENED => Some(AlbionTelemetryEvent::LootChestOpened {
            object_id: p.get_i64(0)?,
        }),
        EV_MISTS_WISP_CAGE_OPENED => Some(AlbionTelemetryEvent::MistsWispCageOpened {
            object_id: p.get_i64(0)?,
        }),
        EV_MIGHT_AND_FAVOR_RECEIVED => Some(AlbionTelemetryEvent::MightAndFavorReceived {
            might: p.get_fixpoint(1).unwrap_or_default(),
            bonus_might: p.get_fixpoint(2).unwrap_or_default(),
            premium_might: p.get_fixpoint(3).unwrap_or_default(),
            favor: p.get_fixpoint(4).unwrap_or_default(),
            bonus_favor: p.get_fixpoint(5).unwrap_or_default(),
            premium_favor: p.get_fixpoint(6).unwrap_or_default(),
            total_favor: p.get_fixpoint(7).unwrap_or_default(),
        }),
        _ => None,
    }
}

fn decode_operation_request(req: &OperationRequestPayload) -> Option<AlbionTelemetryEvent> {
    let p = &req.parameters;
    let op_code = p.get_i16(253)?;
    match op_code {
        OP_REPAIR_EQUIPMENT_ITEM => Some(AlbionTelemetryEvent::RepairEquipmentRequest {
            slot_id: p.get_i32(0).unwrap_or(0),
            repair_cost: p.get_fixpoint(1).unwrap_or_default(),
        }),
        _ => None,
    }
}

fn decode_operation_response(resp: &OperationResponsePayload) -> Option<AlbionTelemetryEvent> {
    let p = &resp.parameters;
    let op_code = p.get_i16(253)?;
    match op_code {
        OP_JOIN => {
            let user_object_id = p.get_i64(0)?;
            let user_guid = p.get_guid(1).unwrap_or_default();
            let username = p.get_str(2).unwrap_or_default().to_string();
            let map_index = p.get_str(8).unwrap_or_default().to_string();
            let current_focus_points = p.get_f64(26).unwrap_or(0.0);
            let max_focus_points = p.get_f64(27).unwrap_or(30_000.0);
            let silver = p
                .get_fixpoint(28)
                .or_else(|| p.get_fixpoint(32))
                .unwrap_or_default();
            let gold = p
                .get_fixpoint(29)
                .or_else(|| p.get_fixpoint(33))
                .unwrap_or_default();
            let learning_points = p.get_fixpoint(36).unwrap_or_default();
            let reputation = p.get_f64(40).unwrap_or(0.0);
            let guild_name = p.get_str(57).unwrap_or_default().to_string();
            let main_map_index = p.get_str(65).unwrap_or_default().to_string();
            let alliance_name = p.get_str(78).unwrap_or_default().to_string();
            let is_respec_active = p.get_bool(98).unwrap_or(false);
            Some(AlbionTelemetryEvent::JoinResponse {
                user_object_id,
                user_guid,
                username,
                map_index,
                main_map_index,
                guild_name,
                alliance_name,
                current_focus_points,
                max_focus_points,
                silver,
                gold,
                learning_points,
                reputation,
                is_respec_active,
            })
        }
        OP_CHANGE_CLUSTER => Some(AlbionTelemetryEvent::ChangeClusterResponse {
            index: p.get_str(0).unwrap_or_default().to_string(),
            world_map_data_type: p.get_str(1).unwrap_or_default().to_string(),
            island_name: p.get_str(2).unwrap_or_default().to_string(),
            mists_dungeon_tier: p.get_i32(5).unwrap_or(0),
        }),
        OP_AUCTION_GET_OFFERS => Some(AlbionTelemetryEvent::AuctionEntriesResponse {
            is_requests: false,
            raw_json_entries: p.get_string_vec(0).unwrap_or_default(),
        }),
        OP_AUCTION_GET_REQUESTS => Some(AlbionTelemetryEvent::AuctionEntriesResponse {
            is_requests: true,
            raw_json_entries: p.get_string_vec(0).unwrap_or_default(),
        }),
        OP_READ_MAIL => Some(AlbionTelemetryEvent::ReadMailResponse {
            mail_id: p.get_i64(0)?,
            content: p.get_str(1).unwrap_or_default().to_string(),
        }),
        _ => None,
    }
}

/// Encodes an [`AlbionTelemetryEvent`] back into a [`PhotonMessage`] with exact parameter indices.
pub fn encode_telemetry_event_to_photon(event: &AlbionTelemetryEvent) -> PhotonMessage {
    let mut p = ParameterTable::new();
    match event {
        AlbionTelemetryEvent::Leave { object_id } => {
            p.insert(0, Protocol16Value::Long(*object_id));
            p.insert(252, Protocol16Value::Short(EV_LEAVE));
            PhotonMessage::EventData(EventDataPayload {
                code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::HealthUpdate(tick) => {
            p.insert(0, Protocol16Value::Long(tick.affected_object_id));
            p.insert(1, Protocol16Value::Long(tick.timestamp));
            p.insert(2, Protocol16Value::Double(tick.health_change));
            p.insert(3, Protocol16Value::Double(tick.new_health_value));
            p.insert(
                4,
                Protocol16Value::Byte(match tick.effect_type {
                    EffectType::None => 0,
                    EffectType::Physical => 1,
                    EffectType::Magic => 2,
                }),
            );
            p.insert(
                5,
                Protocol16Value::Byte(match tick.effect_origin {
                    EffectOrigin::Unknown => 0,
                    EffectOrigin::Direct => 1,
                    EffectOrigin::OverTime => 2,
                }),
            );
            p.insert(6, Protocol16Value::Long(tick.causer_id));
            p.insert(7, Protocol16Value::Short(tick.causing_spell_index as i16));
            p.insert(252, Protocol16Value::Short(EV_HEALTH_UPDATE));
            PhotonMessage::EventData(EventDataPayload {
                code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::HealthUpdates { ticks } => {
            let affected_id = ticks.first().map(|t| t.affected_object_id).unwrap_or(0);
            p.insert(0, Protocol16Value::Long(affected_id));
            p.insert(
                1,
                Protocol16Value::ObjectArray(
                    ticks
                        .iter()
                        .map(|t| Protocol16Value::Long(t.timestamp))
                        .collect(),
                ),
            );
            p.insert(
                2,
                Protocol16Value::ObjectArray(
                    ticks
                        .iter()
                        .map(|t| Protocol16Value::Double(t.health_change))
                        .collect(),
                ),
            );
            p.insert(
                3,
                Protocol16Value::ObjectArray(
                    ticks
                        .iter()
                        .map(|t| Protocol16Value::Double(t.new_health_value))
                        .collect(),
                ),
            );
            p.insert(
                6,
                Protocol16Value::ObjectArray(
                    ticks
                        .iter()
                        .map(|t| Protocol16Value::Long(t.causer_id))
                        .collect(),
                ),
            );
            p.insert(
                7,
                Protocol16Value::IntArray(ticks.iter().map(|t| t.causing_spell_index).collect()),
            );
            p.insert(252, Protocol16Value::Short(EV_HEALTH_UPDATES));
            PhotonMessage::EventData(EventDataPayload {
                code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::ActiveSpellEffectsUpdate {
            causer_id,
            spell_indices,
        } => {
            p.insert(0, Protocol16Value::Long(*causer_id));
            p.insert(1, Protocol16Value::IntArray(spell_indices.clone()));
            p.insert(252, Protocol16Value::Short(EV_ACTIVE_SPELL_EFFECTS_UPDATE));
            PhotonMessage::EventData(EventDataPayload {
                code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::NewCharacter {
            object_id,
            name,
            guid,
            guild_name,
            equipment,
        } => {
            p.insert(0, Protocol16Value::Long(*object_id));
            p.insert(1, Protocol16Value::String(name.clone()));
            p.insert(7, Protocol16Value::ByteArray(guid.0.to_vec()));
            p.insert(8, Protocol16Value::String(guild_name.clone()));
            p.insert(40, Protocol16Value::IntArray(equipment.to_vec()));
            p.insert(252, Protocol16Value::Short(EV_NEW_CHARACTER));
            PhotonMessage::EventData(EventDataPayload {
                code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::CharacterEquipmentChanged {
            object_id,
            equipment,
        } => {
            p.insert(0, Protocol16Value::Long(*object_id));
            p.insert(2, Protocol16Value::IntArray(equipment.to_vec()));
            p.insert(252, Protocol16Value::Short(EV_CHARACTER_EQUIPMENT_CHANGED));
            PhotonMessage::EventData(EventDataPayload {
                code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::NewDiscoveredItem {
            object_id,
            item_id,
            quantity,
            estimated_market_value,
            quality_level,
        } => {
            p.insert(0, Protocol16Value::Long(*object_id));
            p.insert(1, Protocol16Value::Integer(*item_id));
            p.insert(2, Protocol16Value::Integer(*quantity));
            p.insert(4, Protocol16Value::Long(*estimated_market_value));
            p.insert(6, Protocol16Value::Short(*quality_level));
            p.insert(252, Protocol16Value::Short(EV_NEW_EQUIPMENT_ITEM));
            PhotonMessage::EventData(EventDataPayload {
                code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::HarvestFinished {
            user_object_id,
            harvest_object_id,
            item_id,
            standard_amount,
            collector_bonus_amount,
            premium_bonus_amount,
        } => {
            p.insert(0, Protocol16Value::Long(*user_object_id));
            p.insert(3, Protocol16Value::Long(*harvest_object_id));
            p.insert(4, Protocol16Value::Integer(*item_id));
            p.insert(5, Protocol16Value::Integer(*standard_amount));
            p.insert(6, Protocol16Value::Integer(*collector_bonus_amount));
            p.insert(7, Protocol16Value::Integer(*premium_bonus_amount));
            p.insert(252, Protocol16Value::Short(EV_HARVEST_FINISHED));
            PhotonMessage::EventData(EventDataPayload {
                code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::TakeSilver {
            object_id,
            target_entity_id,
            yield_pre_tax,
            guild_tax,
            cluster_tax,
            is_premium_bonus,
        } => {
            p.insert(0, Protocol16Value::Long(*object_id));
            p.insert(2, Protocol16Value::Long(*target_entity_id));
            p.insert(3, Protocol16Value::Long(yield_pre_tax.raw()));
            p.insert(5, Protocol16Value::Long(guild_tax.raw()));
            p.insert(6, Protocol16Value::Long(cluster_tax.raw()));
            p.insert(7, Protocol16Value::Boolean(*is_premium_bonus));
            p.insert(252, Protocol16Value::Short(EV_TAKE_SILVER));
            PhotonMessage::EventData(EventDataPayload {
                code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::UpdateMoney {
            object_id,
            current_player_silver,
        } => {
            p.insert(0, Protocol16Value::Long(*object_id));
            p.insert(1, Protocol16Value::Long(current_player_silver.raw()));
            p.insert(252, Protocol16Value::Short(EV_UPDATE_MONEY));
            PhotonMessage::EventData(EventDataPayload {
                code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::UpdateFame {
            object_id,
            total_player_fame,
            fame_with_zone_multiplier,
            zone_fame,
            is_premium_bonus,
            satchel_fame,
            bonus_factor,
        } => {
            p.insert(0, Protocol16Value::Long(*object_id));
            p.insert(1, Protocol16Value::Long(total_player_fame.raw()));
            p.insert(2, Protocol16Value::Long(fame_with_zone_multiplier.raw()));
            p.insert(3, Protocol16Value::Long(zone_fame.raw()));
            p.insert(5, Protocol16Value::Boolean(*is_premium_bonus));
            p.insert(10, Protocol16Value::Long(satchel_fame.raw()));
            p.insert(17, Protocol16Value::Float(*bonus_factor - 1.0));
            p.insert(252, Protocol16Value::Short(EV_UPDATE_FAME));
            PhotonMessage::EventData(EventDataPayload {
                code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::UpdateReSpecPoints {
            current_total_respec_points,
            gained_respec_points,
            paid_silver,
        } => {
            p.insert(
                0,
                Protocol16Value::ObjectArray(vec![
                    Protocol16Value::Long(0),
                    Protocol16Value::Long(current_total_respec_points.raw()),
                ]),
            );
            p.insert(2, Protocol16Value::Long(gained_respec_points.raw()));
            p.insert(3, Protocol16Value::Long(paid_silver.raw()));
            p.insert(252, Protocol16Value::Short(EV_UPDATE_RESPEC_POINTS));
            PhotonMessage::EventData(EventDataPayload {
                code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::NewLoot {
            object_id,
            loot_body,
        } => {
            p.insert(0, Protocol16Value::Long(*object_id));
            p.insert(3, Protocol16Value::String(loot_body.clone()));
            p.insert(252, Protocol16Value::Short(EV_NEW_LOOT));
            PhotonMessage::EventData(EventDataPayload {
                code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::Died {
            died_player,
            died_guild,
            killed_by,
            killed_by_guild,
        } => {
            p.insert(2, Protocol16Value::String(died_player.clone()));
            p.insert(3, Protocol16Value::String(died_guild.clone()));
            p.insert(10, Protocol16Value::String(killed_by.clone()));
            p.insert(11, Protocol16Value::String(killed_by_guild.clone()));
            p.insert(252, Protocol16Value::Short(EV_DIED));
            PhotonMessage::EventData(EventDataPayload {
                code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::TradeFinished {
            player1_silver,
            player2_silver,
            sales_tax,
            item_count,
        } => {
            p.insert(8, Protocol16Value::IntArray(vec![1; *item_count]));
            p.insert(10, Protocol16Value::Long(player1_silver.raw()));
            p.insert(11, Protocol16Value::Long(player2_silver.raw()));
            p.insert(12, Protocol16Value::Long(sales_tax.raw()));
            p.insert(252, Protocol16Value::Short(EV_TRADE_FINISHED));
            PhotonMessage::EventData(EventDataPayload {
                code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::PartyJoined {
            party_object_id,
            members,
        } => {
            p.insert(0, Protocol16Value::Long(*party_object_id));
            p.insert(
                4,
                Protocol16Value::ObjectArray(
                    members
                        .iter()
                        .map(|(g, _)| Protocol16Value::ByteArray(g.0.to_vec()))
                        .collect(),
                ),
            );
            p.insert(
                5,
                Protocol16Value::StringArray(members.iter().map(|(_, n)| n.clone()).collect()),
            );
            p.insert(252, Protocol16Value::Short(EV_PARTY_JOINED));
            PhotonMessage::EventData(EventDataPayload {
                code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::PartyDisbanded { party_object_id } => {
            p.insert(0, Protocol16Value::Long(*party_object_id));
            p.insert(252, Protocol16Value::Short(EV_PARTY_DISBANDED));
            PhotonMessage::EventData(EventDataPayload {
                code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::PartyPlayerJoined {
            party_object_id,
            user_guid,
            username,
        } => {
            p.insert(0, Protocol16Value::Long(*party_object_id));
            p.insert(1, Protocol16Value::ByteArray(user_guid.0.to_vec()));
            p.insert(2, Protocol16Value::String(username.clone()));
            p.insert(252, Protocol16Value::Short(EV_PARTY_PLAYER_JOINED));
            PhotonMessage::EventData(EventDataPayload {
                code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::PartyPlayerLeft {
            party_object_id,
            user_guid,
        } => {
            p.insert(0, Protocol16Value::Long(*party_object_id));
            p.insert(1, Protocol16Value::ByteArray(user_guid.0.to_vec()));
            p.insert(252, Protocol16Value::Short(EV_PARTY_PLAYER_LEFT));
            PhotonMessage::EventData(EventDataPayload {
                code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::InCombatStateUpdate {
            object_id,
            in_active_combat,
            in_passive_combat,
        } => {
            p.insert(0, Protocol16Value::Long(*object_id));
            p.insert(1, Protocol16Value::Boolean(*in_active_combat));
            p.insert(2, Protocol16Value::Boolean(*in_passive_combat));
            p.insert(252, Protocol16Value::Short(EV_IN_COMBAT_STATE_UPDATE));
            PhotonMessage::EventData(EventDataPayload {
                code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::OtherGrabbedLoot {
            object_id,
            looted_from_name,
            looted_by_name,
            is_silver,
            item_index,
            quantity,
        } => {
            p.insert(0, Protocol16Value::Long(*object_id));
            p.insert(1, Protocol16Value::String(looted_from_name.clone()));
            p.insert(2, Protocol16Value::String(looted_by_name.clone()));
            p.insert(3, Protocol16Value::Boolean(*is_silver));
            p.insert(4, Protocol16Value::Integer(*item_index));
            p.insert(5, Protocol16Value::Integer(*quantity));
            p.insert(252, Protocol16Value::Short(EV_OTHER_GRABBED_LOOT));
            PhotonMessage::EventData(EventDataPayload {
                code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::UpdateCurrency {
            city_faction,
            gained_faction_coins,
            faction_rank_points,
            premium_bonus,
            total_player_faction_points,
        } => {
            p.insert(
                2,
                Protocol16Value::Byte(match city_faction {
                    CityFaction::Martlock => 1,
                    CityFaction::Lymhurst => 2,
                    CityFaction::Bridgewatch => 3,
                    CityFaction::FortSterling => 4,
                    CityFaction::Thetford => 5,
                    CityFaction::Caerleon => 6,
                    CityFaction::Unknown => 0,
                }),
            );
            p.insert(3, Protocol16Value::Long(gained_faction_coins.raw()));
            p.insert(4, Protocol16Value::Long(faction_rank_points.raw()));
            p.insert(8, Protocol16Value::Long(premium_bonus.raw()));
            p.insert(9, Protocol16Value::Long(total_player_faction_points.raw()));
            p.insert(252, Protocol16Value::Short(EV_UPDATE_CURRENCY));
            PhotonMessage::EventData(EventDataPayload {
                code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::UpdateFactionStanding {
            city_faction,
            gained_flag_points,
            bonus_premium,
            total_flag_points,
        } => {
            p.insert(
                0,
                Protocol16Value::Byte(match city_faction {
                    CityFaction::Martlock => 1,
                    CityFaction::Lymhurst => 2,
                    CityFaction::Bridgewatch => 3,
                    CityFaction::FortSterling => 4,
                    CityFaction::Thetford => 5,
                    CityFaction::Caerleon => 6,
                    CityFaction::Unknown => 0,
                }),
            );
            p.insert(1, Protocol16Value::Long(gained_flag_points.raw()));
            p.insert(2, Protocol16Value::Long(bonus_premium.raw()));
            p.insert(3, Protocol16Value::Long(total_flag_points.raw()));
            p.insert(252, Protocol16Value::Short(EV_UPDATE_FACTION_STANDING));
            PhotonMessage::EventData(EventDataPayload {
                code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::ReceivedGvgSeasonPoints { season_points } => {
            p.insert(1, Protocol16Value::Integer(*season_points));
            p.insert(252, Protocol16Value::Short(EV_RECEIVED_GVG_SEASON_POINTS));
            PhotonMessage::EventData(EventDataPayload {
                code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::LootChestOpened { object_id } => {
            p.insert(0, Protocol16Value::Long(*object_id));
            p.insert(252, Protocol16Value::Short(EV_LOOT_CHEST_OPENED));
            PhotonMessage::EventData(EventDataPayload {
                code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::MistsWispCageOpened { object_id } => {
            p.insert(0, Protocol16Value::Long(*object_id));
            p.insert(252, Protocol16Value::Short(EV_MISTS_WISP_CAGE_OPENED));
            PhotonMessage::EventData(EventDataPayload {
                code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::MightAndFavorReceived {
            might,
            bonus_might,
            premium_might,
            favor,
            bonus_favor,
            premium_favor,
            total_favor,
        } => {
            p.insert(1, Protocol16Value::Long(might.raw()));
            p.insert(2, Protocol16Value::Long(bonus_might.raw()));
            p.insert(3, Protocol16Value::Long(premium_might.raw()));
            p.insert(4, Protocol16Value::Long(favor.raw()));
            p.insert(5, Protocol16Value::Long(bonus_favor.raw()));
            p.insert(6, Protocol16Value::Long(premium_favor.raw()));
            p.insert(7, Protocol16Value::Long(total_favor.raw()));
            p.insert(252, Protocol16Value::Short(EV_MIGHT_AND_FAVOR_RECEIVED));
            PhotonMessage::EventData(EventDataPayload {
                code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::JoinResponse {
            user_object_id,
            user_guid,
            username,
            map_index,
            main_map_index,
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
            p.insert(0, Protocol16Value::Long(*user_object_id));
            p.insert(1, Protocol16Value::ByteArray(user_guid.0.to_vec()));
            p.insert(2, Protocol16Value::String(username.clone()));
            p.insert(8, Protocol16Value::String(map_index.clone()));
            p.insert(26, Protocol16Value::Double(*current_focus_points));
            p.insert(27, Protocol16Value::Double(*max_focus_points));
            p.insert(28, Protocol16Value::Long(silver.raw()));
            p.insert(29, Protocol16Value::Long(gold.raw()));
            p.insert(36, Protocol16Value::Long(learning_points.raw()));
            p.insert(40, Protocol16Value::Double(*reputation));
            p.insert(57, Protocol16Value::String(guild_name.clone()));
            p.insert(65, Protocol16Value::String(main_map_index.clone()));
            p.insert(78, Protocol16Value::String(alliance_name.clone()));
            p.insert(98, Protocol16Value::Boolean(*is_respec_active));
            p.insert(253, Protocol16Value::Short(OP_JOIN));
            PhotonMessage::OperationResponse(OperationResponsePayload {
                operation_code: 1,
                return_code: 0,
                debug_message: None,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::ChangeClusterResponse {
            index,
            world_map_data_type,
            island_name,
            mists_dungeon_tier,
        } => {
            p.insert(0, Protocol16Value::String(index.clone()));
            p.insert(1, Protocol16Value::String(world_map_data_type.clone()));
            p.insert(2, Protocol16Value::String(island_name.clone()));
            p.insert(5, Protocol16Value::Integer(*mists_dungeon_tier));
            p.insert(253, Protocol16Value::Short(OP_CHANGE_CLUSTER));
            PhotonMessage::OperationResponse(OperationResponsePayload {
                operation_code: 1,
                return_code: 0,
                debug_message: None,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::AuctionEntriesResponse {
            is_requests,
            raw_json_entries,
        } => {
            p.insert(0, Protocol16Value::StringArray(raw_json_entries.clone()));
            p.insert(
                253,
                Protocol16Value::Short(if *is_requests {
                    OP_AUCTION_GET_REQUESTS
                } else {
                    OP_AUCTION_GET_OFFERS
                }),
            );
            PhotonMessage::OperationResponse(OperationResponsePayload {
                operation_code: 1,
                return_code: 0,
                debug_message: None,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::RepairEquipmentRequest {
            slot_id,
            repair_cost,
        } => {
            p.insert(0, Protocol16Value::Integer(*slot_id));
            p.insert(1, Protocol16Value::Long(repair_cost.raw()));
            p.insert(253, Protocol16Value::Short(OP_REPAIR_EQUIPMENT_ITEM));
            PhotonMessage::OperationRequest(OperationRequestPayload {
                operation_code: 1,
                parameters: p,
            })
        }
        AlbionTelemetryEvent::ReadMailResponse { mail_id, content } => {
            p.insert(0, Protocol16Value::Long(*mail_id));
            p.insert(1, Protocol16Value::String(content.clone()));
            p.insert(253, Protocol16Value::Short(OP_READ_MAIL));
            PhotonMessage::OperationResponse(OperationResponsePayload {
                operation_code: 1,
                return_code: 0,
                debug_message: None,
                parameters: p,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::photon::{build_photon_udp_packet, PhotonParser};

    #[test]
    fn roundtrips_health_fame_and_join_events_through_photon_wire() {
        let events = vec![
            AlbionTelemetryEvent::JoinResponse {
                user_object_id: 10,
                user_guid: Guid16([1; 16]),
                username: "Aurelia".into(),
                map_index: "3005".into(),
                main_map_index: "3005".into(),
                guild_name: "Vanguard".into(),
                alliance_name: "Aegis".into(),
                current_focus_points: 24_500.0,
                max_focus_points: 30_000.0,
                silver: FixPoint::from_f64(14_250_000.0),
                gold: FixPoint::from_f64(3_200.0),
                learning_points: FixPoint::from_f64(840.0),
                reputation: 12_500.0,
                is_respec_active: true,
            },
            AlbionTelemetryEvent::HealthUpdate(HealthTick {
                affected_object_id: 99,
                timestamp: 1000,
                health_change: -412.0,
                new_health_value: 1850.0,
                effect_type: EffectType::Magic,
                effect_origin: EffectOrigin::Direct,
                causer_id: 10,
                causing_spell_index: 2403,
            }),
            AlbionTelemetryEvent::UpdateFame {
                object_id: 10,
                total_player_fame: FixPoint::from_f64(50_000_000.0),
                fame_with_zone_multiplier: FixPoint::from_f64(18_450.0),
                zone_fame: FixPoint::from_f64(12_300.0),
                is_premium_bonus: true,
                satchel_fame: FixPoint::from_f64(4_100.0),
                bonus_factor: 1.5,
            },
        ];

        let photon_msgs: Vec<PhotonMessage> = events
            .iter()
            .map(encode_telemetry_event_to_photon)
            .collect();
        let udp = build_photon_udp_packet(&photon_msgs, true, false, 1);

        let mut parser = PhotonParser::new();
        let decoded: Vec<AlbionTelemetryEvent> = parser
            .parse_udp_payload(&udp)
            .iter()
            .filter_map(decode_photon_message)
            .collect();

        assert_eq!(decoded, events);
    }
}
