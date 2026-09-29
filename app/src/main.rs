#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod backend;
mod legion_hid;
mod thumbs;

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    ui.run()
}
