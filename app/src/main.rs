#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod backend;
#[allow(dead_code)]
mod demo;

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    ui.run()
}
