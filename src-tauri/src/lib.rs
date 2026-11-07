mod capture;
mod settings;
mod update;

use albion_core::catalog::{Catalog, CraftingRecipe, ItemEntry};
use albion_core::crafting::{simulate_crafting, CraftingSimulationInput, CraftingSimulationResult};
use albion_core::engine::{AnalyzerEngine, AppSnapshot, ArchivedSession, MeterSnapshot};
use capture::{now_unix_ms, start_background_capture, CaptureStatus};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use serde::Serialize;
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{AppHandle, Manager, WindowEvent};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

const CATALOG_JSON: &str = include_str!("../../data/catalog.json");
const RECIPES_JSON: &str = include_str!("../../data/recipes.json");
const LIVE_HISTORY_FLUSH_INTERVAL_MS: u64 = 10_000;

struct AppState {
    engine: Arc<Mutex<AnalyzerEngine>>,
    capture_status: Arc<Mutex<CaptureStatus>>,
    click_through: AtomicBool,
    last_history_flush_ms: AtomicU64,
    shortcut_error: OnceLock<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FullStatusResponse {
    snapshot: AppSnapshot,
    capture: CaptureStatus,
    click_through: bool,
    shortcut_error: Option<String>,
}

#[tauri::command]
fn get_state(app: AppHandle, state: tauri::State<AppState>) -> Result<FullStatusResponse, String> {
    let now = now_unix_ms();
    let mut history_to_flush = None;
    let snapshot = {
        let mut eng = state
            .engine
            .lock()
            .map_err(|_| "verrou moteur indisponible".to_string())?;
        let last_flush = state.last_history_flush_ms.load(Ordering::Relaxed);
        if now.saturating_sub(last_flush) >= LIVE_HISTORY_FLUSH_INTERVAL_MS
            && eng.take_dirty_history()
        {
            state.last_history_flush_ms.store(now, Ordering::Relaxed);
            history_to_flush = serde_json::to_value(eng.export_history()).ok();
        }
        eng.snapshot(now)
    };

    if let Some(val) = history_to_flush {
        if let Ok(path) = history_path(&app) {
            let _ = settings::save(&path, &val);
        }
    }

    let capture = state
        .capture_status
        .lock()
        .map_err(|_| "verrou capture indisponible".to_string())?
        .clone();
    Ok(FullStatusResponse {
        snapshot,
        capture,
        click_through: state.click_through.load(Ordering::Relaxed),
        shortcut_error: state.shortcut_error.get().cloned(),
    })
}

#[tauri::command]
fn simulate_session(
    app: AppHandle,
    state: tauri::State<AppState>,
) -> Result<FullStatusResponse, String> {
    let now = now_unix_ms();
    {
        let mut eng = state
            .engine
            .lock()
            .map_err(|_| "verrou moteur indisponible".to_string())?;
        eng.run_wire_simulation_scenario(now);
        let _ = eng.take_dirty_history();
        persist_history_best_effort(&app, &eng);
    }
    get_state(app, state)
}

#[tauri::command]
fn reset_meter(
    app: AppHandle,
    state: tauri::State<AppState>,
) -> Result<FullStatusResponse, String> {
    {
        let mut eng = state
            .engine
            .lock()
            .map_err(|_| "verrou moteur indisponible".to_string())?;
        eng.reset_meter();
    }
    get_state(app, state)
}

#[tauri::command]
fn reset_session(
    app: AppHandle,
    state: tauri::State<AppState>,
) -> Result<FullStatusResponse, String> {
    let now = now_unix_ms();
    {
        let mut eng = state
            .engine
            .lock()
            .map_err(|_| "verrou moteur indisponible".to_string())?;
        eng.reset_session(now);
        persist_history_best_effort(&app, &eng);
    }
    get_state(app, state)
}

#[tauri::command]
fn take_snapshot(
    app: AppHandle,
    state: tauri::State<AppState>,
    label: Option<String>,
) -> Result<Option<MeterSnapshot>, String> {
    let now = now_unix_ms();
    let mut eng = state
        .engine
        .lock()
        .map_err(|_| "verrou moteur indisponible".to_string())?;
    let snap = eng.take_snapshot(label, now);
    if snap.is_some() {
        let _ = eng.take_dirty_history();
        persist_history_best_effort(&app, &eng);
    }
    Ok(snap)
}

#[tauri::command]
fn delete_snapshot(
    app: AppHandle,
    state: tauri::State<AppState>,
    id: String,
) -> Result<bool, String> {
    let mut eng = state
        .engine
        .lock()
        .map_err(|_| "verrou moteur indisponible".to_string())?;
    let deleted = eng.delete_snapshot(&id);
    if deleted {
        let _ = eng.take_dirty_history();
        persist_history_best_effort(&app, &eng);
    }
    Ok(deleted)
}

#[tauri::command]
fn archive_session(
    app: AppHandle,
    state: tauri::State<AppState>,
    label: Option<String>,
) -> Result<Option<ArchivedSession>, String> {
    let now = now_unix_ms();
    let mut eng = state
        .engine
        .lock()
        .map_err(|_| "verrou moteur indisponible".to_string())?;
    let archived = eng.archive_session(label, now);
    if archived.is_some() {
        let _ = eng.take_dirty_history();
        persist_history_best_effort(&app, &eng);
    }
    Ok(archived)
}

#[tauri::command]
fn delete_archived_session(
    app: AppHandle,
    state: tauri::State<AppState>,
    id: String,
) -> Result<bool, String> {
    let mut eng = state
        .engine
        .lock()
        .map_err(|_| "verrou moteur indisponible".to_string())?;
    let deleted = eng.delete_archived_session(&id);
    if deleted {
        let _ = eng.take_dirty_history();
        persist_history_best_effort(&app, &eng);
    }
    Ok(deleted)
}

#[tauri::command]
fn calculate_crafting(input: CraftingSimulationInput) -> CraftingSimulationResult {
    simulate_crafting(&input)
}

#[tauri::command]
fn get_crafting_recipe(
    state: tauri::State<AppState>,
    unique_name: String,
) -> Result<Option<CraftingRecipe>, String> {
    let eng = state
        .engine
        .lock()
        .map_err(|_| "verrou moteur indisponible".to_string())?;
    Ok(eng.catalog.recipe(&unique_name))
}

#[tauri::command]
fn export_loot_csv(state: tauri::State<AppState>) -> Result<String, String> {
    let eng = state
        .engine
        .lock()
        .map_err(|_| "verrou moteur indisponible".to_string())?;
    Ok(eng.export_loot_csv())
}

#[tauri::command]
fn list_catalog_items(state: tauri::State<AppState>) -> Result<Vec<ItemEntry>, String> {
    let eng = state
        .engine
        .lock()
        .map_err(|_| "verrou moteur indisponible".to_string())?;
    Ok(eng.catalog.all_items().into_iter().cloned().collect())
}

#[tauri::command]
fn search_catalog_items(
    state: tauri::State<AppState>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<ItemEntry>, String> {
    let eng = state
        .engine
        .lock()
        .map_err(|_| "verrou moteur indisponible".to_string())?;
    Ok(eng
        .catalog
        .search_items(&query, limit.unwrap_or(20))
        .into_iter()
        .cloned()
        .collect())
}

#[tauri::command]
fn import_ao_bin_dumps(
    app: AppHandle,
    state: tauri::State<AppState>,
    json_text: String,
    world_json: Option<String>,
    spells_xml: Option<String>,
    items_xml: Option<String>,
) -> Result<usize, String> {
    let mut eng = state
        .engine
        .lock()
        .map_err(|_| "verrou moteur indisponible".to_string())?;
    let mut added = eng.catalog.merge_ao_bin_dumps_items(&json_text)?;
    if let Some(w_json) = world_json {
        if !w_json.trim().is_empty() {
            added += eng.catalog.merge_ao_bin_dumps_world(&w_json).unwrap_or(0);
        }
    }
    if let Some(s_xml) = spells_xml {
        if !s_xml.trim().is_empty() {
            let res = eng.catalog.merge_ao_bin_dumps_spells_xml(&s_xml);
            added += res.unwrap_or(0);
        }
    }
    if let Some(i_xml) = items_xml {
        if !i_xml.trim().is_empty() {
            let res = eng.catalog.merge_ao_bin_dumps_items_xml(&i_xml);
            added += res.unwrap_or(0);
        }
    }
    if let Ok(path) = catalog_cache_path(&app) {
        if let Ok(raw_val) = serde_json::to_value(eng.catalog.export_raw()) {
            let _ = settings::save(&path, &raw_val);
        }
    }
    Ok(added)
}

fn apply_click_through(app: &AppHandle, state: &AppState, enabled: bool) -> Result<bool, String> {
    let Some(window) = app.get_webview_window("main") else {
        return Err("Fenêtre principale introuvable".into());
    };
    window
        .set_ignore_cursor_events(enabled)
        .map_err(|e| e.to_string())?;
    if enabled {
        let _ = window.set_always_on_top(true);
    }
    state.click_through.store(enabled, Ordering::Relaxed);
    Ok(enabled)
}

#[tauri::command]
fn set_click_through(
    app: AppHandle,
    state: tauri::State<AppState>,
    enabled: bool,
) -> Result<bool, String> {
    apply_click_through(&app, &state, enabled)
}

#[tauri::command]
fn relaunch_as_admin(app: AppHandle) -> Result<(), String> {
    capture::relaunch_as_admin()?;
    app.exit(0);
    Ok(())
}

fn config_file_path(app: &AppHandle, filename: &str) -> Result<std::path::PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    Ok(dir.join(filename))
}

fn settings_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    config_file_path(app, "settings.json")
}

fn history_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    config_file_path(app, "session_history.json")
}

fn catalog_cache_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    config_file_path(app, "catalog_cache.json")
}

fn persist_history_best_effort(app: &AppHandle, eng: &AnalyzerEngine) {
    if let Ok(path) = history_path(app) {
        if let Ok(val) = serde_json::to_value(eng.export_history()) {
            let _ = settings::save(&path, &val);
        }
    }
}

#[tauri::command]
fn load_settings(app: AppHandle) -> Result<settings::LoadedSettings, String> {
    settings::load(&settings_path(&app)?)
}

#[tauri::command]
fn save_settings(app: AppHandle, settings: serde_json::Value) -> Result<(), String> {
    settings::save(&settings_path(&app)?, &settings)
}

/// Ctrl + Alt + M: hide the window or bring it back in front of Albion Online.
fn toggle_main_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let shown = window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(false);
    let result = if shown {
        window.hide()
    } else {
        window
            .unminimize()
            .and_then(|_| window.show())
            .and_then(|_| window.set_focus())
    };
    if let Err(e) = result {
        eprintln!("Ctrl+Alt+M: {e}");
    }
}

/// Ctrl + Alt + L: toggle click-through mode so mouse clicks pass directly to Albion Online.
fn toggle_click_through_shortcut(app: &AppHandle) {
    let state = app.state::<AppState>();
    let next = !state.click_through.load(Ordering::Relaxed);
    if let Err(e) = apply_click_through(app, &state, next) {
        eprintln!("Ctrl+Alt+L: {e}");
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let now = now_unix_ms();
    let mut catalog =
        Catalog::from_json_str(CATALOG_JSON).expect("embedded data/catalog.json is valid");
    let _ = catalog.load_compact_recipes_json(RECIPES_JSON, false);
    let engine = Arc::new(Mutex::new(AnalyzerEngine::new(catalog, now)));
    let capture_status = Arc::new(Mutex::new(CaptureStatus::default()));
    start_background_capture(Arc::clone(&engine), Arc::clone(&capture_status));

    let toggle_vis = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyM);
    let toggle_lock = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyL);
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    if shortcut == &toggle_vis {
                        toggle_main_window(app);
                    } else if shortcut == &toggle_lock {
                        toggle_click_through_shortcut(app);
                    }
                })
                .build(),
        )
        .manage(AppState {
            engine,
            capture_status,
            click_through: AtomicBool::new(false),
            last_history_flush_ms: AtomicU64::new(now),
            shortcut_error: OnceLock::new(),
        })
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed => {
                let app = window.app_handle();
                if let Ok(mut eng) = app.state::<AppState>().engine.lock() {
                    let _ = eng.archive_session(None, now_unix_ms());
                    let _ = eng.take_dirty_history();
                    persist_history_best_effort(app, &eng);
                }
            }
            _ => {}
        })
        .setup(move |app| {
            let handle = app.handle();
            // Restore cached catalog and persisted session history if present
            if let Ok(mut eng) = app.state::<AppState>().engine.lock() {
                if let Ok(cat_path) = catalog_cache_path(handle) {
                    if let Ok(text) = std::fs::read_to_string(&cat_path) {
                        if let Ok(mut cached_cat) = Catalog::from_json_str(&text) {
                            let _ = cached_cat.load_compact_recipes_json(RECIPES_JSON, false);
                            if cached_cat.item_count() >= eng.catalog.item_count()
                                || cached_cat.spell_count() > eng.catalog.spell_count()
                                || cached_cat.recipe_count() > eng.catalog.recipe_count()
                            {
                                eng.catalog = cached_cat;
                            }
                        }
                    }
                }
                if let Ok(hist_path) = history_path(handle) {
                    if let Ok(text) = std::fs::read_to_string(&hist_path) {
                        if let Ok(hist) = serde_json::from_str(&text) {
                            eng.restore_history(hist);
                        }
                    }
                }
            }
            for sc in [toggle_vis, toggle_lock] {
                if let Err(e) = app.global_shortcut().register(sc) {
                    let _ = app.state::<AppState>().shortcut_error.set(e.to_string());
                }
            }
            update::check_in_background(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            simulate_session,
            reset_meter,
            reset_session,
            archive_session,
            delete_archived_session,
            take_snapshot,
            delete_snapshot,
            calculate_crafting,
            get_crafting_recipe,
            export_loot_csv,
            list_catalog_items,
            search_catalog_items,
            import_ao_bin_dumps,
            set_click_through,
            relaunch_as_admin,
            load_settings,
            save_settings
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
