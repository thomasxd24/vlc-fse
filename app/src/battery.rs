//! Battery details for the battery panel: power draw, time left, capacity, health, voltage, cycles.
//! Windows asks the battery driver directly (the IOCTL_BATTERY_* interface, no PowerShell); Linux
//! reads /sys/class/power_supply. Everything is optional: drivers leave out what they don't know.

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Info {
    pub percent: Option<u32>,
    /// "charging" | "discharging" | "full" | "idle" (plugged in, not charging)
    pub state: String,
    /// Power flowing in (charging) or out (discharging), in milliwatts; always positive.
    pub rate_mw: Option<u32>,
    pub capacity_mwh: Option<u32>,
    pub full_mwh: Option<u32>,
    pub design_mwh: Option<u32>,
    pub voltage_mv: Option<u32>,
    pub cycles: Option<u32>,
    /// The system's own estimate of time to empty, in seconds (Windows); `None` → computed from the rate.
    pub os_estimate_s: Option<u32>,
    pub name: String,
}

impl Info {
    /// Seconds until empty (discharging) or full (charging), if it can be worked out.
    pub fn seconds_left(&self) -> Option<u32> {
        let rate = self.rate_mw.filter(|r| *r > 0)? as f64;
        match self.state.as_str() {
            "discharging" => self.os_estimate_s.or_else(|| Some((self.capacity_mwh? as f64 / rate * 3600.0) as u32)),
            "charging" => {
                let missing = self.full_mwh?.saturating_sub(self.capacity_mwh?) as f64;
                Some((missing / rate * 3600.0) as u32)
            }
            _ => None,
        }
    }

    /// Full-charge capacity as a share of the design capacity (0..100+).
    pub fn health(&self) -> Option<u32> {
        let design = self.design_mwh.filter(|d| *d > 0)?;
        Some(((self.full_mwh? as f64 / design as f64) * 100.0).round() as u32)
    }
}

/// Made-up numbers for the demo (and screenshots).
pub fn demo() -> Info {
    Info {
        percent: Some(64),
        state: "discharging".into(),
        rate_mw: Some(14_300),
        capacity_mwh: Some(31_200),
        full_mwh: Some(48_700),
        design_mwh: Some(49_200),
        voltage_mv: Some(15_870),
        cycles: Some(212),
        os_estimate_s: None,
        name: "Legion Go battery".into(),
    }
}

#[cfg(windows)]
pub fn read() -> Option<Info> {
    win::read()
}

#[cfg(not(windows))]
pub fn read() -> Option<Info> {
    let dir = std::fs::read_dir("/sys/class/power_supply").ok()?.flatten().find(|e| e.file_name().to_string_lossy().starts_with("BAT"))?;
    let read = |f: &str| std::fs::read_to_string(dir.path().join(f)).ok().map(|s| s.trim().to_string());
    let num = |f: &str| read(f).and_then(|s| s.parse::<i64>().ok());
    let voltage_uv = num("voltage_now");
    // Energy in µWh, or charge in µAh converted at the present voltage.
    let energy = |e: &str, c: &str| {
        num(e).or_else(|| Some(num(c)? * voltage_uv? / 1_000_000)).map(|uwh| (uwh / 1000) as u32)
    };
    let rate_uw = num("power_now").or_else(|| Some(num("current_now")? * voltage_uv? / 1_000_000));
    let state = match read("status").as_deref() {
        Some("Charging") => "charging",
        Some("Discharging") => "discharging",
        Some("Full") => "full",
        _ => "idle",
    };
    Some(Info {
        percent: num("capacity").map(|p| p as u32),
        state: state.into(),
        rate_mw: rate_uw.map(|r| (r.unsigned_abs() / 1000) as u32),
        capacity_mwh: energy("energy_now", "charge_now"),
        full_mwh: energy("energy_full", "charge_full"),
        design_mwh: energy("energy_full_design", "charge_full_design"),
        voltage_mv: voltage_uv.map(|v| (v / 1000) as u32),
        cycles: num("cycle_count").filter(|c| *c > 0).map(|c| c as u32),
        os_estimate_s: None,
        name: [read("manufacturer"), read("model_name")].into_iter().flatten().collect::<Vec<_>>().join(" "),
    })
}

#[cfg(windows)]
mod win {
    use super::Info;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE,
        DIGCF_PRESENT, GUID_DEVCLASS_BATTERY, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
    };
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows_sys::Win32::System::Power::{
        BatteryDeviceName, BatteryEstimatedTime, BatteryInformation, BATTERY_CAPACITY_RELATIVE, BATTERY_CHARGING, BATTERY_DISCHARGING,
        BATTERY_INFORMATION, BATTERY_POWER_ON_LINE, BATTERY_QUERY_INFORMATION, BATTERY_QUERY_INFORMATION_LEVEL, BATTERY_STATUS,
        BATTERY_UNKNOWN_RATE, BATTERY_UNKNOWN_TIME, BATTERY_WAIT_STATUS, IOCTL_BATTERY_QUERY_INFORMATION, IOCTL_BATTERY_QUERY_STATUS,
        IOCTL_BATTERY_QUERY_TAG,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;

    // BATTERY_STATUS.Capacity / Voltage use this for "unknown".
    const UNKNOWN: u32 = 0xFFFF_FFFF;

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    /// DeviceIoControl with a plain-old-data input and output.
    unsafe fn ioctl<I, O>(h: HANDLE, code: u32, input: &I, output: &mut O) -> bool {
        let mut got = 0u32;
        DeviceIoControl(
            h,
            code,
            input as *const I as *const _,
            std::mem::size_of::<I>() as u32,
            output as *mut O as *mut _,
            std::mem::size_of::<O>() as u32,
            &mut got,
            null_mut(),
        ) != 0
    }

    unsafe fn query<O>(h: HANDLE, tag: u32, level: BATTERY_QUERY_INFORMATION_LEVEL, out: &mut O) -> bool {
        let q = BATTERY_QUERY_INFORMATION { BatteryTag: tag, InformationLevel: level, AtRate: 0 };
        ioctl(h, IOCTL_BATTERY_QUERY_INFORMATION, &q, out)
    }

    /// The first battery's device path (the documented SetupAPI enumeration of GUID_DEVCLASS_BATTERY).
    unsafe fn first_battery_path() -> Option<Vec<u16>> {
        let set = SetupDiGetClassDevsW(&GUID_DEVCLASS_BATTERY, null(), null_mut(), DIGCF_PRESENT | DIGCF_DEVICEINTERFACE);
        if set as isize == -1 {
            return None;
        }
        let mut iface: SP_DEVICE_INTERFACE_DATA = std::mem::zeroed();
        iface.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
        let mut path = None;
        if SetupDiEnumDeviceInterfaces(set, null(), &GUID_DEVCLASS_BATTERY, 0, &mut iface) != 0 {
            let mut needed = 0u32;
            SetupDiGetDeviceInterfaceDetailW(set, &iface, null_mut(), 0, &mut needed, null_mut());
            if needed > 0 {
                // u64-aligned buffer for the variable-length detail struct.
                let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
                let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
                (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
                if SetupDiGetDeviceInterfaceDetailW(set, &iface, detail, needed, null_mut(), null_mut()) != 0 {
                    let start = std::ptr::addr_of!((*detail).DevicePath) as *const u16;
                    let max = (needed as usize - 4) / 2;
                    let len = (0..max).take_while(|&i| *start.add(i) != 0).count();
                    let mut p: Vec<u16> = std::slice::from_raw_parts(start, len).to_vec();
                    p.push(0);
                    path = Some(p);
                }
            }
        }
        SetupDiDestroyDeviceInfoList(set);
        path
    }

    pub fn read() -> Option<Info> {
        unsafe {
            let path = first_battery_path()?;
            let h = CreateFileW(
                path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                null_mut(),
            );
            if h == INVALID_HANDLE_VALUE {
                return None;
            }
            let h = Handle(h);

            let wait = 0u32; // don't wait for a battery to appear
            let mut tag = 0u32;
            if !ioctl(h.0, IOCTL_BATTERY_QUERY_TAG, &wait, &mut tag) || tag == 0 {
                return None;
            }

            let mut bi: BATTERY_INFORMATION = std::mem::zeroed();
            let have_info = query(h.0, tag, BatteryInformation, &mut bi);
            // Relative-capacity batteries report percentages, not mWh: no energy figures then.
            let absolute = have_info && bi.Capabilities & BATTERY_CAPACITY_RELATIVE == 0;

            let ws = BATTERY_WAIT_STATUS { BatteryTag: tag, Timeout: 0, PowerState: 0, LowCapacity: 0, HighCapacity: 0 };
            let mut bs: BATTERY_STATUS = std::mem::zeroed();
            if !ioctl(h.0, IOCTL_BATTERY_QUERY_STATUS, &ws, &mut bs) {
                return None;
            }

            let mut est = UNKNOWN;
            query(h.0, tag, BatteryEstimatedTime, &mut est);
            let mut name = [0u16; 128];
            query(h.0, tag, BatteryDeviceName, &mut name);
            let name_len = name.iter().position(|c| *c == 0).unwrap_or(name.len());

            let known = |v: u32| (v != UNKNOWN && v != 0).then_some(v);
            let full = if absolute { known(bi.FullChargedCapacity) } else { None };
            let capacity = if absolute { known(bs.Capacity) } else { None };
            let rate = (bs.Rate as u32 != BATTERY_UNKNOWN_RATE && absolute).then(|| bs.Rate.unsigned_abs()).filter(|r| *r > 0);
            let state = if bs.PowerState & BATTERY_CHARGING != 0 {
                "charging"
            } else if bs.PowerState & BATTERY_DISCHARGING != 0 {
                "discharging"
            } else if bs.PowerState & BATTERY_POWER_ON_LINE != 0 && capacity.is_some() && capacity >= full {
                "full"
            } else {
                "idle"
            };
            let percent = match (capacity, full) {
                (Some(c), Some(f)) if f > 0 => Some(((c as f64 / f as f64) * 100.0).round().min(100.0) as u32),
                _ => None,
            };
            Some(Info {
                percent,
                state: state.into(),
                rate_mw: rate,
                capacity_mwh: capacity,
                full_mwh: full,
                design_mwh: if absolute { known(bi.DesignedCapacity) } else { None },
                voltage_mv: known(bs.Voltage),
                cycles: if have_info { known(bi.CycleCount) } else { None },
                os_estimate_s: (est != BATTERY_UNKNOWN_TIME && est != 0 && state == "discharging").then_some(est),
                name: String::from_utf16_lossy(&name[..name_len]),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_left_from_rate() {
        let mut i = demo();
        // 31.2 Wh at 14.3 W ≈ 2 h 11 min
        assert_eq!(i.seconds_left().map(|s| s / 60), Some(130));
        i.os_estimate_s = Some(3600);
        assert_eq!(i.seconds_left(), Some(3600), "the system's estimate wins while discharging");
        i.state = "charging".into();
        i.rate_mw = Some(17_500);
        // (48.7 - 31.2) Wh at 17.5 W = 1 h
        assert_eq!(i.seconds_left(), Some(3600));
        i.state = "full".into();
        assert_eq!(i.seconds_left(), None);
    }

    #[test]
    fn health_against_design() {
        assert_eq!(demo().health(), Some(99));
        assert_eq!(Info { full_mwh: Some(40_000), design_mwh: None, ..demo() }.health(), None);
    }
}
