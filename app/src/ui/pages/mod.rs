//! One module per screen; each wires its Slint global in `install`.

use super::ctx::Ctx;

pub mod apps;
pub mod detail;
pub mod grid;
pub mod home;
pub mod padtest;
pub mod remote;
pub mod search;
pub mod settings;
pub mod stats;
pub mod transfers;
pub mod welcome;

pub fn install(ctx: &Ctx) {
    apps::install(ctx);
    detail::install(ctx);
    grid::install(ctx);
    home::install(ctx);
    padtest::install(ctx);
    remote::install(ctx);
    search::install(ctx);
    settings::install(ctx);
    stats::install(ctx);
    transfers::install(ctx);
    welcome::install(ctx);
}
