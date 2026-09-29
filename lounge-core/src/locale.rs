//! Small stand-in for the two `String.prototype.localeCompare` option combinations `library.js`
//! actually uses — `{ numeric: true }` and `{ sensitivity: 'base', numeric: true }` — since there's no
//! ICU collation available here. Natural-sorts embedded digit runs numerically; `sensitivity: 'base'`
//! additionally folds case and strips diacritics (the same NFKD-based approach as `parse::normalize_key`,
//! but keeping punctuation/spacing intact, since this is a comparison key, not a grouping key).

use std::cmp::Ordering;
use unicode_normalization::UnicodeNormalization;

fn fold_base(s: &str) -> String {
    let lower = s.to_lowercase();
    let decomposed: String = lower.nfkd().collect();
    decomposed.chars().filter(|c| !('\u{0300}'..='\u{036f}').contains(c)).collect()
}

/// Compare two strings the way `numeric: true` does: runs of ASCII digits compare by numeric value
/// (so "2" sorts before "10"), everything else compares by Unicode scalar value.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ac = a.chars().peekable();
    let mut bc = b.chars().peekable();
    loop {
        match (ac.peek().copied(), bc.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                if x.is_ascii_digit() && y.is_ascii_digit() {
                    let mut na = String::new();
                    while let Some(&c) = ac.peek() {
                        if c.is_ascii_digit() {
                            na.push(c);
                            ac.next();
                        } else {
                            break;
                        }
                    }
                    let mut nb = String::new();
                    while let Some(&c) = bc.peek() {
                        if c.is_ascii_digit() {
                            nb.push(c);
                            bc.next();
                        } else {
                            break;
                        }
                    }
                    let va: u128 = na.parse().unwrap_or(0);
                    let vb: u128 = nb.parse().unwrap_or(0);
                    match va.cmp(&vb) {
                        Ordering::Equal => continue,
                        other => return other,
                    }
                } else {
                    ac.next();
                    bc.next();
                    match x.cmp(&y) {
                        Ordering::Equal => continue,
                        other => return other,
                    }
                }
            }
        }
    }
}

/// `localeCompare(b, undefined, { sensitivity: 'base', numeric: true })`: case/diacritic-insensitive,
/// natural sort. Used for sorting movies/shows by title.
pub fn compare_base_numeric(a: &str, b: &str) -> Ordering {
    natural_cmp(&fold_base(a), &fold_base(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order_treats_digit_runs_numerically() {
        assert_eq!(natural_cmp("ep2", "ep10"), Ordering::Less);
        assert_eq!(natural_cmp("ep10", "ep2"), Ordering::Greater);
        assert_eq!(natural_cmp("a", "a"), Ordering::Equal);
        assert_eq!(natural_cmp("a1", "a01"), Ordering::Equal); // leading zeros don't change numeric value
    }

    #[test]
    fn base_sensitivity_ignores_case_and_diacritics() {
        assert_eq!(compare_base_numeric("Amelie", "amélie"), Ordering::Equal);
        assert_eq!(compare_base_numeric("Dark", "dark"), Ordering::Equal);
        assert_eq!(compare_base_numeric("Alien", "Heat"), Ordering::Less);
    }
}
