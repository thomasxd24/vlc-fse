//! Layers over the pages: quick menu, now playing, the running-game layer, updates, the boot intro,
//! and sound / rumble feedback.

use super::ctx::Ctx;

pub mod battery;
pub mod wifi;
pub mod bluetooth;
pub mod feedback;
pub mod gamelayer;
pub mod intro;
pub mod nowplaying;
pub mod quickmenu;
pub mod update;

pub fn install(ctx: &Ctx) {
    battery::install(ctx);
    wifi::install(ctx);
    bluetooth::install(ctx);
    feedback::install(ctx);
    gamelayer::install(ctx);
    intro::install(ctx);
    nowplaying::install(ctx);
    quickmenu::install(ctx);
    update::install(ctx);
}
