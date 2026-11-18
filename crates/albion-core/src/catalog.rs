//! Indexed catalog of Albion Online items, combat spells, and world clusters,
//! plus importer for public `ao-bin-dumps` (`formatted/items.json`) metadata.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::OnceLock;

const EMBEDDED_SPELLS_I18N: &str = include_str!("../../../data/spells_i18n.json");

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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weapon_spells: Vec<(i32, i32, u8)>,
}

type CompactRecipeTuple = (u32, u32, i64, Vec<(usize, u32, u8)>);

#[derive(Debug, Deserialize)]
struct CompactRecipesDoc {
    materials: Vec<(String, String)>,
    recipes: HashMap<String, CompactRecipeTuple>,
}

#[derive(Debug, Clone, Default)]
struct RawCraftSpellList {
    subcategory: String,
    reference: Option<String>,
    adds: Vec<String>,
    removes: Vec<String>,
}

/// Fast `O(1)` lookup catalog for items, spells, clusters, recipes, and weapon inference.
#[derive(Debug, Clone)]
pub struct Catalog {
    pub version: String,
    items_by_index: HashMap<i32, ItemEntry>,
    items_by_unique: HashMap<String, i32>,
    spells_by_index: HashMap<i32, SpellEntry>,
    spell_indices_by_unique: HashMap<String, Vec<i32>>,
    weapon_by_spell: HashMap<i32, (i32, u8)>,
    clusters_by_index: HashMap<String, ClusterEntry>,
    recipes_by_unique: HashMap<String, CraftingRecipe>,
    spell_triggers: HashMap<String, Vec<String>>,
    raw_mainhand_csl: HashMap<String, RawCraftSpellList>,
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

fn spells_i18n_map() -> &'static HashMap<String, (String, String)> {
    static MAP: OnceLock<HashMap<String, (String, String)>> = OnceLock::new();
    MAP.get_or_init(|| {
        let parsed: HashMap<String, [String; 2]> =
            serde_json::from_str(EMBEDDED_SPELLS_I18N).unwrap_or_default();
        parsed
            .into_iter()
            .filter_map(|(k, [fr, en])| {
                let fr_t = fr.trim();
                let en_t = en.trim();
                let valid = !fr_t.is_empty()
                    && !en_t.is_empty()
                    && fr_t.len() <= 90
                    && en_t.len() <= 90
                    && !fr_t.contains(['\n', '\r', '$', '{', '}', '[', ']', '§'])
                    && !en_t.contains(['\n', '\r', '$', '{', '}', '[', ']', '§']);
                valid.then(|| (k.to_ascii_uppercase(), (fr_t.to_string(), en_t.to_string())))
            })
            .collect()
    })
}

fn lookup_i18n_with_suffix_strip(
    map: &HashMap<String, (String, String)>,
    ident: &str,
) -> Option<(String, String)> {
    let mut stem = ident.trim().to_ascii_uppercase();
    if stem.is_empty() {
        return None;
    }
    if let Some(pair) = map.get(&format!("@SPELLS_{stem}")) {
        return Some(pair.clone());
    }
    while let Some((prefix, _suffix)) = stem.rsplit_once('_') {
        if prefix.is_empty() {
            break;
        }
        if let Some(pair) = map.get(&format!("@SPELLS_{prefix}")) {
            return Some(pair.clone());
        }
        stem = prefix.to_string();
    }
    None
}

fn resolve_spell_i18n(
    unique_name: &str,
    name_loca_tag: Option<&str>,
    ui_sprite: Option<&str>,
) -> (String, String) {
    let map = spells_i18n_map();

    if let Some(tag) = name_loca_tag {
        let t = tag.trim().to_ascii_uppercase();
        if !t.is_empty() {
            if let Some((fr, en)) = map.get(&t) {
                return (fr.clone(), en.clone());
            }
            if !t.starts_with("@SPELLS_") {
                if let Some((fr, en)) = map.get(&format!("@SPELLS_{t}")) {
                    return (fr.clone(), en.clone());
                }
            }
        }
    }

    let up_name = unique_name.trim().to_ascii_uppercase();
    if let Some((fr, en)) = map.get(&format!("@SPELLS_{up_name}")) {
        return (fr.clone(), en.clone());
    }

    if let Some(sprite) = ui_sprite {
        let up_sprite = sprite.trim().to_ascii_uppercase();
        if !up_sprite.is_empty() {
            if let Some((fr, en)) = map.get(&format!("@SPELLS_{up_sprite}")) {
                return (fr.clone(), en.clone());
            }
        }
    }

    if let Some(pair) = lookup_i18n_with_suffix_strip(map, &up_name) {
        return pair;
    }

    if let Some(sprite) = ui_sprite {
        if let Some(pair) = lookup_i18n_with_suffix_strip(map, sprite) {
            return pair;
        }
    }

    let readable = humanize_spell_identifier(unique_name);
    (readable.clone(), readable)
}

impl Catalog {
    pub fn from_json_str(json: &str) -> Result<Self, String> {
        let raw: RawCatalog = serde_json::from_str(json).map_err(|e| e.to_string())?;
        Ok(Self::from_raw(raw))
    }

    pub fn from_raw(raw: RawCatalog) -> Self {
        let mut items_by_index = HashMap::with_capacity(raw.items.len());
        let mut items_by_unique = HashMap::with_capacity(raw.items.len());

        for item in raw.items {
            items_by_unique.insert(item.unique_name.clone(), item.index);
            items_by_index.insert(item.index, item);
        }

        let mut spells_by_index = HashMap::with_capacity(raw.spells.len());
        let mut spell_indices_by_unique: HashMap<String, Vec<i32>> =
            HashMap::with_capacity(raw.spells.len());
        for mut spell in raw.spells {
            let humanized = humanize_spell_identifier(&spell.unique_name);
            let needs_heal = spell.name_fr.trim().is_empty()
                || spell.name_fr == spell.name_en
                || spell.name_fr == humanized;
            if needs_heal {
                let (fr, en) =
                    resolve_spell_i18n(&spell.unique_name, None, spell.ui_sprite.as_deref());
                if fr != en || spell.name_fr.trim().is_empty() {
                    spell.name_fr = fr;
                    if spell.name_en == humanized || spell.name_en.trim().is_empty() {
                        spell.name_en = en;
                    }
                }
            }
            if !spell.unique_name.is_empty() {
                spell_indices_by_unique
                    .entry(spell.unique_name.to_ascii_uppercase())
                    .or_default()
                    .push(spell.index);
            }
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

        let mut catalog = Self {
            version: raw.version,
            items_by_index,
            items_by_unique,
            spells_by_index,
            spell_indices_by_unique,
            weapon_by_spell: HashMap::new(),
            clusters_by_index,
            recipes_by_unique,
            spell_triggers: HashMap::new(),
            raw_mainhand_csl: HashMap::new(),
        };

        for (spell_idx, item_idx, spec) in raw.weapon_spells {
            if catalog.items_by_index.contains_key(&item_idx) && spec > 0 {
                let replace = catalog
                    .weapon_by_spell
                    .get(&spell_idx)
                    .is_none_or(|&(_, cur_spec)| spec > cur_spec);
                if replace {
                    catalog.weapon_by_spell.insert(spell_idx, (item_idx, spec));
                }
            }
        }
        catalog.rebuild_embedded_item_spell_bindings();
        catalog
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
        let (item_idx, _) = self.weapon_by_spell.get(&spell_index)?;
        self.items_by_index.get(item_idx)
    }

    /// Infers a player's main-hand weapon along with its specificity tier (`1` = shared tree skill,
    /// `2` = narrow sub-group skill, `3` = signature weapon skill).
    pub fn infer_weapon_from_spell_with_specificity(
        &self,
        spell_index: i32,
    ) -> Option<(&ItemEntry, u8)> {
        let (item_idx, spec) = self.weapon_by_spell.get(&spell_index)?;
        self.items_by_index.get(item_idx).map(|item| (item, *spec))
    }

    /// Chooses the highest-specificity main-hand weapon inferred across a set of spell indices.
    pub fn infer_best_weapon_from_spells(&self, spell_indices: &[i32]) -> Option<(&ItemEntry, u8)> {
        let mut best: Option<(&ItemEntry, u8)> = None;
        for &sp in spell_indices {
            if let Some((item, spec)) = self.infer_weapon_from_spell_with_specificity(sp) {
                let replace = match best {
                    None => true,
                    Some((cur_item, cur_spec)) => {
                        spec > cur_spec || (spec == cur_spec && item.index < cur_item.index)
                    }
                };
                if replace {
                    best = Some((item, spec));
                }
            }
        }
        best
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

    pub fn weapon_spell_count(&self) -> usize {
        self.weapon_by_spell.len()
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

    /// Exports the full catalog (including merged `ao-bin-dumps` items, spells, clusters, recipes,
    /// and inferred weapon-spell bindings) for disk caching.
    pub fn export_raw(&self) -> RawCatalog {
        let mut items: Vec<ItemEntry> = self.items_by_index.values().cloned().collect();
        items.sort_by_key(|i| i.index);
        let mut spells: Vec<SpellEntry> = self.spells_by_index.values().cloned().collect();
        spells.sort_by_key(|s| s.index);
        let mut clusters: Vec<ClusterEntry> = self.clusters_by_index.values().cloned().collect();
        clusters.sort_by(|a, b| a.index.cmp(&b.index));
        let mut recipes: Vec<CraftingRecipe> = self.recipes_by_unique.values().cloned().collect();
        recipes.sort_by(|a, b| a.item_unique_name.cmp(&b.item_unique_name));
        let mut weapon_spells: Vec<(i32, i32, u8)> = self
            .weapon_by_spell
            .iter()
            .map(|(&sp, &(item, spec))| (sp, item, spec))
            .collect();
        weapon_spells.sort_by_key(|&(sp, _, _)| sp);
        RawCatalog {
            version: self.version.clone(),
            items,
            spells,
            clusters,
            recipes,
            weapon_spells,
        }
    }

    /// Backfills any starter clusters, items, spells, or weapon-spell bindings present in
    /// `fallback` (the embedded catalog) when restoring an older disk cache.
    pub fn merge_missing_defaults(&mut self, fallback: &Catalog) {
        for (idx, item) in &fallback.items_by_index {
            if !self.items_by_index.contains_key(idx) {
                self.items_by_unique
                    .entry(item.unique_name.clone())
                    .or_insert(*idx);
                self.items_by_index.insert(*idx, item.clone());
            }
        }
        for (idx, cluster) in &fallback.clusters_by_index {
            match self.clusters_by_index.get_mut(idx) {
                None => {
                    self.clusters_by_index.insert(idx.clone(), cluster.clone());
                }
                Some(existing)
                    if existing.name_fr.trim().is_empty()
                        || existing.name_fr.starts_with("Zone ") =>
                {
                    *existing = cluster.clone();
                }
                _ => {}
            }
        }
        for (idx, spell) in &fallback.spells_by_index {
            if !self.spells_by_index.contains_key(idx) {
                if !spell.unique_name.is_empty() {
                    let list = self
                        .spell_indices_by_unique
                        .entry(spell.unique_name.to_ascii_uppercase())
                        .or_default();
                    if !list.contains(idx) {
                        list.push(*idx);
                    }
                }
                self.spells_by_index.insert(*idx, spell.clone());
            }
        }
        for (&sp_idx, &(item_idx, spec)) in &fallback.weapon_by_spell {
            if self.items_by_index.contains_key(&item_idx) {
                let replace = self
                    .weapon_by_spell
                    .get(&sp_idx)
                    .is_none_or(|&(_, cur_spec)| spec > cur_spec);
                if replace {
                    self.weapon_by_spell.insert(sp_idx, (item_idx, spec));
                }
            }
        }
    }

    fn rebuild_embedded_item_spell_bindings(&mut self) {
        let mut spell_archetypes: HashMap<i32, HashSet<String>> = HashMap::new();
        let mut candidates: Vec<(i32, &ItemEntry)> = Vec::new();
        for item in self.items_by_index.values() {
            if item.slot == "mainHand" && !item.spell_ids.is_empty() {
                let arch = strip_tier_and_enchant(&item.unique_name).to_string();
                for &sp in &item.spell_ids {
                    spell_archetypes.entry(sp).or_default().insert(arch.clone());
                    candidates.push((sp, item));
                }
            }
        }
        let mut sorted: Vec<(i32, i32, u8)> = Vec::new();
        for (sp, item) in candidates {
            let count = spell_archetypes.get(&sp).map_or(1, HashSet::len);
            let spec = specificity_from_archetype_count(count);
            sorted.push((sp, item.index, spec));
        }
        sorted.sort_by_key(|&(sp, idx, _)| (sp, idx));
        for (sp, idx, spec) in sorted {
            let replace = self
                .weapon_by_spell
                .get(&sp)
                .is_none_or(|&(_, cur_spec)| spec > cur_spec);
            if replace {
                self.weapon_by_spell.insert(sp, (idx, spec));
            }
        }
    }

    pub fn rebuild_weapon_bindings(&mut self) {
        self.weapon_by_spell.clear();
        self.rebuild_embedded_item_spell_bindings();

        if self.raw_mainhand_csl.is_empty() || self.spell_indices_by_unique.is_empty() {
            return;
        }

        // Resolve <craftingspelllist reference="..." /> inheritance into base spells per weapon.
        let mut resolved_base_spells: HashMap<String, (String, Vec<String>)> = HashMap::new();
        for uid in self.raw_mainhand_csl.keys() {
            let mut visited = HashSet::new();
            let (subcat, spells) = resolve_csl_chain(uid, &self.raw_mainhand_csl, &mut visited);
            if !spells.is_empty() {
                resolved_base_spells.insert(uid.clone(), (subcat, spells));
            }
        }

        // Pick one representative ItemEntry per canonical weapon archetype (preferred tier 6 or 4, @0).
        let mut rep_by_archetype: HashMap<String, &ItemEntry> = HashMap::new();
        for item in self.items_by_index.values() {
            if item.slot != "mainHand" {
                continue;
            }
            let base_uid = item
                .unique_name
                .split('@')
                .next()
                .unwrap_or(&item.unique_name);
            if !resolved_base_spells.contains_key(base_uid) {
                continue;
            }
            let arch = strip_tier_and_enchant(&item.unique_name).to_string();
            let replace = rep_by_archetype
                .get(&arch)
                .is_none_or(|cur| item_rep_rank(item) < item_rep_rank(cur));
            if replace {
                rep_by_archetype.insert(arch, item);
            }
        }

        // Map each spell index -> list of (subcat, archetype, item_index, hop_distance, arch_rank).
        #[allow(clippy::type_complexity)]
        let mut candidates: HashMap<
            i32,
            Vec<(String, String, i32, u8, (u8, usize, u32, i32))>,
        > = HashMap::new();

        for (arch, rep_item) in &rep_by_archetype {
            let base_uid = rep_item
                .unique_name
                .split('@')
                .next()
                .unwrap_or(&rep_item.unique_name);
            let Some((subcat, direct_spells)) = resolved_base_spells.get(base_uid) else {
                continue;
            };
            let arch_rank = archetype_canonical_rank(arch, rep_item);
            let mut bfs_visited: HashSet<String> = HashSet::new();
            let mut queue: VecDeque<(String, u8)> = VecDeque::new();
            for sp_name in direct_spells {
                let up = sp_name.trim().to_ascii_uppercase();
                if up.is_empty() || up == "AUTO_ATTACK" {
                    continue;
                }
                if bfs_visited.insert(up.clone()) {
                    queue.push_back((up, 0));
                }
            }

            while let Some((cur_name, dist)) = queue.pop_front() {
                if let Some(indices) = self.spell_indices_by_unique.get(&cur_name) {
                    for &sp_idx in indices {
                        if sp_idx <= 0 {
                            continue;
                        }
                        candidates.entry(sp_idx).or_default().push((
                            subcat.clone(),
                            arch.clone(),
                            rep_item.index,
                            dist,
                            arch_rank,
                        ));
                    }
                }
                if dist >= 4 {
                    continue;
                }
                if let Some(children) = self.spell_triggers.get(&cur_name) {
                    for child in children {
                        if !is_generic_non_weapon_spell(child) && bfs_visited.insert(child.clone())
                        {
                            queue.push_back((child.clone(), dist + 1));
                        }
                    }
                }
            }
        }

        for (sp_idx, entries) in candidates {
            let mut distinct_subcats: HashSet<&str> = HashSet::new();
            let mut distinct_archs: HashSet<&str> = HashSet::new();
            for (subcat, arch, _, _, _) in &entries {
                if !subcat.is_empty() {
                    distinct_subcats.insert(subcat.as_str());
                }
                distinct_archs.insert(arch.as_str());
            }
            // Reject generic debuffs/procs shared across multiple unrelated weapon trees.
            if distinct_subcats.len() > 1 {
                continue;
            }
            let spec = specificity_from_archetype_count(distinct_archs.len());
            if let Some((_, _, item_idx, _, _)) = entries
                .iter()
                .min_by_key(|entry| (entry.4, entry.3, entry.2))
            {
                let replace = self
                    .weapon_by_spell
                    .get(&sp_idx)
                    .is_none_or(|&(_, cur_spec)| spec > cur_spec);
                if replace {
                    self.weapon_by_spell.insert(sp_idx, (*item_idx, spec));
                }
            }
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
            if row.unique_name.is_empty() {
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

            if let Some(existing) = self.items_by_index.get_mut(&index) {
                let old_uid = std::mem::replace(&mut existing.unique_name, row.unique_name.clone());
                let same_uid = old_uid == row.unique_name;
                if !same_uid {
                    self.items_by_unique.remove(&old_uid);
                    existing.spell_ids.clear();
                    existing.item_value = (tier as i64) * 160 * (1 << enchantment);
                }
                existing.name_fr = name_fr;
                existing.name_en = name_en;
                existing.slot = slot;
                existing.tier = tier;
                existing.enchantment = enchantment;
                self.items_by_unique.insert(row.unique_name, index);
                continue;
            }

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
            if idx.is_empty() || name.is_empty() {
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
            if let Some(existing) = self.clusters_by_index.get_mut(idx) {
                let is_placeholder =
                    existing.name_fr.trim().is_empty() || existing.name_fr.starts_with("Zone ");
                if is_placeholder {
                    existing.name_fr = name.to_string();
                    existing.name_en = name.to_string();
                    existing.kind = kind.into();
                }
                continue;
            }
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
            let name_loca_tag =
                extract_xml_attr(open_tag_slice, "namelocatag").filter(|s| !s.is_empty());
            let raw_cat = extract_xml_attr(open_tag_slice, "category")
                .or_else(|| extract_xml_attr(open_tag_slice, "uitype"))
                .unwrap_or_else(|| "damage".into());

            let cur_idx = spell_index;
            spell_index += 1;

            let (resolved_fr, resolved_en) = if !unique_name.is_empty() {
                resolve_spell_i18n(&unique_name, name_loca_tag.as_deref(), ui_sprite.as_deref())
            } else {
                (String::new(), String::new())
            };

            if !unique_name.is_empty()
                && (!self.spells_by_index.contains_key(&cur_idx) || cur_idx > 0)
            {
                let was_absent = !self.spells_by_index.contains_key(&cur_idx);
                let up_name = unique_name.to_ascii_uppercase();
                let entry = SpellEntry {
                    index: cur_idx,
                    unique_name: unique_name.clone(),
                    name_fr: resolved_fr.clone(),
                    name_en: resolved_en.clone(),
                    category: normalize_spell_category(&raw_cat),
                    ui_sprite: ui_sprite.clone(),
                };
                self.spells_by_index.insert(cur_idx, entry);
                let indices = self.spell_indices_by_unique.entry(up_name).or_default();
                if !indices.contains(&cur_idx) {
                    indices.push(cur_idx);
                }
                if was_absent {
                    added += 1;
                }
            }

            let after_open = tag_start + 1 + rel_gt + 1;
            if self_closing {
                pos = after_open;
                continue;
            }

            let close_pat = format!("</{tag_name}>");
            if let Some(rel_close) = clean[after_open..].find(&close_pat) {
                let inner = &clean[after_open..after_open + rel_close];
                if !unique_name.is_empty() {
                    let triggered = extract_spell_trigger_refs(inner);
                    if !triggered.is_empty() {
                        self.spell_triggers
                            .insert(unique_name.to_ascii_uppercase(), triggered);
                    }
                }
                if tag_name == "activespell" && inner.contains("<channelingspell") {
                    let chan_idx = spell_index;
                    spell_index += 1;
                    if !unique_name.is_empty()
                        && (!self.spells_by_index.contains_key(&chan_idx) || chan_idx > 0)
                    {
                        let was_absent = !self.spells_by_index.contains_key(&chan_idx);
                        let up_name = unique_name.to_ascii_uppercase();
                        self.spells_by_index.insert(
                            chan_idx,
                            SpellEntry {
                                index: chan_idx,
                                unique_name: unique_name.clone(),
                                name_fr: format!("{resolved_fr} (Canal.)"),
                                name_en: format!("{resolved_en} (Channel)"),
                                category: normalize_spell_category(&raw_cat),
                                ui_sprite,
                            },
                        );
                        let indices = self.spell_indices_by_unique.entry(up_name).or_default();
                        if !indices.contains(&chan_idx) {
                            indices.push(chan_idx);
                        }
                        if was_absent {
                            added += 1;
                        }
                    }
                }
                pos = after_open + rel_close + close_pat.len();
            } else {
                pos = after_open;
            }
        }

        self.rebuild_weapon_bindings();
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
            let slot_attr = extract_xml_attr(open_slice, "slottype");
            let subcat_attr = extract_xml_attr(open_slice, "shopsubcategory1")
                .unwrap_or_default()
                .to_ascii_lowercase();
            let is_mainhand_tag = tag_name == "weapon"
                || tag_name == "transformationweapon"
                || (tag_name == "equipmentitem"
                    && slot_attr
                        .as_deref()
                        .is_some_and(|s| s.eq_ignore_ascii_case("mainhand")));
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

            let (base_body, post_ench_body, ench_body) =
                if let Some(ench_pos) = body.find("<enchantments") {
                    let end_ench = body[ench_pos..]
                        .find("</enchantments>")
                        .map(|e| ench_pos + e + "</enchantments>".len())
                        .unwrap_or(body.len());
                    (
                        &body[..ench_pos],
                        &body[end_ench..],
                        Some(&body[ench_pos..end_ench]),
                    )
                } else {
                    (body, "", None)
                };

            if is_mainhand_tag {
                if let Some((reference, adds, removes)) = extract_craftingspelllist(base_body)
                    .or_else(|| extract_craftingspelllist(post_ench_body))
                {
                    self.raw_mainhand_csl.insert(
                        unique_name.clone(),
                        RawCraftSpellList {
                            subcategory: subcat_attr,
                            reference,
                            adds,
                            removes,
                        },
                    );
                }
            }

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

        self.rebuild_weapon_bindings();
        Ok(upserted)
    }
}

type ParsedRawRecipe = (u32, u32, Vec<(String, u32, bool)>);

fn strip_tier_and_enchant(unique_name: &str) -> &str {
    let base = unique_name.split('@').next().unwrap_or(unique_name);
    let b = base.as_bytes();
    if b.len() >= 4 && b[0] == b'T' && b[1].is_ascii_digit() && b[2] == b'_' {
        &base[3..]
    } else {
        base
    }
}

fn specificity_from_archetype_count(count: usize) -> u8 {
    match count {
        0..=1 => 3,
        2..=3 => 2,
        _ => 1,
    }
}

fn item_rep_rank(item: &ItemEntry) -> (u8, u32, i32) {
    (
        item.enchantment,
        (item.tier as i32 - 6).unsigned_abs(),
        item.index,
    )
}

fn is_artifact_archetype(arch: &str) -> bool {
    const ART_MARKERS: &[&str] = &[
        "_HELL", "_MORGANA", "_UNDEAD", "_KEEPER", "_AVALON", "_FEY", "_CRYSTAL", "_ROYAL",
        "_SET2", "_SET3",
    ];
    ART_MARKERS.iter().any(|m| arch.contains(m))
}

fn archetype_canonical_rank(arch: &str, item: &ItemEntry) -> (u8, usize, u32, i32) {
    (
        u8::from(is_artifact_archetype(arch)),
        arch.len(),
        (item.tier as i32 - 6).unsigned_abs(),
        item.index,
    )
}

fn is_generic_non_weapon_spell(name: &str) -> bool {
    name == "AUTO_ATTACK"
        || name.starts_with("MOUNT_")
        || name.starts_with("FOOD_")
        || name.starts_with("POTION_")
        || name.starts_with("CAPE_")
        || name.contains("GLOBAL_COOLDOWN")
        || name.contains("DISMOUNT")
}

fn resolve_csl_chain(
    uid: &str,
    map: &HashMap<String, RawCraftSpellList>,
    visited: &mut HashSet<String>,
) -> (String, Vec<String>) {
    if !visited.insert(uid.to_string()) {
        return (String::new(), Vec::new());
    }
    let Some(node) = map.get(uid) else {
        return (String::new(), Vec::new());
    };
    let (ref_subcat, mut spells) = if let Some(ref ref_uid) = node.reference {
        resolve_csl_chain(ref_uid, map, visited)
    } else {
        (String::new(), Vec::new())
    };
    let subcat = if !node.subcategory.is_empty() {
        node.subcategory.clone()
    } else {
        ref_subcat
    };
    for rem in &node.removes {
        let up = rem.to_ascii_uppercase();
        spells.retain(|s| s != &up);
    }
    for add in &node.adds {
        let up = add.to_ascii_uppercase();
        if !up.is_empty() && !spells.contains(&up) {
            spells.push(up);
        }
    }
    (subcat, spells)
}

fn extract_craftingspelllist(section: &str) -> Option<(Option<String>, Vec<String>, Vec<String>)> {
    let mut pos = 0usize;
    while let Some(rel) = section[pos..].find("<craftingspelllist") {
        let start = pos + rel;
        let after_tag = start + "<craftingspelllist".len();
        let next_ch = section.as_bytes().get(after_tag).copied().unwrap_or(b' ');
        if !next_ch.is_ascii_whitespace() && next_ch != b'>' && next_ch != b'/' {
            pos = after_tag;
            continue;
        }
        let rel_gt = section[after_tag..].find('>')?;
        let gt = after_tag + rel_gt;
        let attrs = &section[after_tag..gt];
        let self_closing = attrs.trim_end().ends_with('/');
        let reference = extract_xml_attr(attrs, "reference").filter(|s| !s.is_empty());
        if self_closing {
            return Some((reference, Vec::new(), Vec::new()));
        }
        let after_open = gt + 1;
        let rel_close = section[after_open..].find("</craftingspelllist>")?;
        let inner = &section[after_open..after_open + rel_close];

        let mut adds = Vec::new();
        let mut apos = 0usize;
        while let Some(rel_cs) = inner[apos..].find("<craftspell") {
            let cs_start = apos + rel_cs;
            let Some(rel_csgt) = inner[cs_start..].find('>') else {
                break;
            };
            let cs_slice = &inner[cs_start..cs_start + rel_csgt];
            apos = cs_start + rel_csgt + 1;
            if let Some(u) = extract_xml_attr(cs_slice, "uniquename") {
                if !u.is_empty() {
                    adds.push(u);
                }
            }
        }

        let mut removes = Vec::new();
        let mut rpos = 0usize;
        while let Some(rel_rs) = inner[rpos..].find("<removespell") {
            let rs_start = rpos + rel_rs;
            let Some(rel_rsgt) = inner[rs_start..].find('>') else {
                break;
            };
            let rs_slice = &inner[rs_start..rs_start + rel_rsgt];
            rpos = rs_start + rel_rsgt + 1;
            if let Some(u) = extract_xml_attr(rs_slice, "uniquename") {
                if !u.is_empty() {
                    removes.push(u);
                }
            }
        }
        return Some((reference, adds, removes));
    }
    None
}

fn extract_spell_trigger_refs(inner: &str) -> Vec<String> {
    const SKIP_TAGS: &[&str] = &[
        "removeactivespell",
        "removespell",
        "ifspellactive",
        "ifspellcategoryactive",
        "checkspell",
        "removeeffect",
        "cancelspell",
        "spellimmunity",
        "locareference",
    ];
    const TRIGGER_ATTRS: &[&str] = &[
        "spell",
        "effect",
        "endeffect",
        "starteffect",
        "overridespell",
        "spellontrigger",
        "applyspellwhennotargetsinarea",
        "applyspellwhentargetsinarea",
        "landscapecollisioneffect",
        "collisioneffect",
        "chargespell",
        "spellchargesspell",
    ];

    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let bytes = inner.as_bytes();
    let len = bytes.len();
    let mut pos = 0usize;

    while let Some(rel_lt) = inner[pos..].find('<') {
        let lt = pos + rel_lt;
        if lt + 2 >= len {
            break;
        }
        let next_b = bytes[lt + 1];
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
        let child_tag = inner[lt + 1..name_end].to_ascii_lowercase();
        let Some(rel_gt) = inner[name_end..].find('>') else {
            break;
        };
        let attrs = &inner[name_end..name_end + rel_gt];
        pos = name_end + rel_gt + 1;

        if SKIP_TAGS.contains(&child_tag.as_str()) {
            continue;
        }
        for attr in TRIGGER_ATTRS {
            if let Some(val) = extract_xml_attr(attrs, attr) {
                let up = val.trim().to_ascii_uppercase();
                if !up.is_empty() && seen.insert(up.clone()) {
                    out.push(up);
                }
            }
        }
    }
    out
}

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
    let bytes = attrs.as_bytes();
    let mut search_from = 0usize;
    while let Some(rel) = attrs[search_from..].find(&needle) {
        let match_pos = search_from + rel;
        if match_pos == 0 || bytes[match_pos - 1].is_ascii_whitespace() {
            let start = match_pos + needle.len();
            let end = attrs[start..].find('"')?;
            return Some(attrs[start..start + end].trim().to_string());
        }
        search_from = match_pos + 1;
    }
    None
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

    #[test]
    fn resolves_quarterstaff_spells_fr_world_upserts_and_craftingspelllist_weapon_bindings() {
        let mut catalog = Catalog::from_json_str(EMBEDDED).unwrap();

        let items_json = r#"[
            {
                "Index": "8100",
                "UniqueName": "T6_2H_QUARTERSTAFF",
                "LocalizedNames": { "EN-US": "Master's Quarterstaff", "FR-FR": "Bâton de combat du maître" }
            },
            {
                "Index": "8101",
                "UniqueName": "T6_2H_TWINSCYTHE_HELL",
                "LocalizedNames": { "EN-US": "Master's Soulscythe", "FR-FR": "Faux d'âme du maître" }
            }
        ]"#;
        assert_eq!(catalog.merge_ao_bin_dumps_items(items_json).unwrap(), 2);

        // World merge resolves numeric cluster codes like 0203 -> Willowsigh Marsh
        let world_json = r#"[
            { "Index": "0203", "UniqueName": "Willowsigh Marsh" }
        ]"#;
        assert_eq!(catalog.merge_ao_bin_dumps_world(world_json).unwrap(), 1);
        assert_eq!(catalog.cluster("0203").unwrap().name_fr, "Willowsigh Marsh");

        let spells_xml = r#"<spells>
            <passivespell uniquename="PASSIVE_MAXLOAD" />
            <activespell uniquename="QSTAFF_COMBO" uisprite="QSTAFF_COMBO" category="damage">
                <channelingspell effectcount="3">
                    <spelleffectarea effect="QSTAFF_COMBO_EFFECT" />
                </channelingspell>
            </activespell>
            <activespell uniquename="QSTAFF_COMBO_EFFECT" uisprite="QSTAFF_COMBO" category="damage" />
            <activespell uniquename="VAULT_ATTACK" uisprite="VAULT_LEAP" category="damage">
                <dash endeffect="VAULT_ATTACK_AREA" />
            </activespell>
            <passivespell uniquename="VAULT_ATTACK_AREA">
                <removeactivespell spell="TORNADO" />
                <spelleffectarea effect="VAULT_ATTACK_EFFECT" />
            </passivespell>
            <activespell uniquename="VAULT_ATTACK_EFFECT" uisprite="VAULT_LEAP" category="damage" />
            <activespell uniquename="WHIRLING_STAFF" uisprite="WHIRLING_STRIKES" category="damage">
                <channelingspell effectcount="3">
                    <applyspell spell="WHIRLING_STAFF_DAMAGE" />
                </channelingspell>
            </activespell>
            <activespell uniquename="WHIRLING_STAFF_DAMAGE" uisprite="WHIRLING_STRIKES" category="damage" />
            <activespell uniquename="TORNADO" uisprite="SOULSCYTHE" category="damage" />
            <passivespell uniquename="PASSIVE_CAPE_AVALON_EXPLOSION" uisprite="CAPE_COOLDOWN" />
            <activespell uniquename="SPRINTHOT" uisprite="SPRINT_HEAL" category="buff" />
        </spells>"#;
        catalog.merge_ao_bin_dumps_spells_xml(spells_xml).unwrap();

        // Check French localization resolution (even via suffix strip / uisprite)
        let find_spell_in = |cat: &Catalog, uid: &str| -> SpellEntry {
            (1..20)
                .filter_map(|i| cat.spell(i))
                .find(|s| s.unique_name == uid)
                .unwrap()
                .clone()
        };
        assert_eq!(
            find_spell_in(&catalog, "QSTAFF_COMBO_EFFECT").name_fr,
            "Danse du vent"
        );
        assert_eq!(
            find_spell_in(&catalog, "VAULT_ATTACK_EFFECT").name_fr,
            "Saut voûté"
        );
        assert_eq!(
            find_spell_in(&catalog, "WHIRLING_STAFF_DAMAGE").name_fr,
            "Frappes tourbillonnantes"
        );
        assert_eq!(
            find_spell_in(&catalog, "PASSIVE_CAPE_AVALON_EXPLOSION").name_fr,
            "Impulsion de force"
        );
        assert_eq!(
            find_spell_in(&catalog, "SPRINTHOT").name_fr,
            "Sprint rajeunissant"
        );

        let items_xml = r#"<items>
            <weapon uniquename="T4_2H_QUARTERSTAFF" slottype="mainhand" shopsubcategory1="quarterstaff">
                <craftingspelllist>
                    <craftspell uniquename="QSTAFF_COMBO" slots="2" />
                    <craftspell uniquename="WHIRLING_STAFF" slots="1" />
                    <craftspell uniquename="VAULT_ATTACK" slots="3" />
                </craftingspelllist>
            </weapon>
            <weapon uniquename="T6_2H_QUARTERSTAFF" slottype="mainhand" shopsubcategory1="quarterstaff">
                <craftingspelllist reference="T4_2H_QUARTERSTAFF" />
            </weapon>
            <weapon uniquename="T6_2H_TWINSCYTHE_HELL" slottype="mainhand" shopsubcategory1="quarterstaff">
                <craftingspelllist reference="T4_2H_QUARTERSTAFF">
                    <removespell uniquename="VAULT_ATTACK" />
                    <craftspell uniquename="TORNADO" slots="3" />
                </craftingspelllist>
            </weapon>
        </items>"#;
        catalog.merge_ao_bin_dumps_items_xml(items_xml).unwrap();

        let qcombo_idx = find_spell_in(&catalog, "QSTAFF_COMBO_EFFECT").index;
        let vault_idx = find_spell_in(&catalog, "VAULT_ATTACK_EFFECT").index;
        let tornado_idx = find_spell_in(&catalog, "TORNADO").index;

        let (w_shared, spec_shared) = catalog
            .infer_weapon_from_spell_with_specificity(qcombo_idx)
            .unwrap();
        assert_eq!(w_shared.unique_name, "T6_2H_QUARTERSTAFF");
        assert_eq!(spec_shared, 2); // 2 quarterstaff archetypes share W

        let (w_vault, spec_vault) = catalog
            .infer_weapon_from_spell_with_specificity(vault_idx)
            .unwrap();
        assert_eq!(w_vault.unique_name, "T6_2H_QUARTERSTAFF");
        assert_eq!(spec_vault, 3); // signature E after removespell

        let (w_tornado, spec_tornado) = catalog
            .infer_weapon_from_spell_with_specificity(tornado_idx)
            .unwrap();
        assert_eq!(w_tornado.unique_name, "T6_2H_TWINSCYTHE_HELL");
        assert_eq!(spec_tornado, 3);

        // Best weapon picks signature over shared
        let (best, best_spec) = catalog
            .infer_best_weapon_from_spells(&[qcombo_idx, tornado_idx])
            .unwrap();
        assert_eq!(best.unique_name, "T6_2H_TWINSCYTHE_HELL");
        assert_eq!(best_spec, 3);

        // Cache persistence preserves weapon_spells across export_raw -> from_raw
        let raw = catalog.export_raw();
        let reloaded = Catalog::from_raw(raw);
        assert_eq!(
            reloaded
                .infer_weapon_from_spell(vault_idx)
                .unwrap()
                .unique_name,
            "T6_2H_QUARTERSTAFF"
        );
        assert_eq!(
            reloaded
                .infer_weapon_from_spell(tornado_idx)
                .unwrap()
                .unique_name,
            "T6_2H_TWINSCYTHE_HELL"
        );
    }
}
