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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_sprite: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecipeIngredient {
    pub unique_name: String,
    pub name_fr: String,
    pub count: u32,
    pub is_returnable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CraftingRecipe {
    pub item_unique_name: String,
    pub amount_crafted: u32,
    pub crafting_focus: u32,
    pub item_value: i64,
    pub ingredients: Vec<RecipeIngredient>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawCatalog {
    pub version: String,
    pub items: Vec<ItemEntry>,
    pub spells: Vec<SpellEntry>,
    pub clusters: Vec<ClusterEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recipes: Vec<CraftingRecipe>,
}

type CompactRecipeTuple = (u32, u32, i64, Vec<(usize, u32, u8)>);

#[derive(Debug, Deserialize)]
struct CompactRecipesDoc {
    materials: Vec<(String, String)>,
    recipes: HashMap<String, CompactRecipeTuple>,
}

/// Fast `O(1)` lookup catalog for items, spells, clusters, recipes, and weapon inference.
#[derive(Debug, Clone)]
pub struct Catalog {
    pub version: String,
    items_by_index: HashMap<i32, ItemEntry>,
    items_by_unique: HashMap<String, i32>,
    spells_by_index: HashMap<i32, SpellEntry>,
    weapon_by_spell: HashMap<i32, i32>,
    clusters_by_index: HashMap<String, ClusterEntry>,
    recipes_by_unique: HashMap<String, CraftingRecipe>,
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

        let mut recipes_by_unique = HashMap::with_capacity(raw.recipes.len());
        for rec in raw.recipes {
            recipes_by_unique.insert(rec.item_unique_name.clone(), rec);
        }

        Self {
            version: raw.version,
            items_by_index,
            items_by_unique,
            spells_by_index,
            weapon_by_spell,
            clusters_by_index,
            recipes_by_unique,
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

    pub fn recipe_count(&self) -> usize {
        self.recipes_by_unique.len()
    }

    /// Looks up the exact crafting recipe for an item `UniqueName` (e.g. `T8_2H_BOW_KEEPER@2`),
    /// enriching ingredient French labels from the live item catalog when available.
    pub fn recipe(&self, unique_name: &str) -> Option<CraftingRecipe> {
        let key = unique_name.trim();
        let mut rec = self.recipes_by_unique.get(key)?.clone();
        for ing in &mut rec.ingredients {
            if let Some(item) = self.item_by_unique_name(&ing.unique_name) {
                if !item.name_fr.trim().is_empty() && item.name_fr != ing.unique_name {
                    ing.name_fr = item.name_fr.clone();
                }
            }
        }
        Some(rec)
    }

    /// Loads the compact string-interned `data/recipes.json` dictionary into `recipes_by_unique`.
    /// Existing recipes (for example from a newer `items.xml` sync) are preserved unless `overwrite` is set.
    pub fn load_compact_recipes_json(
        &mut self,
        json_text: &str,
        overwrite: bool,
    ) -> Result<usize, String> {
        let doc: CompactRecipesDoc = serde_json::from_str(json_text).map_err(|e| e.to_string())?;
        let mut loaded = 0usize;
        for (uid, (amount_crafted, crafting_focus, item_value, raw_ings)) in doc.recipes {
            if !overwrite && self.recipes_by_unique.contains_key(&uid) {
                continue;
            }
            let mut ingredients = Vec::with_capacity(raw_ings.len());
            for (mat_idx, count, ret_flag) in raw_ings {
                let Some((mat_uid, mat_fr)) = doc.materials.get(mat_idx) else {
                    continue;
                };
                if count == 0 || mat_uid.is_empty() {
                    continue;
                }
                ingredients.push(RecipeIngredient {
                    unique_name: mat_uid.clone(),
                    name_fr: mat_fr.clone(),
                    count,
                    is_returnable: ret_flag != 0,
                });
            }
            if ingredients.is_empty() {
                continue;
            }
            if item_value > 0 {
                if let Some(&idx) = self.items_by_unique.get(&uid) {
                    if let Some(entry) = self.items_by_index.get_mut(&idx) {
                        entry.item_value = item_value;
                    }
                }
            }
            self.recipes_by_unique.insert(
                uid.clone(),
                CraftingRecipe {
                    item_unique_name: uid,
                    amount_crafted: amount_crafted.max(1),
                    crafting_focus,
                    item_value,
                    ingredients,
                },
            );
            loaded += 1;
        }
        Ok(loaded)
    }

    /// Exports the full catalog (including merged `ao-bin-dumps` items, spells, clusters, and recipes) for disk caching.
    pub fn export_raw(&self) -> RawCatalog {
        let mut items: Vec<ItemEntry> = self.items_by_index.values().cloned().collect();
        items.sort_by_key(|i| i.index);
        let mut spells: Vec<SpellEntry> = self.spells_by_index.values().cloned().collect();
        spells.sort_by_key(|s| s.index);
        let mut clusters: Vec<ClusterEntry> = self.clusters_by_index.values().cloned().collect();
        clusters.sort_by(|a, b| a.index.cmp(&b.index));
        let mut recipes: Vec<CraftingRecipe> = self.recipes_by_unique.values().cloned().collect();
        recipes.sort_by(|a, b| a.item_unique_name.cmp(&b.item_unique_name));
        RawCatalog {
            version: self.version.clone(),
            items,
            spells,
            clusters,
            recipes,
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
            let ui_sprite = extract_xml_attr(open_tag_slice, "uisprite").filter(|s| !s.is_empty());
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
                        ui_sprite: ui_sprite.clone(),
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
                                ui_sprite,
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

    /// Parses crafting recipes (`<craftingrequirements>`, `<enchantment>`, `<craftresource>`)
    /// and recursive nutrition `ItemValue`s from `ao-bin-dumps/items.xml`.
    pub fn merge_ao_bin_dumps_items_xml(&mut self, xml_text: &str) -> Result<usize, String> {
        let clean = strip_xml_comments(xml_text);
        let Some(items_open) = clean.find("<items") else {
            return Err("Format items.xml invalide (balise <items> absente)".into());
        };
        let Some(items_gt) = clean[items_open..].find('>') else {
            return Err("Balise <items> mal formée".into());
        };

        let mut pos = items_open + items_gt + 1;
        let bytes = clean.as_bytes();
        let len = bytes.len();

        let mut explicit_iv: HashMap<String, f64> = HashMap::new();
        let mut raw_recipes: HashMap<String, ParsedRawRecipe> = HashMap::new();

        while let Some(rel_lt) = clean[pos..].find('<') {
            let lt = pos + rel_lt;
            if lt + 2 >= len {
                break;
            }
            let next_b = bytes[lt + 1];
            if next_b == b'/' {
                if clean[lt..].starts_with("</items>") {
                    break;
                }
                pos = lt + 2;
                continue;
            }
            if !next_b.is_ascii_alphabetic() {
                pos = lt + 2;
                continue;
            }

            let mut name_end = lt + 1;
            while name_end < len
                && (bytes[name_end].is_ascii_alphanumeric()
                    || bytes[name_end] == b'_'
                    || bytes[name_end] == b'-')
            {
                name_end += 1;
            }
            let tag_name = &clean[lt + 1..name_end];
            let Some(rel_gt) = clean[name_end..].find('>') else {
                break;
            };
            let gt = name_end + rel_gt;
            let open_slice = &clean[name_end..gt];
            let self_closing = open_slice.trim_end().ends_with('/');
            let after_open = gt + 1;

            if tag_name == "shopcategories" {
                if self_closing {
                    pos = after_open;
                } else if let Some(rel_close) = clean[after_open..].find("</shopcategories>") {
                    pos = after_open + rel_close + "</shopcategories>".len();
                } else {
                    pos = after_open;
                }
                continue;
            }

            let unique_name = extract_xml_attr(open_slice, "uniquename").unwrap_or_default();
            if let Some(iv_str) = extract_xml_attr(open_slice, "itemvalue") {
                if let Ok(iv) = iv_str.parse::<f64>() {
                    if !unique_name.is_empty() {
                        explicit_iv.insert(unique_name.clone(), iv);
                    }
                }
            }

            if self_closing {
                pos = after_open;
                continue;
            }

            let close_pat = format!("</{tag_name}>");
            let Some(rel_close) = clean[after_open..].find(&close_pat) else {
                pos = after_open;
                continue;
            };
            let body = &clean[after_open..after_open + rel_close];
            pos = after_open + rel_close + close_pat.len();

            if unique_name.is_empty() || unique_name.starts_with("UNIQUE_") {
                continue;
            }

            let (base_body, ench_body) = if let Some(ench_pos) = body.find("<enchantments") {
                let end_ench = body[ench_pos..]
                    .find("</enchantments>")
                    .map(|e| ench_pos + e + "</enchantments>".len())
                    .unwrap_or(body.len());
                (&body[..ench_pos], Some(&body[ench_pos..end_ench]))
            } else {
                (body, None)
            };

            if let Some(rec) = extract_first_crafting_req(base_body) {
                raw_recipes.insert(unique_name.clone(), rec);
            }

            if let Some(enchs) = ench_body {
                let mut epos = 0usize;
                while let Some(rel_e) = enchs[epos..].find("<enchantment ") {
                    let e_start = epos + rel_e;
                    let Some(rel_egt) = enchs[e_start..].find('>') else {
                        break;
                    };
                    let e_gt = e_start + rel_egt;
                    let e_attrs = &enchs[e_start..e_gt];
                    let e_self = e_attrs.trim_end().ends_with('/');
                    let after_e = e_gt + 1;
                    let lvl = extract_xml_attr(e_attrs, "enchantmentlevel").unwrap_or_default();
                    let full_uid = format!("{unique_name}@{lvl}");

                    if !lvl.is_empty() {
                        if let Some(iv_str) = extract_xml_attr(e_attrs, "itemvalue") {
                            if let Ok(iv) = iv_str.parse::<f64>() {
                                explicit_iv.insert(full_uid.clone(), iv);
                            }
                        }
                    }

                    if e_self {
                        epos = after_e;
                        continue;
                    }
                    if let Some(rel_eclose) = enchs[after_e..].find("</enchantment>") {
                        let inner_e = &enchs[after_e..after_e + rel_eclose];
                        if !lvl.is_empty() {
                            if let Some(erec) = extract_first_crafting_req(inner_e) {
                                raw_recipes.insert(full_uid, erec);
                            }
                        }
                        epos = after_e + rel_eclose + "</enchantment>".len();
                    } else {
                        epos = after_e;
                    }
                }
            }
        }

        let mut memo_iv: HashMap<String, f64> = HashMap::new();
        let mut visiting: std::collections::HashSet<String> = std::collections::HashSet::new();
        let keys: Vec<String> = raw_recipes.keys().cloned().collect();
        let mut upserted = 0usize;

        for uid in keys {
            let iv = resolve_recipe_item_value(
                &uid,
                &explicit_iv,
                &raw_recipes,
                &mut memo_iv,
                &mut visiting,
            )
            .round() as i64;
            let Some((amount_crafted, crafting_focus, raw_ings)) = raw_recipes.get(&uid) else {
                continue;
            };
            let existing_fr: HashMap<String, String> = self
                .recipes_by_unique
                .get(&uid)
                .map(|r| {
                    r.ingredients
                        .iter()
                        .map(|i| (i.unique_name.clone(), i.name_fr.clone()))
                        .collect()
                })
                .unwrap_or_default();

            let ingredients: Vec<RecipeIngredient> = raw_ings
                .iter()
                .map(|(mat_uid, count, is_ret)| {
                    let name_fr = self
                        .item_by_unique_name(mat_uid)
                        .map(|it| it.name_fr.clone())
                        .or_else(|| existing_fr.get(mat_uid).cloned())
                        .unwrap_or_else(|| mat_uid.clone());
                    RecipeIngredient {
                        unique_name: mat_uid.clone(),
                        name_fr,
                        count: *count,
                        is_returnable: *is_ret,
                    }
                })
                .collect();

            if iv > 0 {
                if let Some(&idx) = self.items_by_unique.get(&uid) {
                    if let Some(entry) = self.items_by_index.get_mut(&idx) {
                        entry.item_value = iv;
                    }
                }
            }

            self.recipes_by_unique.insert(
                uid.clone(),
                CraftingRecipe {
                    item_unique_name: uid,
                    amount_crafted: (*amount_crafted).max(1),
                    crafting_focus: *crafting_focus,
                    item_value: iv,
                    ingredients,
                },
            );
            upserted += 1;
        }

        Ok(upserted)
    }
}

type ParsedRawRecipe = (u32, u32, Vec<(String, u32, bool)>);

fn is_ingredient_returnable(unique_name: &str, max_return_attr: Option<&str>) -> bool {
    if max_return_attr == Some("0") {
        return false;
    }
    let up = unique_name.to_ascii_uppercase();
    !(up.contains("FISHSAUCE")
        || up.contains("ALCHEMY_EXTRACT")
        || up.contains("_ARTEFACT_")
        || up.contains("_FACTION_")
        || up.starts_with("QUESTITEM_TOKEN_"))
}

fn extract_first_crafting_req(section: &str) -> Option<ParsedRawRecipe> {
    let mut pos = 0usize;
    while let Some(rel_cr) = section[pos..].find("<craftingrequirements") {
        let cr_start = pos + rel_cr;
        let after_tag = cr_start + "<craftingrequirements".len();
        let next_ch = section.as_bytes().get(after_tag).copied().unwrap_or(b' ');
        if !next_ch.is_ascii_whitespace() && next_ch != b'>' && next_ch != b'/' {
            pos = after_tag;
            continue;
        }
        let rel_gt = section[after_tag..].find('>')?;
        let gt = after_tag + rel_gt;
        let attrs = &section[after_tag..gt];
        let self_closing = attrs.trim_end().ends_with('/');
        let after_open = gt + 1;

        if self_closing {
            pos = after_open;
            continue;
        }
        let Some(rel_close) = section[after_open..].find("</craftingrequirements>") else {
            break;
        };
        let inner = &section[after_open..after_open + rel_close];
        pos = after_open + rel_close + "</craftingrequirements>".len();

        if extract_xml_attr(attrs, "swaptransaction").as_deref() == Some("true") {
            continue;
        }

        let mut mats = Vec::new();
        let mut rpos = 0usize;
        while let Some(rel_r) = inner[rpos..].find("<craftresource") {
            let r_start = rpos + rel_r;
            let Some(rel_rgt) = inner[r_start..].find('>') else {
                break;
            };
            let r_gt = r_start + rel_rgt;
            let r_attrs = &inner[r_start..r_gt];
            rpos = r_gt + 1;

            let ru = extract_xml_attr(r_attrs, "uniquename").unwrap_or_default();
            let count = extract_xml_attr(r_attrs, "count")
                .and_then(|s| s.parse::<f64>().ok())
                .map(|v| v as u32)
                .unwrap_or(0);
            if ru.is_empty() || count == 0 {
                continue;
            }
            let ench = extract_xml_attr(r_attrs, "enchantmentlevel")
                .and_then(|s| s.parse::<u8>().ok())
                .unwrap_or(0);
            let full_ru = if ench > 0 && !ru.contains('@') {
                format!("{ru}@{ench}")
            } else {
                ru
            };
            let max_ret = extract_xml_attr(r_attrs, "maxreturnamount");
            let is_ret = is_ingredient_returnable(&full_ru, max_ret.as_deref());
            mats.push((full_ru, count, is_ret));
        }

        if !mats.is_empty() {
            let amt = extract_xml_attr(attrs, "amountcrafted")
                .and_then(|s| s.parse::<f64>().ok())
                .map(|v| (v as u32).max(1))
                .unwrap_or(1);
            let focus = extract_xml_attr(attrs, "craftingfocus")
                .and_then(|s| s.parse::<f64>().ok())
                .map(|v| v.round() as u32)
                .unwrap_or(0);
            return Some((amt, focus, mats));
        }
    }
    None
}

fn lookup_explicit_iv(uid: &str, explicit_iv: &HashMap<String, f64>) -> Option<f64> {
    if let Some(&v) = explicit_iv.get(uid) {
        return Some(v);
    }
    if let Some((base, _)) = uid.split_once('@') {
        if let Some(&v) = explicit_iv.get(base) {
            return Some(v);
        }
    }
    None
}

fn resolve_recipe_item_value(
    uid: &str,
    explicit_iv: &HashMap<String, f64>,
    raw_recipes: &HashMap<String, ParsedRawRecipe>,
    memo: &mut HashMap<String, f64>,
    visiting: &mut std::collections::HashSet<String>,
) -> f64 {
    if let Some(&cached) = memo.get(uid) {
        return cached;
    }
    if let Some(ex) = lookup_explicit_iv(uid, explicit_iv) {
        memo.insert(uid.to_string(), ex);
        return ex;
    }
    if visiting.contains(uid) {
        return 0.0;
    }
    let Some((amt, _focus, mats)) = raw_recipes.get(uid) else {
        return 0.0;
    };
    visiting.insert(uid.to_string());
    let mut ret_sum = 0.0;
    for (mu, cnt, is_ret) in mats {
        if *is_ret {
            ret_sum += (*cnt as f64)
                * resolve_recipe_item_value(mu, explicit_iv, raw_recipes, memo, visiting);
        }
    }
    if ret_sum <= 0.0 {
        for (mu, cnt, _) in mats {
            ret_sum += (*cnt as f64)
                * resolve_recipe_item_value(mu, explicit_iv, raw_recipes, memo, visiting);
        }
    }
    visiting.remove(uid);
    let val = ret_sum / ((*amt).max(1) as f64);
    memo.insert(uid.to_string(), val);
    val
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
    const EMBEDDED_RECIPES: &str = include_str!("../../../data/recipes.json");

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
    fn loads_compact_recipes_and_resolves_enchanted_artefacts_and_consumables() {
        let mut catalog = Catalog::from_json_str(EMBEDDED).unwrap();
        let count = catalog
            .load_compact_recipes_json(EMBEDDED_RECIPES, false)
            .unwrap();
        assert!(count > 8_500);

        let cleaver = catalog.recipe("T7_2H_CLEAVER_HELL@1").unwrap();
        assert_eq!(cleaver.amount_crafted, 1);
        assert_eq!(cleaver.item_value, 8_192);
        assert_eq!(cleaver.ingredients.len(), 3);
        assert_eq!(cleaver.ingredients[0].unique_name, "T7_METALBAR_LEVEL1@1");
        assert_eq!(cleaver.ingredients[0].count, 20);
        assert!(cleaver.ingredients[0].is_returnable);
        assert_eq!(cleaver.ingredients[1].unique_name, "T7_LEATHER_LEVEL1@1");
        assert_eq!(cleaver.ingredients[1].count, 12);
        assert!(cleaver.ingredients[1].is_returnable);
        assert_eq!(
            cleaver.ingredients[2].unique_name,
            "T7_ARTEFACT_2H_CLEAVER_HELL"
        );
        assert_eq!(cleaver.ingredients[2].count, 1);
        assert!(!cleaver.ingredients[2].is_returnable);

        let stew = catalog.recipe("T8_MEAL_STEW@1").unwrap();
        assert_eq!(stew.amount_crafted, 10);
        let sauce = stew
            .ingredients
            .iter()
            .find(|i| i.unique_name == "T1_FISHSAUCE_LEVEL1")
            .unwrap();
        assert!(!sauce.is_returnable);
    }

    #[test]
    fn parses_items_xml_recipes_and_recursive_item_values() {
        let mut catalog = Catalog::from_json_str(EMBEDDED).unwrap();
        let items_xml = r#"<items>
            <simpleitem uniquename="T8_PLANKS_LEVEL2" itemvalue="1024" />
            <simpleitem uniquename="T8_ARTEFACT_2H_BOW_KEEPER" itemvalue="14336" />
            <weapon uniquename="T8_2H_BOW_KEEPER" tier="8">
                <craftingrequirements silver="0" time="5" craftingfocus="16085">
                    <craftresource uniquename="T8_PLANKS" count="32" />
                    <craftresource uniquename="T8_ARTEFACT_2H_BOW_KEEPER" count="1" maxreturnamount="0" />
                </craftingrequirements>
                <enchantments>
                    <enchantment enchantmentlevel="2" itempower="1375">
                        <craftingrequirements silver="0" time="6" craftingfocus="49260">
                            <craftresource uniquename="T8_PLANKS_LEVEL2" enchantmentlevel="2" count="32" />
                            <craftresource uniquename="T8_ARTEFACT_2H_BOW_KEEPER" count="1" maxreturnamount="0" />
                        </craftingrequirements>
                        <craftingrequirements time="6" craftingfocus="49260">
                            <craftresource uniquename="T8_PLANKS_LEVEL2" enchantmentlevel="2" count="32" />
                            <craftresource uniquename="T8_ARTEFACT_TOKEN_FAVOR_2" count="1" maxreturnamount="0" />
                        </craftingrequirements>
                    </enchantment>
                </enchantments>
            </weapon>
        </items>"#;
        let merged = catalog.merge_ao_bin_dumps_items_xml(items_xml).unwrap();
        assert_eq!(merged, 2);

        let bow = catalog.recipe("T8_2H_BOW_KEEPER@2").unwrap();
        assert_eq!(bow.amount_crafted, 1);
        assert_eq!(bow.crafting_focus, 49_260);
        assert_eq!(bow.item_value, 32_768);
        assert_eq!(bow.ingredients[0].unique_name, "T8_PLANKS_LEVEL2@2");
        assert!(bow.ingredients[0].is_returnable);
        assert_eq!(bow.ingredients[1].unique_name, "T8_ARTEFACT_2H_BOW_KEEPER");
        assert!(!bow.ingredients[1].is_returnable);
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
            <activespell uniquename="FLAME_PILLAR" uisprite="FIRESTAFF_FLAMEPILLAR" category="damage">
                <channelingspell effectcount="3" />
            </activespell>
            <togglespell uniquename="TOGGLE_AURA" category="heal" />
        </spells>"#;
        let spells_added = catalog.merge_ao_bin_dumps_spells_xml(spells_xml).unwrap();
        assert_eq!(spells_added, 3);
        assert_eq!(catalog.spell(0).unwrap().unique_name, "AUTO_ATTACK");
        assert_eq!(catalog.spell(1).unwrap().unique_name, "FLAME_PILLAR");
        assert_eq!(
            catalog.spell(1).unwrap().ui_sprite.as_deref(),
            Some("FIRESTAFF_FLAMEPILLAR")
        );
        assert_eq!(catalog.spell(1).unwrap().name_fr, "Flame Pillar");
        assert_eq!(catalog.spell(2).unwrap().name_fr, "Flame Pillar (Canal.)");
        assert_eq!(
            catalog.spell(2).unwrap().ui_sprite.as_deref(),
            Some("FIRESTAFF_FLAMEPILLAR")
        );
        assert_eq!(catalog.spell(3).unwrap().unique_name, "TOGGLE_AURA");
        assert_eq!(catalog.spell(3).unwrap().category, "heal");

        let exported = catalog.export_raw();
        let reloaded = Catalog::from_raw(exported);
        assert_eq!(reloaded.item_count(), catalog.item_count());
        assert_eq!(reloaded.spell_count(), catalog.spell_count());
        assert_eq!(reloaded.cluster_count(), catalog.cluster_count());
    }
}
