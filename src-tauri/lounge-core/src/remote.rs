//! Browsing and downloading from an SFTP, FTP or FTPS server, behind one small interface: `list(dir)`,
//! `walk(dir)`, `download(remote, local, on_bytes)`, `close()`.
//!
//! Only the pure/generic pieces are ported here: [`sort_entries`] and [`walk`] (which is generic over
//! any directory lister, injected as a closure — there's no real SFTP/FTP client backing it in this
//! crate). `connect`/`connect_sftp`/`connect_ftp` — actually opening a connection — are **not** ported:
//! that needs picking an SSH/FTP crate, and `test/remote.test.js`'s own tests only exercise this module
//! through a real, embedded SFTP server (`ssh2`'s), which would mean either embedding an SSH server in
//! Rust too just to test against, or trusting an unverified client implementation — neither of which
//! this migration does elsewhere. That choice is better made once the Tauri `app` crate picks its async
//! runtime (a pure-Rust SSH crate like `russh` is tokio-based; this crate is deliberately
//! runtime-agnostic so far — see `transfers.rs`'s module doc).

use crate::locale;

pub const DEFAULT_PORT_SFTP: u16 = 22;
pub const DEFAULT_PORT_FTP: u16 = 21;
pub const DEFAULT_PORT_FTPS: u16 = 21;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteError {
    pub code: String,
}

impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.code)
    }
}

impl std::error::Error for RemoteError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

/// Directories first, then by name — case/diacritic-insensitive, natural sort (same
/// `localeCompare(..., { numeric: true, sensitivity: 'base' })` as `library.rs`'s title sort).
pub fn sort_entries(mut list: Vec<Entry>) -> Vec<Entry> {
    list.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| locale::compare_base_numeric(&a.name, &b.name)));
    list
}

pub const WALK_MAX_DEPTH: u32 = 6;
pub const WALK_MAX_FILES: usize = 5000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkEntry {
    pub remote: String,
    pub rel: String,
    pub size: u64,
}

fn posix_join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// Everything below `dir`, depth-first, with paths relative to it. Hidden entries are skipped. `list` is
/// injected (a real client's `list(dir)`, or a fake one for tests) so this needs no real connection.
pub fn walk(list: &impl Fn(&str) -> Result<Vec<Entry>, RemoteError>, dir: &str) -> Result<Vec<WalkEntry>, RemoteError> {
    let mut out = Vec::new();
    walk_inner(list, dir, 0, "", &mut out)?;
    Ok(out)
}

fn walk_inner(list: &impl Fn(&str) -> Result<Vec<Entry>, RemoteError>, dir: &str, depth: u32, rel: &str, out: &mut Vec<WalkEntry>) -> Result<(), RemoteError> {
    if depth > WALK_MAX_DEPTH || out.len() >= WALK_MAX_FILES {
        return Ok(());
    }
    for e in list(dir)? {
        if e.name.starts_with('.') {
            continue;
        }
        let r = if rel.is_empty() { e.name.clone() } else { format!("{rel}/{}", e.name) };
        let child = posix_join(dir, &e.name);
        if e.is_dir {
            walk_inner(list, &child, depth + 1, &r, out)?;
        } else {
            out.push(WalkEntry { remote: child, rel: r, size: e.size });
        }
        if out.len() >= WALK_MAX_FILES {
            break;
        }
    }
    Ok(())
}

/// Connect to a saved server. `protocol` selects the default port when `port` isn't set.
pub fn default_port(protocol: &str) -> Option<u16> {
    match protocol {
        "sftp" => Some(DEFAULT_PORT_SFTP),
        "ftp" => Some(DEFAULT_PORT_FTP),
        "ftps" => Some(DEFAULT_PORT_FTPS),
        _ => None,
    }
}

/// Whether a server selection has enough to attempt a connection at all (the JS version's
/// `!server || !server.host` guard in `connect()`).
pub fn has_host(host: Option<&str>) -> bool {
    host.map(|h| !h.is_empty()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn e(name: &str, is_dir: bool, size: u64) -> Entry {
        Entry { name: name.to_string(), is_dir, size }
    }

    #[test]
    fn sort_entries_puts_directories_first_then_natural_case_insensitive_order() {
        let list = vec![e("banana.txt", false, 1), e("Apple", true, 0), e("episode2.mkv", false, 1), e("episode10.mkv", false, 1), e("apple", true, 0)];
        let sorted = sort_entries(list);
        let names: Vec<&str> = sorted.iter().map(|e| e.name.as_str()).collect();
        // Dirs ("Apple", "apple") before files; among files, alphabetical with "episode2" before
        // "episode10" (natural sort) rather than lexicographic ("episode10" < "episode2").
        assert_eq!(names, vec!["Apple", "apple", "banana.txt", "episode2.mkv", "episode10.mkv"]);
    }

    /// An in-memory directory tree, keyed by path, standing in for a real SFTP/FTP `list()`.
    fn fake_lister(tree: HashMap<&'static str, Vec<Entry>>) -> impl Fn(&str) -> Result<Vec<Entry>, RemoteError> {
        move |dir: &str| tree.get(dir).cloned().ok_or_else(|| RemoteError { code: "notFound".into() })
    }

    #[test]
    fn walk_collects_files_depth_first_with_relative_paths_and_skips_hidden_entries() {
        let mut tree = HashMap::new();
        tree.insert("/Movies", vec![e("Heat (1995)", true, 0), e(".DS_Store", false, 1)]);
        tree.insert("/Movies/Heat (1995)", vec![e("Heat.1995.mkv", false, 123)]);
        let list = fake_lister(tree);

        let files = walk(&list, "/Movies").unwrap();
        assert_eq!(files, vec![WalkEntry { remote: "/Movies/Heat (1995)/Heat.1995.mkv".into(), rel: "Heat (1995)/Heat.1995.mkv".into(), size: 123 }]);
    }

    #[test]
    fn walk_propagates_a_listing_error() {
        let list = fake_lister(HashMap::new());
        assert_eq!(walk(&list, "/nope").unwrap_err(), RemoteError { code: "notFound".into() });
    }

    #[test]
    fn default_ports_and_host_check() {
        assert_eq!(default_port("sftp"), Some(22));
        assert_eq!(default_port("ftp"), Some(21));
        assert_eq!(default_port("ftps"), Some(21));
        assert_eq!(default_port("nope"), None);
        assert!(has_host(Some("example.com")));
        assert!(!has_host(Some("")));
        assert!(!has_host(None));
    }
}
