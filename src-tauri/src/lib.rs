pub mod api;
pub mod config_file;
pub mod device;
pub mod ports;
pub mod protocol;
pub mod sim;
pub mod transport;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use tauri::Manager;
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(api::AppState::default())
        .invoke_handler(tauri::generate_handler![
            api::list_ports,
            api::connect,
            api::disconnect,
            api::read_params,
            api::write_params,
            api::set_streaming,
            api::start_calibration,
            api::auto_gain,
            api::export_config,
            api::import_config
        ])
        .on_window_event(|window, event| {
            if window.label() == "main" && matches!(event, tauri::WindowEvent::Destroyed) {
                // `close` is idempotent (it takes the device out of the state).
                window.app_handle().state::<api::AppState>().close();
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if matches!(
                event,
                tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
            ) {
                app.state::<api::AppState>().close();
            }
        });
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds() {}
}
