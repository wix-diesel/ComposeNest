fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "get_bootstrap",
            "prepare_create",
            "update_create_plan",
            "view_create_plan",
            "get_create_receipt",
            "discard_create_plan",
            "confirm_create",
        ]),
    ))
    .expect("Tauri build failed");
}
