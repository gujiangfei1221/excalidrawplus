//! Tauri v2 application entry point for the Excalidraw cloud-sync desktop app.
//!
//! This file is intentionally minimal for task 1.1. Subsequent tasks will:
//!   * Wire up the COS client and sync engine.
//!   * Register Tauri commands and the initialization error dialog
//!     (see task 16.1).

use std::sync::Arc;

use commands::AppState;
use connectivity::ConnectivityMonitor;
use cos_client::CosClient;
use database::Database;
use file_store::FileStore;
use models::CosConfig;
use sync_engine::SyncEngine;
use tauri::Manager;
use tokio::sync::Mutex;

pub mod commands;
pub mod connectivity;
pub mod cos_client;
pub mod database;
pub mod file_store;
pub mod models;
pub mod sync_engine;

#[cfg(test)]
mod tests;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(setup_cloud_sync)
        .invoke_handler(tauri::generate_handler![
            commands::save_cos_config,
            commands::validate_cos_config,
            commands::get_cos_config,
            commands::save_canvas,
            commands::load_canvas,
            commands::create_new_file,
            commands::delete_file,
            commands::rename_file,
            commands::export_file,
            commands::get_file_list,
            commands::trigger_sync,
            commands::get_sync_status,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn setup_cloud_sync(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let app_data_dir = app.path().app_data_dir()?;

    std::fs::create_dir_all(&app_data_dir)?;

    let db = Database::open(&app_data_dir.join("metadata.sqlite"))?;
    let config = db.get_cos_config()?;
    let has_cos_config = config.is_some();
    let cos_config = config.unwrap_or_else(placeholder_cos_config);
    let cos_client = CosClient::new(&cos_config).map_err(std::io::Error::other)?;
    let file_store = FileStore::new(app_data_dir.join("files"))?;
    let conn_monitor = ConnectivityMonitor::new(Arc::new(cos_client.clone()));
    let mut sync_engine = SyncEngine::new(cos_client, db, file_store, conn_monitor);
    sync_engine.set_cloud_sync_enabled(has_cos_config);

    if has_cos_config {
        sync_engine.start(app.handle().clone());
    }

    app.manage(AppState {
        sync_engine: Arc::new(Mutex::new(sync_engine)),
    });

    Ok(())
}

fn placeholder_cos_config() -> CosConfig {
    CosConfig {
        secret_id: "placeholder-secret-id".to_string(),
        secret_key: "placeholder-secret-key".to_string(),
        bucket: "placeholder-bucket".to_string(),
        region: "ap-guangzhou".to_string(),
    }
}
