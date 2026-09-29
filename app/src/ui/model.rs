//! Library JSON → Slint cards, shared by every screen that shows posters or episode stills (the
//! renderer's `posterCard`, `episodeCard`, `continueCard`, `gameWideCard`).

use super::fmt;
use super::i18n::{t, tv};
use crate::Card;
use serde_json::Value;
use slint::{ModelRc, VecModel};

pub fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

pub fn n(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

pub fn b(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

pub fn arr<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key).and_then(Value::as_array).map(|a| a.as_slice()).unwrap_or(&[])
}

pub fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items))
}

/// A movie / show / game poster (2:3) with the grid's sublabel and badges.
pub fn poster_card(item: &Value) -> Card {
    let kind = s(item, "type");
    let title = s(item, "title");
    let mut card = Card {
        kind: kind.into(),
        id: s(item, "id").into(),
        title: title.into(),
        art: s(item, "poster").into(),
        tint: fmt::tint_of(title),
        progress: -1.0,
        favorite: b(item, "favorite"),
        ..Default::default()
    };
    match kind {
        "movie" => {
            let pr = item.get("progress").cloned().unwrap_or(Value::Null);
            card.watched = b(&pr, "watched");
            if b(&pr, "resumable") {
                card.progress = fmt::pct(&pr);
            }
            let year = n(item, "year");
            card.subtitle = if year > 0.0 { format!("{}", year as i64).into() } else { "".into() };
        }
        "show" => {
            let eps = arr(item, "episodes").len() as i64;
            let watched = n(item, "watchedCount") as i64;
            let left = eps - watched;
            if left == 0 && eps > 0 {
                card.watched = true;
            } else if watched > 0 {
                card.count = left.to_string().into();
            }
            card.subtitle = tv("n.seasons", &[("n", arr(item, "seasons").len().into())]).into();
        }
        _ => {
            let last = n(item, "lastPlayed") as i64;
            card.subtitle = if last > 0 {
                fmt::ago(last)
            } else if s(item, "source") == "steam" {
                "Steam".into()
            } else {
                t("game.notPlayed")
            }
            .into();
            if card.art.is_empty() {
                // No cover: the icon (if any) over the generated gradient, drawn as the placeholder.
                card.logo = s(item, "icon").into();
            }
        }
    }
    card
}

/// An episode still (16:9): number + title, runtime · year (or time left), progress and watched badge.
pub fn episode_card(e: &Value, show: &Value) -> Card {
    let pr = e.get("progress").cloned().unwrap_or(Value::Null);
    let episode = n(e, "episode") as i64;
    let end = e.get("episodeEnd").and_then(Value::as_i64).map(|x| format!("–{x}")).unwrap_or_default();
    let title = match s(e, "title") {
        "" => tv("ep.n", &[("n", episode.into())]),
        t => t.to_string(),
    };
    let runtime = n(e, "runtime") as i64;
    let year = s(e, "airDate").get(0..4).unwrap_or("").to_string();
    let meta = [fmt::runtime(runtime), year].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
    Card {
        kind: "episode".into(),
        id: s(e, "id").into(),
        show_id: s(show, "id").into(),
        title: format!("{episode}{end}  {title}").into(),
        subtitle: if meta.is_empty() && b(&pr, "resumable") { fmt::remaining(&pr) } else { meta }.into(),
        art: s(e, "thumb").into(),
        tint: fmt::tint_of(s(show, "title")),
        progress: if b(&pr, "resumable") { fmt::pct(&pr) } else { -1.0 },
        watched: b(&pr, "watched"),
        wide: true,
        ..Default::default()
    }
}

/// A Continue watching entry (`{ kind: movie|episode, id, showId }`) as a wide still with time left.
pub fn continue_card(c: &Value, movies: &[Value], shows: &[Value]) -> Option<Card> {
    match s(c, "kind") {
        "movie" => {
            let m = movies.iter().find(|m| s(m, "id") == s(c, "id"))?;
            let pr = m.get("progress").cloned().unwrap_or(Value::Null);
            Some(Card {
                kind: "movie".into(),
                id: s(m, "id").into(),
                title: s(m, "title").into(),
                subtitle: fmt::remaining(&pr).into(),
                art: match s(m, "backdrop") {
                    "" => s(m, "poster"),
                    b => b,
                }
                .into(),
                tint: fmt::tint_of(s(m, "title")),
                progress: fmt::pct(&pr),
                wide: true,
                ..Default::default()
            })
        }
        _ => {
            let show = shows.iter().find(|x| s(x, "id") == s(c, "showId"))?;
            let e = arr(show, "episodes").iter().find(|e| s(e, "id") == s(c, "id"))?;
            let pr = e.get("progress").cloned().unwrap_or(Value::Null);
            let sub = [fmt::ep_code(e), if b(&pr, "resumable") { fmt::remaining(&pr) } else { String::new() }]
                .into_iter()
                .filter(|x| !x.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            Some(Card {
                kind: "episode".into(),
                id: s(e, "id").into(),
                show_id: s(show, "id").into(),
                title: s(show, "title").into(),
                subtitle: sub.into(),
                art: match s(e, "thumb") {
                    "" => s(show, "backdrop"),
                    t => t,
                }
                .into(),
                tint: fmt::tint_of(s(show, "title")),
                progress: if b(&pr, "resumable") { fmt::pct(&pr) } else { -1.0 },
                wide: true,
                ..Default::default()
            })
        }
    }
}

/// A game as a wide card (hero / header art with its logo).
pub fn game_wide_card(g: &Value) -> Card {
    let art = [s(g, "hero"), s(g, "header"), s(g, "poster")].into_iter().find(|x| !x.is_empty()).unwrap_or("");
    let sub = [fmt::ago(n(g, "lastPlayed") as i64), fmt::playtime(n(g, "playtime") as i64)]
        .into_iter()
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    Card {
        kind: "game".into(),
        id: s(g, "id").into(),
        title: s(g, "title").into(),
        subtitle: sub.into(),
        art: art.into(),
        logo: s(g, "logo").into(),
        tint: fmt::tint_of(s(g, "title")),
        progress: -1.0,
        favorite: b(g, "favorite"),
        wide: true,
        ..Default::default()
    }
}

/// The backdrop for an item: hero / backdrop / header, falling back to the poster.
pub fn backdrop_of(item: &Value) -> String {
    ["hero", "backdrop", "header", "poster"].iter().map(|k| s(item, k)).find(|x| !x.is_empty()).unwrap_or("").to_string()
}

/// Case-insensitive, accent-insensitive title order with numbers compared numerically
/// (`localeCompare(…, { sensitivity: 'base', numeric: true })`).
pub fn title_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let key = |s: &str| -> Vec<Result<u64, String>> {
        let folded: String = s.chars().flat_map(fold).collect();
        let mut out = Vec::new();
        let mut num = String::new();
        let mut text = String::new();
        for c in folded.chars() {
            if c.is_ascii_digit() {
                if !text.is_empty() {
                    out.push(Err(std::mem::take(&mut text)));
                }
                num.push(c);
            } else {
                if !num.is_empty() {
                    out.push(Ok(num.parse().unwrap_or(u64::MAX)));
                    num.clear();
                }
                text.push(c);
            }
        }
        if !num.is_empty() {
            out.push(Ok(num.parse().unwrap_or(u64::MAX)));
        }
        if !text.is_empty() {
            out.push(Err(text));
        }
        out
    };
    let (ka, kb) = (key(a), key(b));
    for (x, y) in ka.iter().zip(kb.iter()) {
        let o = match (x, y) {
            (Ok(p), Ok(q)) => p.cmp(q),
            (Err(p), Err(q)) => p.cmp(q),
            (Ok(_), Err(_)) => std::cmp::Ordering::Less,
            (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
        };
        if o != std::cmp::Ordering::Equal {
            return o;
        }
    }
    ka.len().cmp(&kb.len())
}

fn fold(c: char) -> Vec<char> {
    let base = match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' => 'a',
        'ç' | 'Ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' | 'È' | 'É' | 'Ê' | 'Ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' | 'Ì' | 'Í' | 'Î' | 'Ï' => 'i',
        'ñ' | 'Ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' => 'o',
        'ù' | 'ú' | 'û' | 'ü' | 'Ù' | 'Ú' | 'Û' | 'Ü' => 'u',
        'ý' | 'ÿ' | 'Ý' => 'y',
        c => return c.to_lowercase().collect(),
    };
    vec![base]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_order() {
        let mut v = vec!["Zelda", "élan", "Alpha 10", "Alpha 2", "beta"];
        v.sort_by(|a, b| title_cmp(a, b));
        assert_eq!(v, vec!["Alpha 2", "Alpha 10", "beta", "élan", "Zelda"]);
    }
}
