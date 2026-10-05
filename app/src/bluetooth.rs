//! Bluetooth audio for the Bluetooth panel: the sound outputs (pick the default), the paired Bluetooth
//! audio devices (connect / disconnect), and AirPods battery levels read from their Bluetooth LE
//! broadcasts. Windows only; the parsing is plain Rust, tested everywhere.
//!
//! - Outputs and devices come from the Windows audio device API. A Bluetooth headset has one audio
//!   endpoint per profile (stereo, hands-free), each behind a Bluetooth kernel-streaming filter; the
//!   filter takes the documented one-shot reconnect / disconnect requests (`KSPROPSETID_BtAudio`).
//! - The default output is set through `IPolicyConfig`, the (undocumented, long-stable) interface
//!   Windows' own sound settings use.
//! - AirPods announce their battery levels in Apple "proximity pairing" broadcasts; a watcher listens
//!   for them only while the panel is open.

/// A sound output (speakers, headphones…).
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub id: String,
    pub name: String,
    pub default: bool,
}

/// A paired Bluetooth audio device (its profiles merged).
#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub name: String,
    pub connected: bool,
    /// The kernel-streaming filters to send reconnect / disconnect to.
    pub filters: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    pub available: bool,
    pub outputs: Vec<Output>,
    pub devices: Vec<Device>,
}

/// What a pair of AirPods says about itself. Levels are percentages (multiples of 10); `None` when a
/// bud or the case isn't reporting (out of the case and asleep, or the case closed).
#[derive(Debug, Clone, PartialEq)]
pub struct AirPods {
    pub model: &'static str,
    pub left: Option<u8>,
    pub right: Option<u8>,
    pub case: Option<u8>,
    pub charging_left: bool,
    pub charging_right: bool,
    pub charging_case: bool,
}

/// Apple's Bluetooth company id.
#[cfg_attr(not(windows), allow(dead_code))]
pub const APPLE: u16 = 0x004c;

/// Parse Apple manufacturer data (the bytes after the company id) when it's an AirPods "proximity
/// pairing" broadcast. The layout is the one OpenPods and AirPodsDesktop read: a type byte (0x07),
/// length, prefix, the model, a status byte whose 0x20 bit says which bud is the "primary" (the
/// battery nibbles swap with it), the two buds' levels, then charging flags and the case level.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse_airpods(data: &[u8]) -> Option<AirPods> {
    if data.len() < 9 || data[0] != 0x07 {
        return None;
    }
    let model = match u16::from_be_bytes([data[3], data[4]]) {
        0x0220 => "AirPods",
        0x0f20 => "AirPods (2nd generation)",
        0x1320 => "AirPods (3rd generation)",
        0x1920 | 0x1b20 => "AirPods 4",
        0x0e20 => "AirPods Pro",
        0x1420 | 0x2420 => "AirPods Pro 2",
        0x2720 => "AirPods Pro 3",
        0x0a20 | 0x1f20 => "AirPods Max",
        _ => return None,
    };
    let flipped = data[5] & 0x20 == 0;
    let (hi, lo) = (data[6] >> 4, data[6] & 0x0f);
    let (left, right) = if flipped { (hi, lo) } else { (lo, hi) };
    let charge = data[7] >> 4;
    let level = |v: u8| (v <= 10).then_some(v * 10);
    Some(AirPods {
        model,
        left: level(left),
        right: level(right),
        case: level(data[7] & 0x0f),
        charging_left: charge & if flipped { 0b10 } else { 0b01 } != 0,
        charging_right: charge & if flipped { 0b01 } else { 0b10 } != 0,
        charging_case: charge & 0b100 != 0,
    })
}

/// Profile suffixes Windows adds to a headset's endpoint names; stripped to group them by device.
#[cfg_attr(not(windows), allow(dead_code))]
fn device_name(interface_name: &str) -> String {
    let n = interface_name.trim();
    for suffix in [" Hands-Free AG Audio", " Hands-Free AG", " Hands-Free", " Stereo"] {
        if let Some(base) = n.strip_suffix(suffix) {
            return base.trim().to_string();
        }
    }
    n.to_string()
}

/// Merge one entry per (profile endpoint) into one per device.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn group_devices(raw: Vec<(String, String, bool)>) -> Vec<Device> {
    let mut out: Vec<Device> = Vec::new();
    for (interface_name, filter, connected) in raw {
        let name = device_name(&interface_name);
        match out.iter_mut().find(|d| d.name == name) {
            Some(d) => {
                d.connected |= connected;
                if !d.filters.contains(&filter) {
                    d.filters.push(filter);
                }
            }
            None => out.push(Device { name, connected, filters: vec![filter] }),
        }
    }
    out.sort_by(|a, b| b.connected.cmp(&a.connected).then(a.name.cmp(&b.name)));
    out
}

pub fn demo() -> Status {
    Status {
        available: true,
        outputs: vec![
            Output { id: "spk".into(), name: "Speakers (Realtek(R) Audio)".into(), default: false },
            Output { id: "pods".into(), name: "Headphones (AirPods Pro)".into(), default: true },
        ],
        devices: vec![
            Device { name: "AirPods Pro".into(), connected: true, filters: vec!["pods".into()] },
            Device { name: "WH-1000XM4".into(), connected: false, filters: vec!["sony".into()] },
        ],
    }
}

pub fn demo_airpods() -> AirPods {
    AirPods { model: "AirPods Pro", left: Some(80), right: Some(70), case: Some(50), charging_left: false, charging_right: false, charging_case: true }
}

#[cfg(windows)]
pub use win::{connect, latest_airpods, read, set_default_output, watch_airpods};

#[cfg(not(windows))]
pub fn read() -> Status {
    Status::default()
}
#[cfg(not(windows))]
pub fn connect(_device: &Device, _on: bool) -> bool {
    false
}
#[cfg(not(windows))]
pub fn set_default_output(_id: &str) -> bool {
    false
}
#[cfg(not(windows))]
pub fn watch_airpods(_on: bool) {}
#[cfg(not(windows))]
pub fn latest_airpods() -> Option<AirPods> {
    None
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use windows::core::PCWSTR;
    use windows::Devices::Bluetooth::Advertisement::{
        BluetoothLEAdvertisementReceivedEventArgs, BluetoothLEAdvertisementWatcher, BluetoothLEScanningMode,
    };
    use windows::Foundation::TypedEventHandler;
    use windows::Storage::Streams::DataReader;
    use windows::Win32::Devices::FunctionDiscovery::{PKEY_DeviceInterface_FriendlyName, PKEY_Device_FriendlyName};
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::Media::Audio::{
        eCommunications, eConsole, eMultimedia, eRender, IDeviceTopology, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
        DEVICE_STATE, DEVICE_STATE_ACTIVE, DEVICE_STATE_UNPLUGGED,
    };
    use windows::Win32::Media::KernelStreaming::{
        IKsControl, KSIDENTIFIER, KSIDENTIFIER_0, KSIDENTIFIER_0_0, KSPROPERTY_ONESHOT_DISCONNECT, KSPROPERTY_ONESHOT_RECONNECT,
        KSPROPERTY_TYPE_GET, KSPROPSETID_BtAudio,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ};

    /// `IPolicyConfig` (Windows 7 and later): only `SetDefaultEndpoint` is called; the methods before
    /// it are declared to keep the vtable in order.
    #[allow(non_snake_case)]
    mod policy {
        use windows::core::{interface, IUnknown, IUnknown_Vtbl, HRESULT, PCWSTR};
        use windows::Win32::Foundation::PROPERTYKEY;
        use windows::Win32::Media::Audio::ERole;

        #[interface("f8679f50-850a-41cf-9c72-430f290290c8")]
        pub unsafe trait IPolicyConfig: IUnknown {
            fn GetMixFormat(&self, id: PCWSTR, format: *mut *mut core::ffi::c_void) -> HRESULT;
            fn GetDeviceFormat(&self, id: PCWSTR, default: i32, format: *mut *mut core::ffi::c_void) -> HRESULT;
            fn ResetDeviceFormat(&self, id: PCWSTR) -> HRESULT;
            fn SetDeviceFormat(&self, id: PCWSTR, endpoint: *mut core::ffi::c_void, mix: *mut core::ffi::c_void) -> HRESULT;
            fn GetProcessingPeriod(&self, id: PCWSTR, default: i32, default_period: *mut i64, min_period: *mut i64) -> HRESULT;
            fn SetProcessingPeriod(&self, id: PCWSTR, period: *mut i64) -> HRESULT;
            fn GetShareMode(&self, id: PCWSTR, mode: *mut core::ffi::c_void) -> HRESULT;
            fn SetShareMode(&self, id: PCWSTR, mode: *mut core::ffi::c_void) -> HRESULT;
            fn GetPropertyValue(&self, id: PCWSTR, key: *const PROPERTYKEY, value: *mut core::ffi::c_void) -> HRESULT;
            fn SetPropertyValue(&self, id: PCWSTR, key: *const PROPERTYKEY, value: *mut core::ffi::c_void) -> HRESULT;
            fn SetDefaultEndpoint(&self, id: PCWSTR, role: ERole) -> HRESULT;
        }

        pub fn set_default(pc: &IPolicyConfig, id: PCWSTR, role: ERole) -> bool {
            unsafe { pc.SetDefaultEndpoint(id, role).is_ok() }
        }
    }
    use policy::IPolicyConfig;
    const CLSID_POLICY_CONFIG: windows::core::GUID = windows::core::GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

    fn com() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
    }

    fn enumerator() -> Option<IMMDeviceEnumerator> {
        com();
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok() }
    }

    fn take(p: windows::core::PWSTR) -> String {
        let s = unsafe { p.to_string() }.unwrap_or_default();
        unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
        s
    }

    fn property(d: &IMMDevice, key: &PROPERTYKEY) -> String {
        unsafe { d.OpenPropertyStore(STGM_READ).and_then(|s| s.GetValue(key)).map(|v| v.to_string()).unwrap_or_default() }
    }

    /// The kernel-streaming filter behind an endpoint, when it's a Bluetooth one.
    fn bt_filter(d: &IMMDevice) -> Option<String> {
        unsafe {
            let topo: IDeviceTopology = d.Activate(CLSCTX_ALL, None).ok()?;
            let conn = topo.GetConnector(0).ok()?;
            let id = take(conn.GetDeviceIdConnectedTo().ok()?);
            id.to_ascii_lowercase().contains("bthenum").then_some(id)
        }
    }

    pub fn read() -> Status {
        let Some(en) = enumerator() else { return Status::default() };
        let mut st = Status { available: true, ..Default::default() };
        unsafe {
            let default = en.GetDefaultAudioEndpoint(eRender, eMultimedia).ok().and_then(|d| d.GetId().ok()).map(take);
            if let Ok(list) = en.EnumAudioEndpoints(eRender, DEVICE_STATE(DEVICE_STATE_ACTIVE.0 | DEVICE_STATE_UNPLUGGED.0)) {
                let mut raw = Vec::new();
                for i in 0..list.GetCount().unwrap_or(0) {
                    let Ok(d) = list.Item(i) else { continue };
                    let Ok(id) = d.GetId().map(take) else { continue };
                    let active = d.GetState().map(|s| s == DEVICE_STATE_ACTIVE).unwrap_or(false);
                    if active {
                        st.outputs.push(Output { name: property(&d, &PKEY_Device_FriendlyName), default: default.as_deref() == Some(id.as_str()), id });
                    }
                    if let Some(filter) = bt_filter(&d) {
                        raw.push((property(&d, &PKEY_DeviceInterface_FriendlyName), filter, active));
                    }
                }
                st.devices = group_devices(raw);
            }
        }
        st
    }

    /// Ask Windows to reconnect (`on`) or disconnect the device's audio. Returns once asked; the
    /// endpoints change state a few seconds later.
    pub fn connect(device: &Device, on: bool) -> bool {
        let Some(en) = enumerator() else { return false };
        let id = if on { KSPROPERTY_ONESHOT_RECONNECT } else { KSPROPERTY_ONESHOT_DISCONNECT };
        let prop = KSIDENTIFIER {
            Anonymous: KSIDENTIFIER_0 { Anonymous: KSIDENTIFIER_0_0 { Set: KSPROPSETID_BtAudio, Id: id.0 as u32, Flags: KSPROPERTY_TYPE_GET } },
        };
        let mut ok = false;
        for filter in &device.filters {
            let wide: Vec<u16> = filter.encode_utf16().chain(std::iter::once(0)).collect();
            unsafe {
                let Ok(d) = en.GetDevice(PCWSTR(wide.as_ptr())) else { continue };
                let Ok(ks) = d.Activate::<IKsControl>(CLSCTX_ALL, None) else { continue };
                let mut returned = 0u32;
                ok |= ks.KsProperty(&prop, std::mem::size_of::<KSIDENTIFIER>() as u32, std::ptr::null_mut(), 0, &mut returned).is_ok();
            }
        }
        ok
    }

    /// Make `id` the default output for everything (media, games, calls).
    pub fn set_default_output(id: &str) -> bool {
        com();
        let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let Ok(pc) = CoCreateInstance::<_, IPolicyConfig>(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL) else { return false };
            [eConsole, eMultimedia, eCommunications].iter().all(|r| policy::set_default(&pc, PCWSTR(wide.as_ptr()), *r))
        }
    }

    /// The strongest AirPods broadcast heard in the last few seconds.
    static LATEST: Mutex<Option<(AirPods, i16, Instant)>> = Mutex::new(None);
    static WATCHER: Mutex<Option<BluetoothLEAdvertisementWatcher>> = Mutex::new(None);
    /// Ignore AirPods further away than this (someone else's, across the room).
    const MIN_RSSI: i16 = -70;
    const FRESH: Duration = Duration::from_secs(8);

    /// Listen for AirPods broadcasts (`on`) or stop listening.
    pub fn watch_airpods(on: bool) {
        let mut w = WATCHER.lock().unwrap();
        if !on {
            if let Some(w) = w.take() {
                let _ = w.Stop();
            }
            return;
        }
        if w.is_some() {
            return;
        }
        let Ok(watcher) = BluetoothLEAdvertisementWatcher::new() else { return };
        let _ = watcher.SetScanningMode(BluetoothLEScanningMode::Passive);
        let handler = TypedEventHandler::<BluetoothLEAdvertisementWatcher, BluetoothLEAdvertisementReceivedEventArgs>::new(|_, args| {
            let Some(args) = args.as_ref() else { return Ok(()) };
            let rssi = args.RawSignalStrengthInDBm()?;
            if rssi < MIN_RSSI {
                return Ok(());
            }
            for m in args.Advertisement()?.ManufacturerData()? {
                if m.CompanyId()? != APPLE {
                    continue;
                }
                let buf = m.Data()?;
                let mut bytes = vec![0u8; buf.Length()? as usize];
                DataReader::FromBuffer(&buf)?.ReadBytes(&mut bytes)?;
                if let Some(p) = parse_airpods(&bytes) {
                    let mut l = LATEST.lock().unwrap();
                    let now = Instant::now();
                    // Keep the nearest pair: replace a weaker reading only once it's gone stale.
                    let replace = match l.as_ref() {
                        Some((_, r, at)) => rssi >= *r - 3 || now.duration_since(*at) > FRESH,
                        None => true,
                    };
                    if replace {
                        *l = Some((p, rssi, now));
                    }
                }
            }
            Ok(())
        });
        if watcher.Received(&handler).is_ok() && watcher.Start().is_ok() {
            *w = Some(watcher);
        }
    }

    pub fn latest_airpods() -> Option<AirPods> {
        let l = LATEST.lock().unwrap();
        l.as_ref().filter(|(_, _, at)| at.elapsed() < FRESH).map(|(p, _, _)| p.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_airpods_pro_broadcast() {
        // Type 07, length 19, prefix 01, model 0e20, status 2b (0x20 set: not flipped), buds 8 / 7
        // (right / left), charging flags 4 (case) with the case at 5.
        let data = [0x07, 0x19, 0x01, 0x0e, 0x20, 0x2b, 0x87, 0x45, 0x00, 0x00];
        let p = parse_airpods(&data).unwrap();
        assert_eq!(p.model, "AirPods Pro");
        assert_eq!((p.left, p.right, p.case), (Some(70), Some(80), Some(50)));
        assert_eq!((p.charging_left, p.charging_right, p.charging_case), (false, false, true));
        // Flipped (0x20 clear): the bud nibbles swap.
        let p = parse_airpods(&[0x07, 0x19, 0x01, 0x14, 0x20, 0x0b, 0x87, 0x1f, 0x00]).unwrap();
        assert_eq!((p.model, p.left, p.right, p.case), ("AirPods Pro 2", Some(80), Some(70), None));
        assert!(p.charging_right && !p.charging_left);
        assert!(parse_airpods(&[0x10, 0x05, 0x01, 0x0e, 0x20, 0x2b, 0x87, 0x45, 0x00]).is_none());
        assert!(parse_airpods(&[0x07, 0x19, 0x01, 0x99, 0x99, 0x2b, 0x87, 0x45, 0x00]).is_none());
    }

    #[test]
    fn groups_a_headsets_profiles_into_one_device() {
        let d = group_devices(vec![
            ("WH-1000XM4".into(), "f1".into(), false),
            ("AirPods Pro".into(), "a1".into(), true),
            ("AirPods Pro Hands-Free".into(), "a2".into(), false),
        ]);
        assert_eq!(d.len(), 2);
        assert_eq!((d[0].name.as_str(), d[0].connected, d[0].filters.len()), ("AirPods Pro", true, 2));
        assert_eq!((d[1].name.as_str(), d[1].connected), ("WH-1000XM4", false));
    }
}
