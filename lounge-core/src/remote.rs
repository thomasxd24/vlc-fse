//! Browsing and downloading from an SFTP, FTP or FTPS server, behind one small interface: `list(dir)`,
//! `walk(dir)`, `download(remote, local, on_bytes)`, `home()`, `close()`. Direct port of `src/remote.js`.
//!
//! SFTP is `ssh2` (libssh2 — the same library the JS `ssh2` module wraps, so the sha256-hex host-key
//! fingerprints already in people's `servers.json` keep matching). FTP/FTPS is `suppaftp`, with MLSD
//! when the server offers it and LIST-line parsing otherwise. The JS tests for this module only ever
//! ran against a real embedded SFTP server, which this crate doesn't reproduce; what's unit-tested here
//! is the parsing/sorting/walking surface plus a live-connection-free trait walk — the client itself
//! gets exercised for real only against an actual server (the Windows side-by-side pass).

use crate::locale;
use serde_json::Value;

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

// ---------------------------------------------------------------------------
// The client interface and the real SFTP / FTP / FTPS implementations
//
// SFTP is `ssh2` (libssh2 — the same library the JS version's `ssh2` module wraps, so the host-key
// fingerprints already in people's servers.json keep matching: sha256 hex of the key blob, as
// `hostHash: 'sha256'` produced). FTP/FTPS is `suppaftp` (MLSD when the server offers it, LIST-line
// parsing otherwise), with self-signed certificates accepted only where the user said so for that
// server, exactly like the JS `secureOptions`.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const TIMEOUT_MS: u64 = 15000;

/// A saved server, as `servers.json` stores it (minus the secret, which is passed separately).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerSpec {
    pub protocol: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    /// `"password"` or `"key"` (SFTP only).
    pub auth_type: String,
    pub key_path: String,
    /// The sha256-hex host-key fingerprint seen last time; empty on first use.
    pub host_key: String,
    pub insecure_tls: bool,
}

impl ServerSpec {
    pub fn from_json(v: &Value) -> ServerSpec {
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        ServerSpec {
            protocol: s("protocol"),
            host: s("host"),
            port: v.get("port").and_then(Value::as_u64).unwrap_or(0) as u16,
            username: s("username"),
            auth_type: s("authType"),
            key_path: s("keyPath"),
            host_key: s("hostKey"),
            insecure_tls: v.get("insecureTls").and_then(Value::as_bool).unwrap_or(false),
        }
    }
}

/// The browsing/download interface `main.js` builds over SFTP and FTP alike: `list(dir)`,
/// `walk(dir)`, `download(remote, local, onBytes)`, `home()`, `close()`. The transfer queue only
/// needs `download`/`close`, so the browsing methods have neutral defaults.
pub trait RemoteClient: Send + Sync {
    fn list(&self, _dir: &str) -> Result<Vec<Entry>, RemoteError> {
        Err(RemoteError { code: "unsupported".into() })
    }
    /// Everything below `dir`, depth-first, with paths relative to it. Hidden entries are skipped.
    fn walk(&self, dir: &str) -> Result<Vec<WalkEntry>, RemoteError> {
        walk(&|d| self.list(d), dir)
    }
    fn download(&self, remote: &str, local: &Path, on_bytes: &mut dyn FnMut(u64)) -> Result<(), String>;
    fn home(&self) -> String {
        "/".to_string()
    }
    fn close(&self);
}

/// SHA-256 hex of a host key blob — the fingerprint shape `hostHash: 'sha256'` produced in the JS
/// version, so previously saved `hostKey` values still match.
pub fn host_key_fingerprint(key: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(key))
}

struct SshInner {
    session: ssh2::Session,
    sftp: Option<ssh2::Sftp>,
}

/// An SFTP connection: one SSH session with an SFTP channel opened over it.
pub struct SftpClient {
    inner: Mutex<SshInner>,
}

impl SftpClient {
    fn with_sftp<T>(&self, f: impl FnOnce(&ssh2::Sftp) -> Result<T, ssh2::Error>) -> Result<T, RemoteError> {
        let mut inner = self.inner.lock().unwrap();
        if inner.sftp.is_none() {
            inner.sftp = Some(inner.session.sftp().map_err(|e| RemoteError { code: e.to_string() })?);
        }
        f(inner.sftp.as_ref().expect("just opened")).map_err(|e| {
            // SFTP's "no such file" is FX code 2, like the JS `err.code === 2` check.
            if matches!(e.code(), ssh2::ErrorCode::SFTP(2)) {
                RemoteError { code: "notFound".into() }
            } else {
                RemoteError { code: e.to_string() }
            }
        })
    }
}

impl RemoteClient for SftpClient {
    fn list(&self, dir: &str) -> Result<Vec<Entry>, RemoteError> {
        let entries = self.with_sftp(|sftp| sftp.readdir(Path::new(if dir.is_empty() { "/" } else { dir })))?;
        let list: Vec<Entry> = entries.into_iter().map(|(path, stat)| Entry {
            name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            is_dir: stat.is_dir(),
            size: stat.size.unwrap_or(0),
        }).collect();
        Ok(sort_entries(list))
    }

    fn download(&self, remote: &str, local: &Path, on_bytes: &mut dyn FnMut(u64)) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let sftp = inner.sftp.take().or_else(|| inner.session.sftp().ok()).ok_or("no sftp channel")?;
        let result = (|| {
            let mut file = sftp.open(Path::new(remote)).map_err(|e| e.to_string())?;
            let mut out = std::fs::File::create(local).map_err(|e| e.to_string())?;
            let mut buf = vec![0u8; 64 * 1024];
            let mut done: u64 = 0;
            loop {
                let n = file.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
                done += n as u64;
                on_bytes(done);
            }
            Ok(())
        })();
        // A dropped Sftp must outlive this call's borrow; put it back for the next operation.
        inner.sftp = Some(sftp);
        result
    }

    fn home(&self) -> String {
        self.with_sftp(|sftp| sftp.realpath(Path::new("."))).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "/".to_string())
    }

    fn close(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.sftp = None;
        let _ = inner.session.disconnect(Some(ssh2::DisconnectCode::ByApplication), "closing", None);
    }
}

/// Connect over SFTP. Host keys are trusted on first use: a new server reports its fingerprint
/// through `on_host_key` so it can be remembered; a changed one refuses to connect.
pub fn connect_sftp(server: &ServerSpec, secret: &str, on_host_key: Option<&dyn Fn(&str)>) -> Result<SftpClient, RemoteError> {
    if server.host.is_empty() {
        return Err(RemoteError { code: "noHost".into() });
    }
    let port = if server.port != 0 { server.port } else { DEFAULT_PORT_SFTP };
    let timeout = Duration::from_millis(TIMEOUT_MS);
    let stream = connect_tcp(&server.host, port, timeout)?;
    let mut session = ssh2::Session::new().map_err(|e| RemoteError { code: e.to_string() })?;
    session.set_timeout(timeout.as_millis() as u32);
    session.set_tcp_stream(stream);
    session.handshake().map_err(|e| RemoteError { code: e.to_string() })?;

    // Trust on first use, refuse on change — before auth, like the JS hostVerifier.
    let seen = session.host_key().map(|(key, _)| host_key_fingerprint(key)).ok_or_else(|| RemoteError { code: "no host key".into() })?;
    if !server.host_key.is_empty() && server.host_key != seen {
        let _ = session.disconnect(Some(ssh2::DisconnectCode::ByApplication), "host key changed", None);
        return Err(RemoteError { code: "hostKeyChanged".into() });
    }
    if server.host_key.is_empty() {
        if let Some(cb) = on_host_key {
            cb(&seen);
        }
    }

    if server.auth_type == "key" {
        let key = Path::new(&server.key_path);
        if !key.is_file() {
            return Err(RemoteError { code: "keyUnreadable".into() });
        }
        let passphrase = if secret.is_empty() { None } else { Some(secret) };
        session.userauth_pubkey_file(&server.username, None, key, passphrase).map_err(|_| RemoteError { code: "auth".into() })?;
    } else {
        session.userauth_password(&server.username, secret).map_err(|_| RemoteError { code: "auth".into() })?;
    }

    let sftp = session.sftp().map_err(|e| RemoteError { code: e.to_string() })?;
    Ok(SftpClient { inner: Mutex::new(SshInner { session, sftp: Some(sftp) }) })
}

fn connect_tcp(host: &str, port: u16, timeout: Duration) -> Result<TcpStream, RemoteError> {
    use std::net::ToSocketAddrs;
    let addrs: Vec<_> = (host, port).to_socket_addrs().map_err(|e| RemoteError { code: e.to_string() })?.collect();
    let mut last = RemoteError { code: "connect failed".into() };
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(s) => {
                let _ = s.set_read_timeout(Some(timeout));
                let _ = s.set_write_timeout(Some(timeout));
                return Ok(s);
            }
            Err(e) => last = RemoteError { code: e.to_string() },
        }
    }
    Err(last)
}

// ---- FTP / FTPS

enum FtpConn {
    Plain(suppaftp::FtpStream),
    Tls(Box<suppaftp::NativeTlsFtpStream>),
}

fn ftp_error(e: suppaftp::FtpError) -> RemoteError {
    match &e {
        suppaftp::FtpError::UnexpectedResponse(resp) if resp.status == suppaftp::Status::FileUnavailable => RemoteError { code: "notFound".into() },
        suppaftp::FtpError::UnexpectedResponse(resp) if resp.status == suppaftp::Status::NotLoggedIn => RemoteError { code: "auth".into() },
        _ => RemoteError { code: e.to_string() },
    }
}

/// An FTP or FTPS connection (explicit FTPS: AUTH TLS, like basic-ftp's `secure: true`).
pub struct FtpClient {
    conn: Mutex<FtpConn>,
}

impl RemoteClient for FtpClient {
    fn list(&self, dir: &str) -> Result<Vec<Entry>, RemoteError> {
        let target = if dir.is_empty() { None } else { Some(dir) };
        let mut conn = self.conn.lock().unwrap();
        // MLSD (RFC 3659) gives machine-readable entries; LIST line parsing covers the rest.
        let lines = match &mut *conn {
            FtpConn::Plain(c) => match c.mlsd(target) {
                Ok(v) => v,
                Err(_) => c.list(target).map_err(ftp_error)?,
            },
            FtpConn::Tls(c) => match c.mlsd(target) {
                Ok(v) => v,
                Err(_) => c.list(target).map_err(ftp_error)?,
            },
        };
        let mut list = Vec::new();
        for line in &lines {
            if let Some(e) = parse_mlsd_line(line).or_else(|| parse_list_line(line)) {
                if e.name != "." && e.name != ".." {
                    list.push(e);
                }
            }
        }
        Ok(sort_entries(list))
    }

    fn download(&self, remote: &str, local: &Path, on_bytes: &mut dyn FnMut(u64)) -> Result<(), String> {
        // Carry on from a partial download left by an earlier attempt.
        let start = std::fs::metadata(local).map(|m| m.len()).unwrap_or(0);
        let mut out = std::fs::OpenOptions::new().create(true).append(true).open(local).map_err(|e| e.to_string())?;
        let mut conn = self.conn.lock().unwrap();
        let finish = match &mut *conn {
            FtpConn::Plain(c) => {
                if start > 0 {
                    c.resume_transfer(start as usize).map_err(|e| e.to_string())?;
                }
                let mut stream = c.retr_as_stream(remote).map_err(|e| e.to_string())?;
                copy_with_progress(&mut stream, &mut out, start, on_bytes)?;
                Some(c.finalize_retr_stream(stream))
            }
            FtpConn::Tls(c) => {
                if start > 0 {
                    c.resume_transfer(start as usize).map_err(|e| e.to_string())?;
                }
                let mut stream = c.retr_as_stream(remote).map_err(|e| e.to_string())?;
                copy_with_progress(&mut stream, &mut out, start, on_bytes)?;
                Some(c.finalize_retr_stream(stream))
            }
        };
        if let Some(r) = finish {
            r.map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn home(&self) -> String {
        let mut conn = self.conn.lock().unwrap();
        match &mut *conn {
            FtpConn::Plain(c) => c.pwd().unwrap_or_else(|_| "/".to_string()),
            FtpConn::Tls(c) => c.pwd().unwrap_or_else(|_| "/".to_string()),
        }
    }

    fn close(&self) {
        if let Ok(mut conn) = self.conn.try_lock() {
            let _ = match &mut *conn {
                FtpConn::Plain(c) => c.quit(),
                FtpConn::Tls(c) => c.quit(),
            };
        }
    }
}

fn copy_with_progress(src: &mut impl Read, out: &mut impl Write, mut done: u64, on_bytes: &mut dyn FnMut(u64)) -> Result<u64, String> {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = src.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(done);
        }
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        on_bytes(done);
    }
}

/// One MLSD line: `type=dir;size=1024;modify=…; name`.
fn parse_mlsd_line(line: &str) -> Option<Entry> {
    let (facts, name) = line.split_once(' ')?;
    if name.is_empty() {
        return None;
    }
    let mut is_dir = false;
    let mut size = 0u64;
    for fact in facts.split(';') {
        let fact = fact.trim();
        if let Some((k, v)) = fact.split_once('=') {
            match (k, v) {
                ("type", "dir") => is_dir = true,
                ("size", n) => size = n.parse().unwrap_or(0),
                _ => {}
            }
        }
    }
    Some(Entry { name: name.to_string(), is_dir, size })
}

/// Best-effort parse of a classic LIST line: UNIX ls-style or Windows/DOS style.
fn parse_list_line(line: &str) -> Option<Entry> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.len() < 4 {
        return None;
    }
    if tokens[0].len() == 10 && (tokens[0].starts_with('-') || tokens[0].starts_with('d') || tokens[0].starts_with('l')) {
        // -rw-r--r-- 1 owner group 4096 Jan 1 12:00 name  (the name may contain spaces)
        if tokens.len() < 9 {
            return None;
        }
        let size = tokens[4].parse().unwrap_or(0);
        let name = after_token(line, 8).unwrap_or_default();
        if name.is_empty() {
            return None;
        }
        return Some(Entry { name, is_dir: tokens[0].starts_with('d'), size });
    }
    // 01-01-23  12:00PM       <DIR>          name   /   01-01-23  12:00PM        12345 name
    let is_dir = tokens[2] == "<DIR>";
    let size = if is_dir { 0 } else { tokens[2].parse().unwrap_or(0) };
    let name = after_token(line, 3).unwrap_or_default();
    if name.is_empty() {
        return None;
    }
    Some(Entry { name, is_dir, size })
}

/// Everything after the `n`-th whitespace run — the name column of a LIST line, spaces included.
fn after_token(line: &str, n: usize) -> Option<String> {
    let mut count = 0;
    let mut in_ws = false;
    for (i, b) in line.bytes().enumerate() {
        if b.is_ascii_whitespace() {
            if !in_ws {
                count += 1;
                in_ws = true;
                if count == n {
                    return Some(line[i..].trim().to_string());
                }
            }
        } else {
            in_ws = false;
        }
    }
    None
}

/// Connect to a saved server: SFTP with password or key auth, or FTP/FTPS with credentials.
pub fn connect(server: &ServerSpec, secret: &str, on_host_key: Option<&dyn Fn(&str)>) -> Result<Arc<dyn RemoteClient>, RemoteError> {
    if !has_host(Some(&server.host)) {
        return Err(RemoteError { code: "noHost".into() });
    }
    if server.protocol == "sftp" {
        Ok(Arc::new(connect_sftp(server, secret, on_host_key)?))
    } else {
        Ok(Arc::new(connect_ftp(server, secret)?))
    }
}

/// Connect over FTP, or FTPS when `protocol` is `"ftps"`. Self-signed certificates (common on home
/// NAS boxes) are accepted only when the user said so for this server (`insecureTls`).
pub fn connect_ftp(server: &ServerSpec, secret: &str) -> Result<FtpClient, RemoteError> {
    let port = if server.port != 0 { server.port } else { DEFAULT_PORT_FTP };
    let timeout = Duration::from_millis(TIMEOUT_MS);
    let username = if server.username.is_empty() { "anonymous".to_string() } else { server.username.clone() };
    let tcp = connect_tcp(&server.host, port, timeout)?;

    let conn = if server.protocol == "ftps" {
        let tls_connector = native_tls::TlsConnector::builder()
            .danger_accept_invalid_certs(server.insecure_tls)
            .danger_accept_invalid_hostnames(server.insecure_tls)
            .build()
            .map_err(|e| RemoteError { code: e.to_string() })?;
        // The stream type parameter is the *eventual* TLS stream: connect plain, then AUTH TLS.
        let plain = suppaftp::NativeTlsFtpStream::connect_with_stream(tcp).map_err(ftp_error)?;
        plain.get_ref().set_read_timeout(Some(timeout)).ok();
        let mut tls = plain.into_secure(suppaftp::NativeTlsConnector::from(tls_connector), &server.host).map_err(ftp_error)?;
        tls.login(username, secret.to_string()).map_err(ftp_error)?;
        FtpConn::Tls(Box::new(tls))
    } else {
        let mut plain = suppaftp::FtpStream::connect_with_stream(tcp).map_err(ftp_error)?;
        plain.get_ref().set_read_timeout(Some(timeout)).ok();
        plain.login(username, secret.to_string()).map_err(ftp_error)?;
        FtpConn::Plain(plain)
    };
    Ok(FtpClient { conn: Mutex::new(conn) })
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

    #[test]
    fn a_server_spec_reads_the_saved_json_shape() {
        let v = serde_json::json!({
            "id": "srv1", "name": "NAS", "protocol": "sftp", "host": "nas.local", "port": 2022,
            "username": "tom", "authType": "key", "keyPath": "C:\\keys\\id_ed25519",
            "hostKey": "abc123", "insecureTls": true
        });
        let s = ServerSpec::from_json(&v);
        assert_eq!(s.protocol, "sftp");
        assert_eq!(s.host, "nas.local");
        assert_eq!(s.port, 2022);
        assert_eq!(s.auth_type, "key");
        assert_eq!(s.host_key, "abc123");
        assert!(s.insecure_tls);
        assert_eq!(ServerSpec::from_json(&serde_json::json!({})), ServerSpec::default(), "missing fields fall back to defaults");
    }

    #[test]
    fn host_key_fingerprints_are_sha256_hex_like_the_js_version() {
        let fp = host_key_fingerprint(b"host key blob");
        assert_eq!(fp.len(), 64);
        assert!(fp.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(host_key_fingerprint(b"host key blob"), host_key_fingerprint(b"host key blob"), "stable");
        assert_ne!(host_key_fingerprint(b"a"), host_key_fingerprint(b"b"));
    }

    #[test]
    fn mlsd_lines_parse_into_entries() {
        assert_eq!(parse_mlsd_line("type=dir;size=0;modify=20230101000000; Movies"), Some(Entry { name: "Movies".into(), is_dir: true, size: 0 }));
        assert_eq!(parse_mlsd_line("type=file;size=1024; Film.mkv"), Some(Entry { name: "Film.mkv".into(), is_dir: false, size: 1024 }));
        assert_eq!(parse_mlsd_line("novalue"), None);
        assert_eq!(parse_mlsd_line("type=file; "), None);
    }

    #[test]
    fn list_lines_parse_in_unix_and_windows_shapes() {
        assert_eq!(
            parse_list_line("-rw-r--r--  1 tom  staff   4096 Jan  1 12:00 Film.mkv"),
            Some(Entry { name: "Film.mkv".into(), is_dir: false, size: 4096 })
        );
        assert_eq!(
            parse_list_line("drwxr-xr-x  2 tom  staff   4096 Jan  1 12:00 Movies"),
            Some(Entry { name: "Movies".into(), is_dir: true, size: 4096 }),
            "LIST shows a directory's block size; only MLSD gives 0"
        );
        assert_eq!(
            parse_list_line("drwxr-xr-x  2 tom  staff   4096 Jan  1 12:00 My Movies"),
            Some(Entry { name: "My Movies".into(), is_dir: true, size: 4096 }),
            "names may contain spaces"
        );
        assert_eq!(parse_list_line("01-01-23  12:00PM       <DIR>          Shows"), Some(Entry { name: "Shows".into(), is_dir: true, size: 0 }));
        assert_eq!(parse_list_line("01-01-23  12:00PM        12345 Film.mkv"), Some(Entry { name: "Film.mkv".into(), is_dir: false, size: 12345 }));
        assert_eq!(parse_list_line("total 3"), None);
        assert_eq!(parse_list_line(""), None);
    }

    /// A fake client that serves a fixed tree, to check `walk` through the trait method.
    struct TreeClient(std::collections::HashMap<String, Vec<Entry>>);

    impl RemoteClient for TreeClient {
        fn list(&self, dir: &str) -> Result<Vec<Entry>, RemoteError> {
            self.0.get(dir).cloned().ok_or_else(|| RemoteError { code: "notFound".into() })
        }
        fn download(&self, _remote: &str, _local: &Path, _on_bytes: &mut dyn FnMut(u64)) -> Result<(), String> {
            Err("unsupported".into())
        }
        fn close(&self) {}
    }

    #[test]
    fn walk_through_the_trait_walks_the_real_tree() {
        let mut tree = std::collections::HashMap::new();
        tree.insert("/".to_string(), vec![Entry { name: "Movies".into(), is_dir: true, size: 0 }, Entry { name: "film.mkv".into(), is_dir: false, size: 5 }, Entry { name: ".hidden".into(), is_dir: false, size: 1 }]);
        tree.insert("/Movies".to_string(), vec![Entry { name: "a.mkv".into(), is_dir: false, size: 2 }]);
        let client = TreeClient(tree);
        let files = client.walk("/").unwrap();
        assert_eq!(files, vec![
            WalkEntry { remote: "/Movies/a.mkv".into(), rel: "Movies/a.mkv".into(), size: 2 },
            WalkEntry { remote: "/film.mkv".into(), rel: "film.mkv".into(), size: 5 },
        ], "depth-first in listed order — the real list() sorts directories first");
    }
}
