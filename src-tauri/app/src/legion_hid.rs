//! Battery/attach reports from the Legion Go's controller halves, read from their Lenovo HID interface
//! (usage page 0xFFA0) and forwarded to the page as `legion-report` events. WebView2 can't grant
//! WebHID silently, so the native side reads the device and the renderer feeds the reports to the same
//! parser it uses elsewhere (`Pads.parseLegionStatus`). Windows only; other platforms never start it.

#[cfg(windows)]
use serde_json::json;
#[cfg(windows)]
use tauri::Emitter;
use tauri::AppHandle;

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
pub fn start(app: AppHandle) {
    std::thread::spawn(move || loop {
        let mut connected = false;
        if let Ok(api) = hidapi::HidApi::new() {
            let found = api
                .device_list()
                .find(|d| is_candidate(d.vendor_id(), d.product_id(), d.usage_page()))
                .and_then(|d| d.open_device(&api).ok());
            if let Some(dev) = found {
                connected = true;
                let _ = app.emit("legion-state", json!({ "connected": true }));
                let mut buf = [0u8; 64];
                loop {
                    match dev.read_timeout(&mut buf, 1000) {
                        Ok(0) => continue,
                        // Numbered reports lead with their id, which WebHID's `data` leaves out.
                        Ok(n) => {
                            let _ = app.emit("legion-report", json!({ "reportId": buf[0], "bytes": buf[1..n].to_vec() }));
                        }
                        Err(_) => break,
                    }
                }
            }
        }
        if connected {
            let _ = app.emit("legion-state", json!({ "connected": false }));
        }
        std::thread::sleep(std::time::Duration::from_secs(5));
    });
}

#[cfg(not(windows))]
pub fn start(_app: AppHandle) {}

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
