#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod app;
slint::include_modules!();

fn main() {
    if let Err(e) = app::run() {
        rfd::MessageDialog::new()
            .set_title("Winit — não foi possível iniciar")
            .set_description(format!("{e:#}\n\nOs dados existentes foram preservados. Consulte o README para recuperação."))
            .set_level(rfd::MessageLevel::Error)
            .show();
        std::process::exit(1);
    }
}
