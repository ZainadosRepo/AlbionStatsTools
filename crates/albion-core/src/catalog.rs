//! Indexed catalog of Albion Online items, combat spells, and world clusters,
//! plus importer for public `ao-bin-dumps` (`formatted/items.json`) metadata.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemEntry {
    pub index: i32,
    pub unique_name: String,
    pub name_fr: String,
    pub name_en: String,
    pub slot: String,
    pub tier: u8,
    pub enchantment: u8,
    pub item_value: i64,
    #[serde(default)]
    pub spell_ids: Vec<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpellEntry {
    pub index: i32,
    pub unique_name: String,
    pub name_fr: String,
    pub name_en: String,
    pub category: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterEntry {
    pub index: String,
    pub name_fr: String,
    pub name_en: String,
    pub kind: String,
    pub tier: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawCatalog {
    pub version: String,
    pub items: Vec<ItemEntry>,
    pub spells: Vec<SpellEntry>,
    pub clusters: Vec<ClusterEntry>,
}

/// Fast `O(1)` lookup catalog for items, spells, clusters, and weapon inference.
#[derive(Debug, Clone)]
pub struct Catalog {
    pub version: String,
    items_by_index: HashMap<i32, ItemEntry>,
    items_by_unique: HashMap<String, i32>,
    spells_by_index: HashMap<i32, SpellEntry>,
    weapon_by_spell: HashMap<i32, i32>,
    clusters_by_index: HashMap<String, ClusterEntry>,
}

/// Shape of entries inside `ao-bin-dumps/formatted/items.json`.
#[derive(Debug, Deserialize)]
struct AoBinDumpItem {
    #[serde(rename = "Index")]
    index: String,
    #[serde(rename = "UniqueName")]
    unique_name: String,
    #[serde(rename = "LocalizedNames", default)]
    localized_names: Option<HashMap<String, String>>,
}

impl Catalog {
    pub fn from_json_str(json: &str) -> Result<Self, String> {
        let raw: RawCatalog = serde_json::from_str(json).map_err(|e| e.to_string())?;
        Ok(Self::from_raw(raw))
    }

    pub fn from_raw(raw: RawCatalog) -> Self {
        let mut items_by_index = HashMap::with_capacity(raw.items.len());
        let mut items_by_unique = HashMap::with_capacity(raw.items.len());
        let mut weapon_by_spell = HashMap::new();

        for item in raw.items {
            if item.slot == "mainHand" {
                for &sp in &item.spell_ids {
                    weapon_by_spell.entry(sp).or_insert(item.index);
                }
            }
            items_by_unique.insert(item.unique_name.clone(), item.index);
            items_by_index.insert(item.index, item);
        }

        let mut spells_by_index = HashMap::with_capacity(raw.spells.len());
        for spell in raw.spells {
            spells_by_index.insert(spell.index, spell);
        }

        let mut clusters_by_index = HashMap::with_capacity(raw.clusters.len());
        for cluster in raw.clusters {
            clusters_by_index.insert(cluster.index.clone(), cluster);
        }

        Self {
            version: raw.version,
            items_by_index,
            items_by_unique,
            spells_by_index,
            weapon_by_spell,
            clusters_by_index,
        }
    }

    pub fn item(&self, index: i32) -> Option<&ItemEntry> {
        self.items_by_index.get(&index)
    }

    pub fn item_by_unique_name(&self, unique_name: &str) -> Option<&ItemEntry> {
        let idx = self.items_by_unique.get(unique_name)?;
        self.items_by_index.get(idx)
    }

    pub fn all_items(&self) -> Vec<&ItemEntry> {
        let mut list: Vec<&ItemEntry> = self.items_by_index.values().collect();
        list.sort_by_key(|i| i.index);
        list
    }

    pub fn spell(&self, index: i32) -> Option<&SpellEntry> {
        if index <= 0 {
            return self.spells_by_index.get(&0);
        }
        self.spells_by_index.get(&index)
    }

    /// Infers a player's main-hand weapon from an observed spell cast (`ActiveSpellEffectsUpdate`)
    /// without needing a full character inspect first.
    pub fn infer_weapon_from_spell(&self, spell_index: i32) -> Option<&ItemEntry> {
        let item_idx = self.weapon_by_spell.get(&spell_index)?;
        self.items_by_index.get(item_idx)
    }

    pub fn cluster(&self, index: &str) -> Option<&ClusterEntry> {
        self.clusters_by_index.get(index)
    }

    pub fn item_count(&self) -> usize {
        self.items_by_index.len()
    }

    pub fn spell_count(&self) -> usize {
        self.spells_by_index.len()
    }

    pub fn cluster_count(&self) -> usize {
        self.clusters_by_index.len()
    }

    /// Merges items from the public `ao-bin-dumps` `formatted/items.json` file.
    pub fn merge_ao_bin_dumps_items(&mut self, json_text: &str) -> Result<usize, String> {
        let entries: Vec<AoBinDumpItem> =
            serde_json::from_str(json_text).map_err(|e| e.to_string())?;
        let mut added = 0usize;
        for row in entries {
            let Ok(index) = row.index.parse::<i32>() else {
                continue;
            };
            if row.unique_name.is_empty() || self.items_by_index.contains_key(&index) {
                continue;
            }
            let names = row.localized_names.unwrap_or_default();
            let name_en = names
                .get("EN-US")
                .cloned()
                .unwrap_or_else(|| row.unique_name.clone());
            let name_fr = names
                .get("FR-FR")
                .cloned()
                .unwrap_or_else(|| name_en.clone());
            let (tier, enchantment) = parse_tier_and_enchant(&row.unique_name);
            let slot = infer_slot_from_unique_name(&row.unique_name);
            let item = ItemEntry {
                index,
                unique_name: row.unique_name.clone(),
                name_fr,
                name_en,
                slot,
                tier,
                enchantment,
                item_value: (tier as i64) * 160 * (1 << enchantment),
                spell_ids: Vec::new(),
            };
            self.items_by_unique.insert(item.unique_name.clone(), index);
            self.items_by_index.insert(index, item);
            added += 1;
        }
        Ok(added)
    }
}

fn parse_tier_and_enchant(unique_name: &str) -> (u8, u8) {
    let tier = if let Some(rest) = unique_name.strip_prefix('T') {
        rest.chars()
            .next()
            .and_then(|c| c.to_digit(10))
            .map(|d| d as u8)
            .unwrap_or(4)
    } else {
        4
    };
    let enchantment = unique_name
        .rsplit_once('@')
        .and_then(|(_, e)| e.parse::<u8>().ok())
        .unwrap_or(0);
    (tier, enchantment)
}

fn infer_slot_from_unique_name(unique_name: &str) -> String {
    if unique_name.contains("_MAIN_") || unique_name.contains("_2H_") {
        "mainHand".into()
    } else if unique_name.contains("_OFF_") {
        "offHand".into()
    } else if unique_name.contains("_HEAD_") {
        "head".into()
    } else if unique_name.contains("_ARMOR_") {
        "chest".into()
    } else if unique_name.contains("_SHOES_") {
        "shoes".into()
    } else if unique_name.contains("_BAG") {
        "bag".into()
    } else if unique_name.contains("_CAPE") {
        "cape".into()
    } else if unique_name.contains("_MOUNT_") {
        "mount".into()
    } else if unique_name.contains("_POTION_") {
        "potion".into()
    } else if unique_name.contains("_MEAL_") {
        "food".into()
    } else if unique_name.contains("_WOOD")
        || unique_name.contains("_ORE")
        || unique_name.contains("_FIBER")
        || unique_name.contains("_HIDE")
        || unique_name.contains("_ROCK")
        || unique_name.contains("_FISH_")
    {
        "resource".into()
    } else {
        "material".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMBEDDED: &str = include_str!("../../../data/catalog.json");

    #[test]
    fn loads_embedded_catalog_and_infers_weapons_from_spells() {
        let catalog = Catalog::from_json_str(EMBEDDED).unwrap();
        assert!(catalog.item_count() >= 45);
        assert!(catalog.spell_count() >= 40);
        assert!(catalog.cluster_count() >= 12);

        let pyroblast_weapon = catalog.infer_weapon_from_spell(2403).unwrap();
        assert_eq!(pyroblast_weapon.unique_name, "T7_MAIN_FIRESTAFF@1");

        let hallowfall = catalog.infer_weapon_from_spell(2704).unwrap();
        assert_eq!(hallowfall.unique_name, "T8_MAIN_HOLYSTAFF_AVALON@2");
    }

    #[test]
    fn merges_ao_bin_dumps_formatted_items() {
        let mut catalog = Catalog::from_json_str(EMBEDDED).unwrap();
        let sample = r#"[
            {
                "Index": "9001",
                "UniqueName": "T8_2H_HALBERD@3",
                "LocalizedNames": {
                    "EN-US": "Elder's Halberd .3",
                    "FR-FR": "Hallebarde du vétéran .3"
                }
            }
        ]"#;
        let added = catalog.merge_ao_bin_dumps_items(sample).unwrap();
        assert_eq!(added, 1);
        let item = catalog.item(9001).unwrap();
        assert_eq!(item.tier, 8);
        assert_eq!(item.enchantment, 3);
        assert_eq!(item.slot, "mainHand");
    }
}
