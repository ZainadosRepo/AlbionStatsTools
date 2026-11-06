//! Crafting Return Rate (RRR), station nutrition fee, and market profit calculator.
//!
//! Uses the exact Albion Online production formulas:
//! - Resource Return Rate from total crafting bonus `b` (%):
//!   `RRR = 1.0 - 1.0 / (1.0 + b / 100.0)`
//! - Station usage fee per craft:
//!   `Fee = (ItemValue * 0.1125) * (UsageFeePerHundredNutrition / 100.0)`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CraftingMaterialLine {
    pub name: String,
    pub quantity_per_item: u32,
    pub unit_price: f64,
    /// Artefacts and hearts are not returned by Resource Return Rate (`is_returnable = false`).
    pub is_returnable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CraftingSimulationInput {
    pub item_name: String,
    pub craft_quantity: u32,
    pub sell_unit_price: f64,
    pub item_value: f64,
    /// Total zone + hideout + city + focus production bonus percentage (e.g. `18.0`, `47.9`, `77.9`).
    pub production_bonus_percent: f64,
    /// Usage fee charged by the crafting station per 100 nutrition (silver).
    pub usage_fee_per_100_nutrition: f64,
    pub is_premium: bool,
    pub is_sell_order: bool,
    pub journal_profit_per_item: f64,
    pub materials: Vec<CraftingMaterialLine>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CraftingSimulationResult {
    pub item_name: String,
    pub craft_quantity: u32,
    pub resource_return_rate: f64,
    pub gross_material_cost_per_item: f64,
    pub net_material_cost_per_item: f64,
    pub station_fee_per_item: f64,
    pub market_tax_rate: f64,
    pub market_fee_per_item: f64,
    pub total_cost_per_item: f64,
    pub net_revenue_per_item: f64,
    pub net_profit_per_item: f64,
    pub total_batch_cost: f64,
    pub total_batch_revenue: f64,
    pub total_batch_profit: f64,
    pub margin_percent: f64,
}

/// Converts a production bonus percentage `b` (e.g. `18.0` -> `15.25%`, `47.9` -> `32.39%`)
/// into the effective Resource Return Rate in `[0.0, 1.0)`.
#[inline]
pub fn effective_return_rate(production_bonus_percent: f64) -> f64 {
    if production_bonus_percent <= 0.0 {
        return 0.0;
    }
    1.0 - 1.0 / (1.0 + production_bonus_percent / 100.0)
}

/// Computes the crafting station silver fee from `item_value` and `usage_fee_per_100_nutrition`.
#[inline]
pub fn station_fee_per_craft(item_value: f64, usage_fee_per_100_nutrition: f64) -> f64 {
    if item_value <= 0.0 || usage_fee_per_100_nutrition <= 0.0 {
        return 0.0;
    }
    (item_value * 0.1125) * (usage_fee_per_100_nutrition / 100.0)
}

/// Computes the full crafting & market profit breakdown.
pub fn simulate_crafting(input: &CraftingSimulationInput) -> CraftingSimulationResult {
    let qty = input.craft_quantity.max(1) as f64;
    let rrr = effective_return_rate(input.production_bonus_percent);

    let mut gross_mat = 0.0;
    let mut net_mat = 0.0;
    for mat in &input.materials {
        let line = (mat.quantity_per_item as f64) * mat.unit_price.max(0.0);
        gross_mat += line;
        if mat.is_returnable {
            net_mat += line * (1.0 - rrr);
        } else {
            net_mat += line;
        }
    }

    let station_fee = station_fee_per_craft(input.item_value, input.usage_fee_per_100_nutrition);

    // Albion Market tax: 4% with Premium, 8% without Premium, plus 2.5% setup fee for sell orders.
    let sales_tax = if input.is_premium { 0.04 } else { 0.08 };
    let setup_fee = if input.is_sell_order { 0.025 } else { 0.0 };
    let market_tax_rate = sales_tax + setup_fee;
    let market_fee = input.sell_unit_price.max(0.0) * market_tax_rate;

    let total_cost_per_item = net_mat + station_fee + market_fee;
    let net_revenue_per_item =
        input.sell_unit_price.max(0.0) + input.journal_profit_per_item.max(0.0);
    let net_profit_per_item = net_revenue_per_item - total_cost_per_item;

    let total_batch_cost = total_cost_per_item * qty;
    let total_batch_revenue = net_revenue_per_item * qty;
    let total_batch_profit = net_profit_per_item * qty;
    let margin_percent = if total_cost_per_item > 0.0 {
        (net_profit_per_item / total_cost_per_item) * 100.0
    } else {
        0.0
    };

    CraftingSimulationResult {
        item_name: input.item_name.clone(),
        craft_quantity: input.craft_quantity.max(1),
        resource_return_rate: rrr,
        gross_material_cost_per_item: gross_mat,
        net_material_cost_per_item: net_mat,
        station_fee_per_item: station_fee,
        market_tax_rate,
        market_fee_per_item: market_fee,
        total_cost_per_item,
        net_revenue_per_item,
        net_profit_per_item,
        total_batch_cost,
        total_batch_revenue,
        total_batch_profit,
        margin_percent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_royal_city_and_focus_return_rates_accurately() {
        // Royal city without focus: +18% bonus => 1 - 1/1.18 = 15.2542%
        let rrr_no_focus = effective_return_rate(18.0);
        assert!((rrr_no_focus - 0.152542).abs() < 1e-5);

        // Royal city with focus: +18% + 59% = 77% => 1 - 1/1.77 = 43.5028%
        let rrr_focus = effective_return_rate(77.0);
        assert!((rrr_focus - 0.435028).abs() < 1e-5);
    }

    #[test]
    fn separates_returnable_materials_from_artefacts_and_computes_profit() {
        let input = CraftingSimulationInput {
            item_name: "Lames sculptées T7.1".into(),
            craft_quantity: 10,
            sell_unit_price: 420_000.0,
            item_value: 5_120.0,
            production_bonus_percent: 18.0,
            usage_fee_per_100_nutrition: 450.0,
            is_premium: true,
            is_sell_order: true,
            journal_profit_per_item: 14_500.0,
            materials: vec![
                CraftingMaterialLine {
                    name: "Barres d'acier T7.1".into(),
                    quantity_per_item: 20,
                    unit_price: 9_500.0,
                    is_returnable: true,
                },
                CraftingMaterialLine {
                    name: "Cuir travaillé T7.1".into(),
                    quantity_per_item: 12,
                    unit_price: 8_200.0,
                    is_returnable: true,
                },
                CraftingMaterialLine {
                    name: "Artefact infernal T7".into(),
                    quantity_per_item: 1,
                    unit_price: 85_000.0,
                    is_returnable: false,
                },
            ],
        };
        let res = simulate_crafting(&input);
        assert!(res.net_material_cost_per_item < res.gross_material_cost_per_item);
        assert!((res.market_tax_rate - 0.065).abs() < 1e-9);
        assert!(res.net_profit_per_item > 50_000.0);
        assert!((res.total_batch_profit - res.net_profit_per_item * 10.0).abs() < 1e-6);
    }
}
