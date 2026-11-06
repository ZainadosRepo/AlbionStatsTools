//! Background release check against GitHub Releases (`latest.json`).

use tauri::AppHandle;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
use tauri_plugin_updater::UpdaterExt;

pub fn check_in_background(app: AppHandle) {
    if cfg!(debug_assertions) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        if let Err(e) = offer(&app).await {
            eprintln!("update check: {e}");
        }
    });
}

async fn offer(app: &AppHandle) -> tauri_plugin_updater::Result<()> {
    let Some(update) = app.updater()?.check().await? else {
        return Ok(());
    };
    let install = app
        .dialog()
        .message(format!(
            "Albion Stats {} est disponible (installée : {}).\n\nL'installer maintenant ? L'application redémarrera.",
            update.version, update.current_version
        ))
        .title("Mise à jour disponible")
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Installer".into(),
            "Plus tard".into(),
        ))
        .blocking_show();
    if install {
        update.download_and_install(|_, _| {}, || {}).await?;
        app.restart();
    }
    Ok(())
}
