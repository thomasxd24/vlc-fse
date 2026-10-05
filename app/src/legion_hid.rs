//! Battery/attach reports from the Legion Go's controller halves, read from their Lenovo HID interface
//! (usage page 0xFFA0) and forwarded to the UI as `legion-report` / `legion-state` host events. WebView2
//! couldn't grant WebHID silently, so the native side reads the device and the UI feeds the reports to
//! the same parser it uses elsewhere (`Pads.parseLegionStatus`). Windows only; other platforms never
//! start it.
//!
//! On connecting it also switches the controllers' XInput output on, which Legion Space can leave off
//! (see [`enable_xinput`]).

use crate::backend::Host;
#[cfg(windows)]
use serde_json::json;
use std::sync::Arc;

#[cfg(any(windows, test))]
const USAGE_PAGE: u16 = 0xffa0;
#[cfg(any(windows, test))]
const IDS: &[(u16, &[u16])] = &[
    (0x17ef, &[0x6182, 0x6183, 0x6184, 0x6185, 0x61eb, 0x61ec, 0x61ed, 0x61ee]),
    (0x1a86, &[0xe310, 0xe311]),
];

#[cfg(any(windows, test))]
fn is_candidate(vendor: u16, product: u16, usage_page: u16) -> bool {
    usage_page == USAGE_PAGE && IDS.iter().any(|(v, ps)| *v == vendor && ps.contains(&product))
}

#[cfg(windows)]
pub fn start(host: Arc<dyn Host>) {
    std::thread::spawn(move || loop {
        let mut connected = false;
        if let Ok(api) = hidapi::HidApi::new() {
            let found = api
                .device_list()
                .find(|d| is_candidate(d.vendor_id(), d.product_id(), d.usage_page()))
                .and_then(|d| d.open_device(&api).ok());
            if let Some(dev) = found {
                connected = true;
                enable_xinput(&dev);
                host.emit("legion-state", json!({ "connected": true }));
                let mut buf = [0u8; 64];
                loop {
                    match dev.read_timeout(&mut buf, 1000) {
                        Ok(0) => continue,
                        // Numbered reports lead with their id, which WebHID's `data` leaves out.
                        Ok(n) => {
                            host.emit("legion-report", json!({ "reportId": buf[0], "bytes": buf[1..n].to_vec() }));
                        }
                        Err(_) => break,
                    }
                }
            }
        }
        if connected {
            host.emit("legion-state", json!({ "connected": false }));
        }
        std::thread::sleep(std::time::Duration::from_secs(5));
    });
}

/// Switch the controllers' XInput output back on. Legion Space turns it off while its own window is
/// in front (`05 00 04 0F <target> 02`) and on again when it leaves; the setting persists in the
/// controller, so if Legion Space exits without restoring it the pad stays silent for every app and
/// game until Legion Space runs again. Targets: 0 the receiver, 3 / 4 the left / right halves (the
/// command Legion Space sends, also Handheld Companion's `SetPhysicalXInputEnabled`).
#[cfg(windows)]
fn enable_xinput(dev: &hidapi::HidDevice) {
    for target in [0x00, 0x03, 0x04] {
        let mut report = [0u8; 64];
        report[..6].copy_from_slice(&[0x05, 0x00, 0x04, 0x0f, target, 0x01]);
        let _ = dev.write(&report);
    }
}

#[cfg(not(windows))]
pub fn start(_host: Arc<dyn Host>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_legion_controllers_only_on_the_vendor_page() {
        assert!(is_candidate(0x17ef, 0x61eb, USAGE_PAGE));
        assert!(is_candidate(0x1a86, 0xe310, USAGE_PAGE));
        assert!(!is_candidate(0x17ef, 0x61eb, 0x0001));
        assert!(!is_candidate(0x045e, 0x028e, USAGE_PAGE));
    }
}
