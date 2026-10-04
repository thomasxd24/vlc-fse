//! Search (the renderer's VIEWS.search, renderResults, searchKey and norm).
//!
//! Rust owns the query: on-screen keys and physical typing both arrive here, the query goes back to
//! `SearchPage.query`, and the matches are rebuilt as sections (Games, TV Shows, Movies, Apps). The flat
//! (kind, id) list behind the cards is kept so indices from Slint map straight back.

use crate::ui::ctx::{cx, Ctx};
use crate::ui::i18n::t;
use crate::ui::model::{self, b, poster_card, s};
use crate::ui::{actions, fmt, router};
use crate::{Card, SearchPage, SearchSection};
use serde_json::Value;
use slint::ComponentHandle;
use std::cell::RefCell;

thread_local! {
    /// (kind, id) of every result card, in display order.
    static RESULTS: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
}

pub fn install(ctx: &Ctx) {
    let ui = ctx.ui();
    let page = ui.global::<SearchPage>();
    page.on_press(|k| key(&k));
    page.on_typed(|text| {
        let mut chars = text.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else { return false };
        // Letters, digits and space (`/[\p{L}\p{N} ]/u`); Slint's special keys live in the private-use area.
        if c == ' ' || (c.is_alphanumeric() && !('\u{e000}'..='\u{f8ff}').contains(&c)) {
            key(&c.to_lowercase().to_string());
            true
        } else {
            false
        }
    });
    page.on_open(|i| {
        let Some((kind, id)) = result(i) else { return };
        if kind == "app" {
            super::apps::open_app(&id);
        } else {
            router::go(&kind, &id);
        }
    });
    page.on_options(|i| {
        if let Some((kind, id)) = result(i) {
            actions::options(&kind, &id, "");
        }
    });
    ctx.on_event("route", |_, r| {
        if s(r, "name") != "search" {
            return;
        }
        // A fresh visit starts empty on the "a" key; coming back from a result keeps the query and focus.
        if b(r, "forward") {
            let ui = cx().ui();
            let page = ui.global::<SearchPage>();
            page.set_query("".into());
            page.set_zone(1);
            page.set_key(0);
            page.set_card(0);
        }
        rebuild();
    });
    ctx.on_event("state", |_, _| {
        if router::current().0 == "search" {
            rebuild();
        }
    });
}

fn result(i: i32) -> Option<(String, String)> {
    RESULTS.with(|r| r.borrow().get(i as usize).cloned())
}

/// The renderer's searchKey: a character, "back" or "clear".
fn key(k: &str) {
    let ui = cx().ui();
    let page = ui.global::<SearchPage>();
    let mut q = page.get_query().to_string();
    match k {
        "back" => {
            q.pop();
        }
        "clear" => q.clear(),
        _ => q.push_str(k),
    }
    page.set_query(q.into());
    page.set_card(0);
    rebuild();
}

/// Lowercase, strip accents, collapse everything that isn't a letter or digit into single spaces.
pub fn norm(s: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in s.chars().flat_map(char::to_lowercase) {
        let c = fold(c);
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    }
    out
}

fn fold(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'č' => 'c',
        'ď' => 'd',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => 'e',
        'ğ' => 'g',
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' => 'i',
        'ľ' | 'ĺ' => 'l',
        'ñ' | 'ń' | 'ň' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ō' | 'ő' => 'o',
        'ŕ' | 'ř' => 'r',
        'ś' | 'š' | 'ş' => 's',
        'ť' | 'ţ' => 't',
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' => 'u',
        'ý' | 'ÿ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        c => c,
    }
}

/// Matches for `q` (already normalised) among `list`, by `field`: every word must appear; titles that
/// start with the whole query come first, then alphabetical.
fn matches(list: Vec<Value>, field: &str, q: &str) -> Vec<Value> {
    let words: Vec<&str> = q.split(' ').collect();
    let mut out: Vec<(bool, Value)> = list
        .into_iter()
        .filter(|x| !b(x, "hidden"))
        .filter_map(|x| {
            let n = norm(s(&x, field));
            words.iter().all(|w| n.contains(w)).then(|| (n.starts_with(q), x))
        })
        .collect();
    out.sort_by(|(pa, a), (pb, b)| pb.cmp(pa).then_with(|| model::title_cmp(s(a, field), s(b, field))));
    out.into_iter().map(|(_, x)| x).collect()
}

fn app_card(a: &Value) -> Card {
    let name = s(a, "name");
    Card {
        kind: "app".into(),
        id: s(a, "id").into(),
        title: name.into(),
        // The initial, shown when there's no icon.
        subtitle: name.chars().next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default().into(),
        art: s(a, "icon").into(),
        tint: fmt::hsl(fmt::hue_of(name) as f32, 0.30, 0.24),
        progress: -1.0,
        ..Default::default()
    }
}

fn rebuild() {
    let ctx = cx();
    let ui = ctx.ui();
    let page = ui.global::<SearchPage>();
    let q = norm(&page.get_query());
    let mut flat = Vec::new();
    let mut sections = Vec::new();
    let mut total = 0;
    if !q.is_empty() {
        let apps = ctx.state.borrow().get("apps").and_then(Value::as_array).cloned().unwrap_or_default();
        let groups = [
            ("tab.games", matches(ctx.library("games"), "title", &q), false),
            ("tab.shows", matches(ctx.library("shows"), "title", &q), false),
            ("tab.movies", matches(ctx.library("movies"), "title", &q), false),
            ("tab.apps", matches(apps, "name", &q), true),
        ];
        for (title, list, apps) in groups {
            total += list.len();
            if list.is_empty() {
                continue;
            }
            let start = flat.len() as i32;
            let shown = &list[..list.len().min(if apps { 30 } else { 60 })];
            let cards: Vec<Card> = shown.iter().map(|x| if apps { app_card(x) } else { poster_card(x) }).collect();
            flat.extend(shown.iter().map(|x| (if apps { "app".to_string() } else { s(x, "type").to_string() }, s(x, "id").to_string())));
            sections.push(SearchSection { title: t(title).into(), apps, start, cards: model::model(cards) });
        }
    }
    let n = flat.len() as i32;
    RESULTS.with(|r| *r.borrow_mut() = flat);
    if page.get_card() >= n {
        page.set_card((n - 1).max(0));
    }
    page.set_total(total as i32);
    page.set_sections(model::model(sections));
}

#[cfg(test)]
mod tests {
    use super::norm;

    #[test]
    fn normalises_like_the_renderer() {
        assert_eq!(norm("  Amélie: Le Fabuleux—Destin!  "), "amelie le fabuleux destin");
        assert_eq!(norm("BA"), "ba");
        assert_eq!(norm("..."), "");
    }
}
