//! Downscaled copies of local artwork, served on `thumb://` (`http://thumb.localhost/` on Windows).
//! A webview decodes an image at its full pixel size (width x height x 4 bytes), so a 1080p Steam hero
//! shown on a 300px card still costs ~8 MB of RAM; a card needs a few dozen KB. Resized JPEGs are cached
//! on disk, keyed by path, modification time and width.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

const EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp"];

/// `/<width>/<percent-encoded path>` -> (width, path).
pub fn parse_request(url_path: &str) -> Option<(u32, PathBuf)> {
    let rest = url_path.strip_prefix('/')?;
    let (w, enc) = rest.split_once('/')?;
    let width: u32 = w.parse().ok().filter(|w| (16..=4096).contains(w))?;
    let path = PathBuf::from(percent_decode(enc)?);
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    EXTENSIONS.contains(&ext.as_str()).then_some((width, path))
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn cache_file(cache: &Path, src: &Path, width: u32) -> Option<PathBuf> {
    let modified = std::fs::metadata(src).ok()?.modified().ok()?;
    let mut h = DefaultHasher::new();
    (src, modified, width).hash(&mut h);
    Some(cache.join(format!("{:016x}.jpg", h.finish())))
}

/// The JPEG bytes of `src` shrunk to at most `width` pixels wide (never enlarged; smaller images are
/// still re-encoded so the cache is uniform).
pub fn thumbnail(cache: &Path, src: &Path, width: u32) -> Result<Vec<u8>, String> {
    let file = cache_file(cache, src, width).ok_or("unreadable")?;
    if let Ok(bytes) = std::fs::read(&file) {
        return Ok(bytes);
    }
    let img = image::open(src).map_err(|e| e.to_string())?;
    let img = if img.width() > width { img.resize(width, u32::MAX, image::imageops::FilterType::Triangle) } else { img };
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 82).encode_image(&img.to_rgb8()).map_err(|e| e.to_string())?;
    let _ = std::fs::create_dir_all(cache);
    let _ = std::fs::write(&file, &bytes);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_encoded_windows_paths_and_rejects_the_rest() {
        let (w, p) = parse_request("/480/C%3A%2FUsers%2Fme%2Fart%20work.png").unwrap();
        assert_eq!((w, p), (480, PathBuf::from("C:/Users/me/art work.png")));
        assert!(parse_request("/480/C%3A%2Fsecrets.txt").is_none(), "only images");
        assert!(parse_request("/99999/a.png").is_none());
        assert!(parse_request("/abc/a.png").is_none());
        assert!(parse_request("/480/%zz.png").is_none());
    }

    #[test]
    fn shrinks_wide_images_and_caches_the_result() {
        let dir = tempfile_dir();
        let src = dir.join("hero.png");
        image::RgbImage::from_pixel(1920, 1080, image::Rgb([200, 90, 20])).save(&src).unwrap();
        let cache = dir.join("cache");
        let bytes = thumbnail(&cache, &src, 480).unwrap();
        let out = image::load_from_memory(&bytes).unwrap();
        assert_eq!((out.width(), out.height()), (480, 270));
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1);
        assert_eq!(thumbnail(&cache, &src, 480).unwrap(), bytes, "second call is served from the cache");
    }

    fn tempfile_dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("lounge-thumbs-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}
