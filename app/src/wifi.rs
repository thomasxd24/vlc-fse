//! Wi-Fi for the Wi-Fi panel: the networks in range, the current one and this machine's IP address,
//! plus scanning and connecting. Windows talks to the WLAN service directly (wlanapi); elsewhere only the
//! IP address is known. The profile building and list merging are plain Rust, tested everywhere.

/// One network in range (several access points / profiles with the same name are merged).
#[derive(Debug, Clone, PartialEq)]
pub struct Network {
    pub ssid: String,
    /// 0..100.
    pub signal: u32,
    pub secured: bool,
    pub connected: bool,
    /// Windows already has a profile for it (a network you've joined before): no password needed.
    pub profile: Option<String>,
    /// How a new profile would secure it; `None` for kinds Lounge can't set up (enterprise / 802.1X).
    pub auth: Option<Auth>,
}

// Built from the WLAN service's answers (Windows only).
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Auth {
    Open,
    WpaPsk,
    Wpa2Psk,
    Wpa3Sae,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    /// False when the machine has no Wi-Fi adapter (or isn't Windows).
    pub available: bool,
    pub networks: Vec<Network>,
}

impl Status {
    pub fn connected(&self) -> Option<&Network> {
        self.networks.iter().find(|n| n.connected)
    }
}

/// The address other devices reach this machine on: the local end of the default route (a UDP
/// "connect" sends nothing).
pub fn ip_address() -> Option<String> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_unspecified()).then(|| ip.to_string())
}

/// Merge duplicate SSIDs (one entry per profile, plus one without) and sort: connected first, then by
/// signal. Hidden networks (no name) are dropped.
pub fn merge(raw: Vec<Network>) -> Vec<Network> {
    let mut out: Vec<Network> = Vec::new();
    for n in raw.into_iter().filter(|n| !n.ssid.is_empty()) {
        match out.iter_mut().find(|o| o.ssid == n.ssid) {
            Some(o) => {
                o.signal = o.signal.max(n.signal);
                o.connected |= n.connected;
                o.secured |= n.secured;
                if o.profile.is_none() {
                    o.profile = n.profile;
                }
                if o.auth.is_none() {
                    o.auth = n.auth;
                }
            }
            None => out.push(n),
        }
    }
    out.sort_by(|a, b| b.connected.cmp(&a.connected).then(b.signal.cmp(&a.signal)).then(a.ssid.cmp(&b.ssid)));
    out
}

#[cfg_attr(not(windows), allow(dead_code))]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// A Windows WLAN profile for joining `ssid` (saved under that name, reconnecting automatically).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn profile_xml(ssid: &str, auth: Auth, password: &str) -> String {
    let hex: String = ssid.bytes().map(|b| format!("{b:02X}")).collect();
    let name = xml_escape(ssid);
    let security = match auth {
        Auth::Open => "<authEncryption><authentication>open</authentication><encryption>none</encryption><useOneX>false</useOneX></authEncryption>".to_string(),
        _ => {
            let (a, e) = match auth {
                Auth::WpaPsk => ("WPAPSK", "TKIP"),
                Auth::Wpa3Sae => ("WPA3SAE", "AES"),
                _ => ("WPA2PSK", "AES"),
            };
            format!(
                "<authEncryption><authentication>{a}</authentication><encryption>{e}</encryption><useOneX>false</useOneX></authEncryption>\
                 <sharedKey><keyType>passPhrase</keyType><protected>false</protected><keyMaterial>{}</keyMaterial></sharedKey>",
                xml_escape(password)
            )
        }
    };
    format!(
        "<?xml version=\"1.0\"?>\
         <WLANProfile xmlns=\"http://www.microsoft.com/networking/WLAN/profile/v1\">\
         <name>{name}</name>\
         <SSIDConfig><SSID><hex>{hex}</hex><name>{name}</name></SSID></SSIDConfig>\
         <connectionType>ESS</connectionType><connectionMode>auto</connectionMode>\
         <MSM><security>{security}</security></MSM>\
         </WLANProfile>"
    )
}

pub fn demo() -> Status {
    let n = |ssid: &str, signal, secured, connected, profile: bool| Network {
        ssid: ssid.into(),
        signal,
        secured,
        connected,
        profile: profile.then(|| ssid.to_string()),
        auth: Some(if secured { Auth::Wpa2Psk } else { Auth::Open }),
    };
    Status {
        available: true,
        networks: vec![
            n("Maison", 92, true, true, true),
            n("Maison-5G", 78, true, false, true),
            n("Freebox-3A2F1C", 54, true, false, false),
            n("Café du Coin", 41, false, false, false),
            n("DIRECT-HP-Printer", 22, true, false, false),
        ],
    }
}

#[cfg(windows)]
pub use win::{connect, read, scan};

#[cfg(not(windows))]
pub fn read() -> Status {
    Status::default()
}
#[cfg(not(windows))]
pub fn scan() {}
#[cfg(not(windows))]
pub fn connect(_net: &Network, _password: Option<&str>) -> Result<(), String> {
    Err("unsupported".into())
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::ptr::{null, null_mut};
    use std::time::{Duration, Instant};
    use windows_sys::core::GUID;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::NetworkManagement::WiFi::*;

    /// An open WLAN client handle and the first Wi-Fi adapter.
    struct Client {
        handle: HANDLE,
        iface: GUID,
    }

    impl Client {
        fn open() -> Option<Client> {
            let mut version = 0u32;
            let mut handle: HANDLE = null_mut();
            if unsafe { WlanOpenHandle(2, null(), &mut version, &mut handle) } != 0 {
                return None;
            }
            let mut list: *mut WLAN_INTERFACE_INFO_LIST = null_mut();
            let iface = unsafe {
                if WlanEnumInterfaces(handle, null(), &mut list) != 0 || list.is_null() {
                    None
                } else {
                    let l = &*list;
                    let g = (l.dwNumberOfItems > 0).then(|| l.InterfaceInfo[0].InterfaceGuid);
                    WlanFreeMemory(list as *const _);
                    g
                }
            };
            match iface {
                Some(iface) => Some(Client { handle, iface }),
                None => {
                    unsafe { WlanCloseHandle(handle, null()) };
                    None
                }
            }
        }

        fn networks(&self) -> Vec<Network> {
            let mut list: *mut WLAN_AVAILABLE_NETWORK_LIST = null_mut();
            let mut out = Vec::new();
            unsafe {
                if WlanGetAvailableNetworkList(self.handle, &self.iface, 0, null(), &mut list) != 0 || list.is_null() {
                    return out;
                }
                let l = &*list;
                let items = std::slice::from_raw_parts(l.Network.as_ptr(), l.dwNumberOfItems as usize);
                for n in items.iter().filter(|n| n.dot11BssType == dot11_BSS_type_infrastructure) {
                    let len = (n.dot11Ssid.uSSIDLength as usize).min(32);
                    let profile = wide(&n.strProfileName);
                    out.push(Network {
                        ssid: String::from_utf8_lossy(&n.dot11Ssid.ucSSID[..len]).into_owned(),
                        signal: n.wlanSignalQuality.min(100),
                        secured: n.bSecurityEnabled != 0,
                        connected: n.dwFlags & WLAN_AVAILABLE_NETWORK_CONNECTED != 0,
                        profile: (n.dwFlags & WLAN_AVAILABLE_NETWORK_HAS_PROFILE != 0 && !profile.is_empty()).then_some(profile),
                        auth: match n.dot11DefaultAuthAlgorithm {
                            DOT11_AUTH_ALGO_80211_OPEN if n.bSecurityEnabled == 0 => Some(Auth::Open),
                            DOT11_AUTH_ALGO_WPA_PSK => Some(Auth::WpaPsk),
                            DOT11_AUTH_ALGO_RSNA_PSK => Some(Auth::Wpa2Psk),
                            DOT11_AUTH_ALGO_WPA3_SAE => Some(Auth::Wpa3Sae),
                            _ => None,
                        },
                    });
                }
                WlanFreeMemory(list as *const _);
            }
            out
        }

        fn connect_profile(&self, profile: &str) -> bool {
            let name = to_wide(profile);
            let params = WLAN_CONNECTION_PARAMETERS {
                wlanConnectionMode: wlan_connection_mode_profile,
                strProfile: name.as_ptr(),
                pDot11Ssid: null_mut(),
                pDesiredBssidList: null_mut(),
                dot11BssType: dot11_BSS_type_infrastructure,
                dwFlags: 0,
            };
            unsafe { WlanConnect(self.handle, &self.iface, &params, null()) == 0 }
        }

        fn set_profile(&self, xml: &str) -> bool {
            let xml = to_wide(xml);
            let mut reason = 0u32;
            unsafe { WlanSetProfile(self.handle, &self.iface, 0, xml.as_ptr(), null(), 1, null(), &mut reason) == 0 }
        }

        fn delete_profile(&self, profile: &str) {
            let name = to_wide(profile);
            unsafe { WlanDeleteProfile(self.handle, &self.iface, name.as_ptr(), null()) };
        }
    }

    impl Drop for Client {
        fn drop(&mut self) {
            unsafe { WlanCloseHandle(self.handle, null()) };
        }
    }

    fn wide(s: &[u16]) -> String {
        let end = s.iter().position(|c| *c == 0).unwrap_or(s.len());
        String::from_utf16_lossy(&s[..end])
    }

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn read() -> Status {
        match Client::open() {
            Some(c) => Status { available: true, networks: merge(c.networks()) },
            None => Status::default(),
        }
    }

    /// Ask the adapter for a fresh scan; results show up in [`read`] a few seconds later.
    pub fn scan() {
        if let Some(c) = Client::open() {
            unsafe { WlanScan(c.handle, &c.iface, null(), null(), null()) };
        }
    }

    /// Join `net` (a saved network, or a new one with `password`) and wait up to 20 s for it to
    /// connect. A new profile that doesn't connect (a wrong password, usually) is removed again.
    pub fn connect(net: &Network, password: Option<&str>) -> Result<(), String> {
        let c = Client::open().ok_or("no Wi-Fi adapter")?;
        let (profile, created) = match &net.profile {
            Some(p) => (p.clone(), false),
            None => {
                let auth = net.auth.ok_or("unsupported security")?;
                if !c.set_profile(&profile_xml(&net.ssid, auth, password.unwrap_or(""))) {
                    return Err("profile rejected".into());
                }
                (net.ssid.clone(), true)
            }
        };
        if !c.connect_profile(&profile) {
            if created {
                c.delete_profile(&profile);
            }
            return Err("connect failed".into());
        }
        let until = Instant::now() + Duration::from_secs(20);
        while Instant::now() < until {
            std::thread::sleep(Duration::from_millis(700));
            if c.networks().iter().any(|n| n.connected && n.ssid == net.ssid) {
                return Ok(());
            }
        }
        if created {
            c.delete_profile(&profile);
        }
        Err("timed out".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(ssid: &str, signal: u32, connected: bool, profile: Option<&str>) -> Network {
        Network { ssid: ssid.into(), signal, secured: true, connected, profile: profile.map(Into::into), auth: Some(Auth::Wpa2Psk) }
    }

    #[test]
    fn merges_duplicates_and_puts_the_connected_network_first() {
        let list = merge(vec![
            net("B", 80, false, None),
            net("A", 40, false, None),
            net("A", 45, true, Some("A")),
            net("", 99, false, None),
        ]);
        let names: Vec<_> = list.iter().map(|n| n.ssid.as_str()).collect();
        assert_eq!(names, ["A", "B"]);
        assert_eq!((list[0].signal, list[0].connected, list[0].profile.as_deref()), (45, true, Some("A")));
    }

    #[test]
    fn profile_escapes_the_name_and_password() {
        let x = profile_xml("Tom & Jerry", Auth::Wpa2Psk, "p<w>\"d");
        assert!(x.contains("<name>Tom &amp; Jerry</name>"));
        assert!(x.contains("<hex>546F6D2026204A65727279</hex>"));
        assert!(x.contains("<authentication>WPA2PSK</authentication><encryption>AES</encryption>"));
        assert!(x.contains("<keyMaterial>p&lt;w&gt;&quot;d</keyMaterial>"));
        let open = profile_xml("Cafe", Auth::Open, "");
        assert!(open.contains("<authentication>open</authentication>") && !open.contains("sharedKey"));
    }
}
