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

/// Shape of entries inside `ao-bin-dumps/formatted/world.json`.
#[derive(Debug, Deserialize)]
struct AoBinDumpWorld {
    #[serde(rename = "Index")]
    index: String,
    #[serde(rename = "UniqueName")]
    unique_name: String,
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

    /// Searches catalog items by French name, English name, or `UniqueName` (multi-token, case-insensitive).
    pub fn search_items(&self, query: &str, limit: usize) -> Vec<&ItemEntry> {
        let cap = limit.clamp(1, 100);
        let trimmed = query.trim().to_lowercase();
        if trimmed.is_empty() {
            return self.all_items().into_iter().take(cap).collect();
        }
        let tokens: Vec<&str> = trimmed.split_whitespace().collect();
        let mut scored: Vec<(i32, i32, &ItemEntry)> = Vec::new();

        for item in self.items_by_index.values() {
            let u_low = item.unique_name.to_lowercase();
            let fr_low = item.name_fr.to_lowercase();
            let en_low = item.name_en.to_lowercase();

            let all_match = tokens
                .iter()
                .all(|tok| fr_low.contains(tok) || en_low.contains(tok) || u_low.contains(tok));
            if !all_match {
                continue;
            }

            let score = if u_low == trimmed || fr_low == trimmed || en_low == trimmed {
                0
            } else if fr_low.starts_with(&trimmed)
                || en_low.starts_with(&trimmed)
                || u_low.starts_with(&trimmed)
            {
                1
            } else {
                2
            };
            scored.push((score, -(item.tier as i32), item));
        }

        scored.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.index.cmp(&b.2.index))
        });
        scored.into_iter().take(cap).map(|(_, _, it)| it).collect()
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

    /// Exports the full catalog (including merged `ao-bin-dumps` items, spells, and clusters) for disk caching.
    pub fn export_raw(&self) -> RawCatalog {
        let mut items: Vec<ItemEntry> = self.items_by_index.values().cloned().collect();
        items.sort_by_key(|i| i.index);
        let mut spells: Vec<SpellEntry> = self.spells_by_index.values().cloned().collect();
        spells.sort_by_key(|s| s.index);
        let mut clusters: Vec<ClusterEntry> = self.clusters_by_index.values().cloned().collect();
        clusters.sort_by(|a, b| a.index.cmp(&b.index));
        RawCatalog {
            version: self.version.clone(),
            items,
            spells,
            clusters,
        }
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

    /// Merges world clusters from the public `ao-bin-dumps` `formatted/world.json` file.
    pub fn merge_ao_bin_dumps_world(&mut self, json_text: &str) -> Result<usize, String> {
        let entries: Vec<AoBinDumpWorld> =
            serde_json::from_str(json_text).map_err(|e| e.to_string())?;
        let mut added = 0usize;
        for row in entries {
            let idx = row.index.trim();
            let name = row.unique_name.trim();
            if idx.is_empty() || name.is_empty() || self.clusters_by_index.contains_key(idx) {
                continue;
            }
            let kind = if idx.contains("MISTS") || name.to_uppercase().contains("MIST") {
                "mists"
            } else if idx.contains("HELLGATE") {
                "hellgate"
            } else if idx.contains("CORRUPTED") {
                "corruptedDungeon"
            } else if idx.starts_with("RD_") {
                "soloDungeon"
            } else {
                "openWorld"
            };
            let cluster = ClusterEntry {
                index: idx.to_string(),
                name_fr: name.to_string(),
                name_en: name.to_string(),
                kind: kind.into(),
                tier: 6,
            };
            self.clusters_by_index.insert(idx.to_string(), cluster);
            added += 1;
        }
        Ok(added)
    }

    /// Merges spells from `ao-bin-dumps/spells.xml` in exact document index order
    /// (`passivespell`, `activespell` + nested `<channelingspell>`, `togglespell`).
    pub fn merge_ao_bin_dumps_spells_xml(&mut self, xml_text: &str) -> Result<usize, String> {
        let clean = strip_xml_comments(xml_text);
        if !clean.contains("<spells") {
            return Err("Format spells.xml invalide (balise <spells> absente)".into());
        }

        let mut added = 0usize;
        let mut spell_index: i32 = 0;
        let bytes = clean.as_bytes();
        let mut pos = 0usize;

        while let Some(rel_lt) = clean[pos..].find('<') {
            let tag_start = pos + rel_lt;
            let rest = &clean[tag_start + 1..];

            let matched_tag = if rest.starts_with("passivespell")
                && rest
                    .as_bytes()
                    .get(12)
                    .is_some_and(|b| b.is_ascii_whitespace())
            {
                Some(("passivespell", 12usize))
            } else if rest.starts_with("activespell")
                && rest
                    .as_bytes()
                    .get(11)
                    .is_some_and(|b| b.is_ascii_whitespace())
            {
                Some(("activespell", 11usize))
            } else if rest.starts_with("togglespell")
                && rest
                    .as_bytes()
                    .get(11)
                    .is_some_and(|b| b.is_ascii_whitespace())
            {
                Some(("togglespell", 11usize))
            } else {
                None
            };

            let Some((tag_name, tag_len)) = matched_tag else {
                pos = tag_start + 1;
                if pos >= bytes.len() {
                    break;
                }
                continue;
            };

            let Some(rel_gt) = rest.find('>') else {
                break;
            };
            let open_tag_slice = &rest[tag_len..rel_gt];
            let self_closing = open_tag_slice.trim_end().ends_with('/');

            let unique_name = extract_xml_attr(open_tag_slice, "uniquename").unwrap_or_default();
            let raw_cat = extract_xml_attr(open_tag_slice, "category")
                .or_else(|| extract_xml_attr(open_tag_slice, "uitype"))
                .unwrap_or_else(|| "damage".into());

            let cur_idx = spell_index;
            spell_index += 1;

            if !unique_name.is_empty() && !self.spells_by_index.contains_key(&cur_idx) {
                let readable = humanize_spell_identifier(&unique_name);
                self.spells_by_index.insert(
                    cur_idx,
                    SpellEntry {
                        index: cur_idx,
                        unique_name: unique_name.clone(),
                        name_fr: readable.clone(),
                        name_en: readable,
                        category: normalize_spell_category(&raw_cat),
                    },
                );
                added += 1;
            }

            let after_open = tag_start + 1 + rel_gt + 1;
            if self_closing {
                pos = after_open;
                continue;
            }

            let close_pat = format!("</{tag_name}>");
            if let Some(rel_close) = clean[after_open..].find(&close_pat) {
                let inner = &clean[after_open..after_open + rel_close];
                if tag_name == "activespell" && inner.contains("<channelingspell") {
                    let chan_idx = spell_index;
                    spell_index += 1;
                    if !unique_name.is_empty() && !self.spells_by_index.contains_key(&chan_idx) {
                        let readable = humanize_spell_identifier(&unique_name);
                        self.spells_by_index.insert(
                            chan_idx,
                            SpellEntry {
                                index: chan_idx,
                                unique_name: unique_name.clone(),
                                name_fr: format!("{readable} (Canal.)"),
                                name_en: format!("{readable} (Channel)"),
                                category: normalize_spell_category(&raw_cat),
                            },
                        );
                        added += 1;
                    }
                }
                pos = after_open + rel_close + close_pat.len();
            } else {
                pos = after_open;
            }
        }

        Ok(added)
    }
}

fn strip_xml_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        if let Some(end) = rest[start + 4..].find("-->") {
            rest = &rest[start + 4 + end + 3..];
        } else {
            return out;
        }
    }
    out.push_str(rest);
    out
}

fn extract_xml_attr(attrs: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = attrs.find(&needle)? + needle.len();
    let end = attrs[start..].find('"')?;
    Some(attrs[start..start + end].trim().to_string())
}

fn humanize_spell_identifier(unique_name: &str) -> String {
    unique_name
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => {
                    let mut word = String::new();
                    word.extend(first.to_uppercase());
                    word.push_str(&chars.as_str().to_lowercase());
                    word
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_spell_category(raw: &str) -> String {
    let low = raw.to_ascii_lowercase();
    if low.contains("heal") {
        "heal".into()
    } else if low.contains("buff") || low.contains("defens") || low.contains("mobility") {
        "buff".into()
    } else {
        "damage".into()
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
    fn searches_items_by_french_english_and_unique_name() {
        let catalog = Catalog::from_json_str(EMBEDDED).unwrap();
        let by_fr = catalog.search_items("lames sculptées", 10);
        assert!(!by_fr.is_empty());
        assert_eq!(by_fr[0].unique_name, "T7_2H_CLEAVER_HELL@1");

        let by_en = catalog.search_items("hallowfall", 10);
        assert!(!by_en.is_empty());
        assert_eq!(by_en[0].unique_name, "T8_MAIN_HOLYSTAFF_AVALON@2");

        let by_id = catalog.search_items("T8_BAG_INSIGHT", 5);
        assert_eq!(by_id[0].unique_name, "T8_BAG_INSIGHT");
    }

    #[test]
    fn merges_ao_bin_dumps_formatted_items_world_and_spells_xml() {
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

        let world_sample = r#"[
            { "Index": "MISTS_999", "UniqueName": "Lethal Greater Mists" }
        ]"#;
        let world_added = catalog.merge_ao_bin_dumps_world(world_sample).unwrap();
        assert_eq!(world_added, 1);
        let cluster = catalog.cluster("MISTS_999").unwrap();
        assert_eq!(cluster.kind, "mists");

        // Index 0 already exists (AUTO_ATTACK), so index 1..=3 should be added including channeling slot
        let spells_xml = r#"<spells>
            <!-- <activespell uniquename="COMMENTED_OUT" /> -->
            <colortag tag="[dmg]" />
            <passivespell uniquename="PASSIVE_MAXLOAD" />
            <activespell uniquename="FLAME_PILLAR" category="damage">
                <channelingspell effectcount="3" />
            </activespell>
            <togglespell uniquename="TOGGLE_AURA" category="heal" />
        </spells>"#;
        let spells_added = catalog.merge_ao_bin_dumps_spells_xml(spells_xml).unwrap();
        assert_eq!(spells_added, 3);
        assert_eq!(catalog.spell(0).unwrap().unique_name, "AUTO_ATTACK");
        assert_eq!(catalog.spell(1).unwrap().unique_name, "FLAME_PILLAR");
        assert_eq!(catalog.spell(1).unwrap().name_fr, "Flame Pillar");
        assert_eq!(catalog.spell(2).unwrap().name_fr, "Flame Pillar (Canal.)");
        assert_eq!(catalog.spell(3).unwrap().unique_name, "TOGGLE_AURA");
        assert_eq!(catalog.spell(3).unwrap().category, "heal");

        let exported = catalog.export_raw();
        let reloaded = Catalog::from_raw(exported);
        assert_eq!(reloaded.item_count(), catalog.item_count());
        assert_eq!(reloaded.spell_count(), catalog.spell_count());
        assert_eq!(reloaded.cluster_count(), catalog.cluster_count());
    }
}
