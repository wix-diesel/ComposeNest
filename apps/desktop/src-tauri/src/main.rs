#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use composenest_adapters::SystemClock;
use composenest_application::BootstrapService;

fn main() -> tauri::Result<()> {
    let bootstrap = BootstrapService::new(SystemClock).bootstrap();

    tauri::Builder::default()
        .manage(bootstrap)
        .run(tauri::generate_context!())
}
