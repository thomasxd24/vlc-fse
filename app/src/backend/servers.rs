//! Servers, remote browsing & transfers.

use super::state::transfer_state_json;
use super::Backend;
use lounge_core::remote::{self, RemoteClient, ServerSpec};
use lounge_core::transfers::AddSpec;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;

/// Passwords are encrypted with the OS's user key (DPAPI on Windows) when available; elsewhere they
/// fall back to plain base64, same as `safeStorage`'s basic mode.
fn seal_secret(plain: &str) -> String {
    if plain.is_empty() {
        return String::new();
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(sealed) = dpapi_protect(plain) {
            return format!("enc:{sealed}");
        }
    }
    use base64::Engine as _;
    format!("plain:{}", base64::engine::general_purpose::STANDARD.encode(plain))
}

fn open_secret(sealed: &str) -> String {
    if sealed.is_empty() {
        return String::new();
    }
    use base64::Engine as _;
    if let Some(rest) = sealed.strip_prefix("enc:") {
        #[cfg(target_os = "windows")]
        {
            if let Some(open) = dpapi_unprotect(rest) {
                return open;
            }
        }
        let _ = rest;
        return String::new();
    }
    if let Some(rest) = sealed.strip_prefix("plain:") {
        return base64::engine::general_purpose::STANDARD.decode(rest).ok().and_then(|b| String::from_utf8(b).ok()).unwrap_or_default();
    }
    String::new()
}

#[cfg(target_os = "windows")]
fn dpapi_protect(plain: &str) -> Option<String> {
    let script = format!(
        "$b=[System.Text.Encoding]::UTF8.GetBytes('{}'); $p=[System.Security.Cryptography.ProtectedData]::Protect($b, $null, [System.Security.Cryptography.DataProtectionScope]::CurrentUser); [Convert]::ToBase64String($p)",
        plain.replace('\'', "''")
    );
    powershell_out(&script)
}

#[cfg(target_os = "windows")]
fn dpapi_unprotect(sealed_b64: &str) -> Option<String> {
    let script = format!(
        "$p=[Convert]::FromBase64String('{}'); $b=[System.Security.Cryptography.ProtectedData]::Unprotect($p, $null, [System.Security.Cryptography.DataProtectionScope]::CurrentUser); [System.Text.Encoding]::UTF8.GetString($b)",
        sealed_b64.replace('\'', "''")
    );
    powershell_out(&script)
}

#[cfg(target_os = "windows")]
fn powershell_out(script: &str) -> Option<String> {
    let out = lounge_core::hidden_command("powershell.exe")
        .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn is_sub_name(name: &str) -> bool {
    const SUBS: [&str; 9] = [".srt", ".ass", ".ssa", ".sub", ".idx", ".vtt", ".sup", ".sami", ".smi"];
    let lower = name.to_lowercase();
    SUBS.iter().any(|ext| lower.ends_with(ext))
}

/// What a remote selection would become on disk: which library it belongs in and where each file
/// lands (`planRemote` in main.js — a folder is walked whole; a single video brings the subtitles
/// sitting next to it).
struct Planned {
    kind_str: &'static str,
    root: Value,
    roots: Value,
    files: usize,
    videos: usize,
    total_size: u64,
    folders: Vec<String>,
    plan: lounge_core::transfer_plan::Plan,
}

impl Backend {
    fn find_server(&self, id: &str) -> Option<Value> {
        self.servers.get("servers").and_then(|v| v.as_array().cloned()).unwrap_or_default().into_iter().find(|s| s.get("id").and_then(Value::as_str) == Some(id))
    }

    /// Open a fresh connection to a saved server, remembering its SSH host key the first time.
    pub(super) fn connect_server(&self, id: &str) -> Result<Arc<dyn RemoteClient>, remote::RemoteError> {
        let Some(srv) = self.find_server(id) else { return Err(remote::RemoteError { code: "noServer".into() }) };
        let spec = ServerSpec::from_json(&srv);
        let secret = open_secret(srv.get("secret").and_then(Value::as_str).unwrap_or(""));
        let on_host_key = |key: &str| {
            self.servers.update("servers", json!([]), |list| {
                if let Value::Array(a) = list {
                    if let Some(s) = a.iter_mut().find(|s| s.get("id").and_then(Value::as_str) == Some(id)) {
                        s["hostKey"] = json!(key);
                    }
                }
            });
            self.push_state();
        };
        remote::connect(&spec, &secret, Some(&on_host_key))
    }

    pub fn save_server(&self, input: Value) {
        let mut server = input.as_object().cloned().unwrap_or_default();
        let secret = server.get("secret").and_then(Value::as_str).unwrap_or("");
        // An empty secret field means "keep whatever was saved" (the UI never sees the password back).
        if server.get("hasSecret").and_then(Value::as_bool) == Some(false) || secret.is_empty() {
            if let Some(id) = server.get("id").and_then(Value::as_str) {
                if let Some(existing) = self.find_server(id) {
                    server.insert("secret".into(), existing.get("secret").cloned().unwrap_or(json!("")));
                }
            }
        } else {
            let sealed = seal_secret(secret);
            server.insert("secret".into(), json!(sealed));
        }
        server.remove("hasSecret");
        let id = server.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        if id.is_empty() {
            let new_id = format!("srv-{}", &lounge_core::games::manual_id(&format!("{:?}", server.get("host")))[5..]);
            server.insert("id".into(), json!(new_id));
        }
        self.servers.update("servers", json!([]), |list| {
            if let Value::Array(a) = list {
                let id = server.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                a.retain(|s| s.get("id").and_then(Value::as_str) != Some(id.as_str()));
                a.push(Value::Object(server.clone()));
            }
        });
        self.push_state();
    }

    pub fn remove_server(&self, id: &str) {
        self.servers.update("servers", json!([]), |list| {
            if let Value::Array(a) = list {
                a.retain(|s| s.get("id").and_then(Value::as_str) != Some(id));
            }
        });
        self.push_state();
    }

    pub fn forget_host_key(&self, id: &str) {
        self.servers.update("servers", json!([]), |list| {
            if let Value::Array(a) = list {
                if let Some(s) = a.iter_mut().find(|s| s.get("id").and_then(Value::as_str) == Some(id)) {
                    s["hostKey"] = json!("");
                }
            }
        });
        self.push_state();
    }

    /// List one directory on a server (also used by "test connection", with the root).
    fn remote_list_blocking(&self, server_id: &str, path: Option<&str>) -> Value {
        let client = match self.connect_server(server_id) {
            Ok(c) => c,
            Err(e) => return json!({"error": e.code}),
        };
        match client.list(path.unwrap_or("/")) {
            Ok(entries) => json!(entries.iter().map(|e| json!({"name": e.name, "isDir": e.is_dir, "size": e.size})).collect::<Vec<_>>()),
            Err(e) => json!({"error": e.code}),
        }
    }

    pub fn test_server(&self, id: &str) -> Value {
        self.remote_list_blocking(id, None)
    }

    pub fn remote_list(&self, server_id: &str, path: Option<&str>) -> Value {
        self.remote_list_blocking(server_id, path)
    }

    pub fn remote_plan(&self, req: Value) -> Value {
        match self.plan_remote(&req) {
            Ok(plan) => json!({
                "ok": true,
                "kind": plan.kind_str,
                "root": plan.root,
                "roots": plan.roots,
                "files": plan.files,
                "videos": plan.videos,
                "totalSize": plan.total_size,
                "folders": plan.folders,
            }),
            Err(code) => json!({"ok": false, "error": code}),
        }
    }

    fn plan_remote(&self, req: &Value) -> Result<Planned, String> {
        use lounge_core::parse;
        use lounge_core::transfer_plan::{self, PlanOptions, RemoteFile, Selection};
        let server_id = req.get("serverId").and_then(Value::as_str).unwrap_or("").to_string();
        let start = req.get("path").and_then(Value::as_str).unwrap_or("/").to_string();
        let is_dir = req.get("isDir").and_then(Value::as_bool).unwrap_or(false);

        let client = self.connect_server(&server_id).map_err(|e| e.code)?;
        let name = start.rsplit('/').find(|s| !s.is_empty()).unwrap_or("").to_string();
        let files: Vec<RemoteFile> = if is_dir {
            client.walk(&start).map_err(|e| e.code)?.into_iter().map(|w| RemoteFile { remote: w.remote, rel: w.rel, size: w.size }).collect()
        } else {
            // A single video brings the subtitles sitting next to it.
            let dir = &start[..start.len().saturating_sub(name.len())];
            let dir = dir.trim_end_matches('/');
            let dir = if dir.is_empty() { "/" } else { dir };
            let siblings = client.list(dir).unwrap_or_default();
            let me = siblings.iter().find(|e| e.name == name).map(|e| e.size).unwrap_or(0);
            let mut files = vec![RemoteFile { remote: start.clone(), rel: name.clone(), size: me }];
            if parse::is_video_file(&name) {
                let base = parse::strip_extension(&name).to_lowercase();
                for e in &siblings {
                    if !e.is_dir && e.name != name && e.name.to_lowercase().starts_with(&base) && is_sub_name(&e.name) {
                        files.push(RemoteFile { remote: format!("{}/{}", dir.trim_end_matches('/'), e.name), rel: e.name.clone(), size: e.size });
                    }
                }
            }
            files
        };
        drop(client);

        let selection = Selection { name: &name, is_dir, parent: None };
        // Kind first, against throwaway roots, then the real plan against the user's matching library.
        let probe = transfer_plan::plan_transfer(&selection, &files, &PlanOptions { kind: None, movie_root: Some(Path::new("/")), tv_root: Some(Path::new("/")), existing_show_dirs: &[] });
        let kind_str = match probe.kind {
            transfer_plan::Kind::Movie => "movie",
            transfer_plan::Kind::Tv => "tv",
        };
        let libs = self.settings.get("libraries").and_then(|v| v.as_array().cloned()).unwrap_or_default();
        let wanted = if kind_str == "tv" { "tv" } else { "movies" };
        let mut roots: Vec<String> = libs.iter().filter(|l| l.get("type").and_then(Value::as_str) == Some(wanted)).filter_map(|l| l.get("path").and_then(Value::as_str).map(String::from)).collect();
        if let Some(preferred) = req.get("library").and_then(Value::as_str).filter(|p| roots.iter().any(|r| r == p)) {
            roots.retain(|r| r != preferred);
            roots.insert(0, preferred.to_string());
        }
        let Some(root) = roots.first().cloned() else {
            return Ok(Planned { kind_str, root: Value::Null, roots: json!(roots), files: 0, videos: 0, total_size: 0, folders: vec![], plan: probe });
        };
        let existing: Vec<String> = std::fs::read_dir(&root).map(|rd| {
            rd.filter_map(|e| e.ok()).filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false)).map(|e| e.file_name().to_string_lossy().into_owned()).collect()
        }).unwrap_or_default();
        let opts = PlanOptions {
            kind: Some(probe.kind),
            movie_root: if kind_str == "movie" { Some(Path::new(&root)) } else { None },
            tv_root: if kind_str == "tv" { Some(Path::new(&root)) } else { None },
            existing_show_dirs: &existing,
        };
        let plan = transfer_plan::plan_transfer(&selection, &files, &opts);
        Ok(Planned {
            kind_str,
            root: json!(root),
            roots: json!(roots),
            files: plan.items.len(),
            videos: plan.items.iter().filter(|i| parse::is_video_file(&i.rel)).count(),
            total_size: plan.total_size,
            folders: plan.folders.iter().map(|f| f.to_string_lossy().into_owned()).collect(),
            plan,
        })
    }

    pub fn remote_download(&self, req: Value) -> Value {
        let planned = match self.plan_remote(&req) {
            Ok(p) => p,
            Err(code) => return json!({"ok": false, "error": code}),
        };
        if planned.plan.root.is_none() {
            return json!({"ok": false, "errorKey": if planned.kind_str == "tv" { "err.remote.noTvLibrary" } else { "err.remote.noMovieLibrary" }});
        }
        if planned.plan.items.is_empty() {
            return json!({"ok": false, "errorKey": "err.remote.nothing"});
        }
        let name = req.get("path").and_then(Value::as_str).unwrap_or("").rsplit('/').find(|s| !s.is_empty()).unwrap_or("").to_string();
        let title = if req.get("isDir").and_then(Value::as_bool).unwrap_or(false) { name } else { lounge_core::parse::strip_extension(&name).to_string() };
        let id = self.transfers.add(AddSpec { server_id: req.get("serverId").and_then(Value::as_str).unwrap_or("").to_string(), title, plan: planned.plan });
        self.push_state();
        json!({"ok": true, "id": id})
    }

    pub fn cancel_transfer(&self, id: &str) {
        self.transfers.cancel(id);
    }

    pub fn clear_transfers(&self, id: Option<&str>) {
        self.transfers.clear(id);
    }

    pub fn retry_transfer(&self, id: &str) -> Option<String> {
        let new_id = self.transfers.retry(id);
        self.push_state();
        new_id
    }

    /// The transfers event payload.
    pub(super) fn transfers_json(&self) -> Value {
        json!(self.transfers.state().iter().map(transfer_state_json).collect::<Vec<_>>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_round_trip_and_never_leak_plaintext() {
        assert_eq!(seal_secret(""), "");
        let sealed = seal_secret("hunter2");
        assert!(!sealed.contains("hunter2"));
        assert_eq!(open_secret(&sealed), "hunter2");
        assert_eq!(open_secret("garbage"), "");
    }

    #[test]
    fn subtitle_names() {
        assert!(is_sub_name("Movie.EN.SRT"));
        assert!(!is_sub_name("Movie.mkv"));
    }
}
