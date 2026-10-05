#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod backend;
mod battery;
mod wifi;
mod demo;
mod legion_hid;
mod thumbs;
mod ui;

slint::include_modules!();

const USAGE: &str = "lounge [--demo [--lang fr] [--resume show]] [--script \"go games; shot a.png\" [--intro]] [--size 1600x1000]";

fn main() -> Result<(), slint::PlatformError> {
    let mut demo = None;
    let mut lang = "en";
    let mut resume = demo::Resume::Game;
    let mut script = None;
    let mut size = None;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--demo" => demo = Some(()),
            "--lang" => {
                i += 1;
                lang = if args.get(i).map(String::as_str) == Some("fr") { "fr" } else { "en" };
            }
            "--resume" => {
                i += 1;
                if args.get(i).map(String::as_str) == Some("show") {
                    resume = demo::Resume::Show;
                }
            }
            "--script" => {
                i += 1;
                script = args.get(i).cloned();
            }
            "--size" => {
                i += 1;
                size = args.get(i).and_then(|s| s.split_once('x')).and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => {}
        }
        i += 1;
    }
    ui::run(ui::RunOpts { demo: demo.map(|_| demo::DemoOpts { lang, resume }), script, size })
}
