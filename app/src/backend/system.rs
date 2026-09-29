//! System (quick menu & status bar): volume/brightness/Wi-Fi/power.

use super::platform::open_url_in_shell;
use super::{Backend, WindowOp};
use serde_json::{json, Value};

impl Backend {
    pub fn system_get(&self) -> Value {
        if !self.helper.supported() {
            return json!({"supported": false});
        }
        let call = |cmd: &str| self.helper.call(cmd, Value::Null).ok();
        json!({
            "supported": true,
            "volume": call("getVolume"),
            "muted": call("getMute"),
            "brightness": call("getBrightness"),
        })
    }

    pub fn system_set(&self, key: &str, value: Value) -> Option<Value> {
        let cmd = match key {
            "volume" => "setVolume",
            "muted" => "setMute",
            "brightness" => "setBrightness",
            _ => return None,
        };
        self.helper.call(cmd, value).ok()
    }

    pub fn wifi(&self) -> Value {
        match lounge_core::system::wifi() {
            Some(w) => json!({"connected": w.connected, "ssid": w.ssid, "signal": w.signal}),
            None => Value::Null,
        }
    }

    pub fn power(&self, action: &str) -> Option<Value> {
        use lounge_core::system::PowerAction;
        if action == "desktop" {
            self.host.window(WindowOp::Minimize);
            return None;
        }
        if !["sleep", "restart", "shutdown"].contains(&action) {
            return None;
        }
        self.flush_all();
        let parsed = match action {
            "sleep" => PowerAction::Sleep,
            "restart" => PowerAction::Restart,
            _ => PowerAction::Shutdown,
        };
        let helper_call = || self.helper.call("sleep", Value::Null).map(|_| ()).map_err(std::io::Error::other);
        lounge_core::system::power(parsed, helper_call).ok().map(|_| Value::Null)
    }

    pub fn open_external(&self, url: &str) {
        // Restricted to https:// and ms-settings:, like the original.
        if url.starts_with("https://") || url.starts_with("ms-settings:") {
            let _ = open_url_in_shell(url);
        }
    }
}
