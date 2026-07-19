//! libvips-backed thumbnail and profile-image processing.

use std::path::{Path, PathBuf};

use libvips::{ops, VipsApp, VipsImage};

pub const COVER_WIDTH: i32 = 500;
pub const PAGE_THUMB_WIDTH: i32 = 312;

/// Card height-to-width ratio shared with the web UI.
const CARD_ASPECT: f64 = 99.0 / 70.0;
const TALL_CROP_RATIO: f64 = 4.0;

fn is_tall(width: i32, height: i32) -> bool {
    width > 0 && (height as f64) >= (width as f64) * TALL_CROP_RATIO
}

pub fn init_vips() -> anyhow::Result<()> {
    let app = VipsApp::new("arcagrad", false)
        .map_err(|e| anyhow::anyhow!("failed to initialize libvips: {e:?}"))?;
    app.concurrency_set(2);
    app.cache_set_max(0);
    app.cache_set_max_mem(0);
    std::mem::forget(app);
    Ok(())
}

pub fn generate_webp_thumbnail(cover: &[u8], width: i32, quality: i32) -> anyhow::Result<Vec<u8>> {
    let suffix = format!(".webp[Q={quality}]");
    if let Some(img) = enhanced_thumbnail(cover, width) {
        if let Ok(out) = img.image_write_to_buffer(&suffix) {
            return Ok(out);
        }
    }
    let image = ops::thumbnail_buffer(cover, width)
        .map_err(|e| anyhow::anyhow!("libvips thumbnail failed: {e:?}"))?;
    image
        .image_write_to_buffer(&suffix)
        .map_err(|e| anyhow::anyhow!("libvips webp encode failed: {e:?}"))
}

fn enhanced_thumbnail(cover: &[u8], width: i32) -> Option<VipsImage> {
    let header = VipsImage::new_from_buffer(cover, "").ok()?;
    if header.get_n_pages() > 1 {
        let all = VipsImage::new_from_buffer(cover, "n=-1").ok()?;
        return ops::thumbnail_image(&all, width).ok();
    }
    if is_tall(header.get_width(), header.get_height()) {
        let w = header.get_width();
        if w <= 0 {
            return None;
        }
        let scaled = ops::resize(&header, width as f64 / w as f64).ok()?;
        let card_h = (width as f64 * CARD_ASPECT).round() as i32;
        return if scaled.get_height() > card_h {
            ops::smartcrop(&scaled, width, card_h).ok()
        } else {
            Some(scaled)
        };
    }
    None
}

/// Avatar output size (square, px).
pub const AVATAR_SIZE: i32 = 256;

pub fn generate_avatar_webp(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    crop_resize_webp(bytes, |w, h| {
        let side = w.min(h);
        (side, side, AVATAR_SIZE)
    })
}

fn crop_resize_webp(
    bytes: &[u8],
    crop: fn(i32, i32) -> (i32, i32, i32),
) -> anyhow::Result<Vec<u8>> {
    let img = VipsImage::new_from_buffer(bytes, "").map_err(|e| {
        anyhow::anyhow!("not a decodable image: {e:?} | vips: {}", vips_last_error())
    })?;
    let (w, h) = (img.get_width(), img.get_height());
    if w < 1 || h < 1 {
        anyhow::bail!("image has no pixels ({w}x{h})");
    }
    let (cw, ch, target_w) = crop(w, h);
    let (cw, ch) = (cw.max(1), ch.max(1));
    let img = ops::extract_area(&img, (w - cw) / 2, (h - ch) / 2, cw, ch)
        .map_err(|e| anyhow::anyhow!("crop failed: {e:?}"))?;
    let img = ops::resize(&img, target_w as f64 / cw as f64)
        .map_err(|e| anyhow::anyhow!("resize failed: {e:?}"))?;
    let out = img
        .image_write_to_buffer(".webp[Q=82,strip]")
        .map_err(|e| anyhow::anyhow!("webp encode failed: {e:?}"))?;
    Ok(out)
}

/// Banner output width in pixels.
pub const BANNER_W: i32 = 1500;
pub const BANNER_H: i32 = 500;

pub fn generate_banner_webp(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    crop_resize_webp(bytes, |w, h| {
        if w as f64 / h as f64 > BANNER_W as f64 / BANNER_H as f64 {
            (
                ((h as i64 * BANNER_W as i64) / BANNER_H as i64) as i32,
                h,
                BANNER_W,
            )
        } else {
            (
                w,
                ((w as i64 * BANNER_H as i64) / BANNER_W as i64) as i32,
                BANNER_W,
            )
        }
    })
}

pub fn dhash(bytes: &[u8]) -> Option<u64> {
    let img = match VipsImage::new_from_buffer(bytes, "") {
        Ok(i) => i,
        Err(e) => return dhash_fail("new_from_buffer", e),
    };
    let (w, h) = (img.get_width(), img.get_height());
    if w < 1 || h < 1 {
        return dhash_fail("dimensions", format!("{w}x{h}"));
    }
    let small = match ops::resize_with_opts(
        &img,
        9.0 / w as f64,
        &ops::ResizeOptions {
            vscale: 8.0 / h as f64,
            ..Default::default()
        },
    ) {
        Ok(i) => i,
        Err(e) => return dhash_fail("resize", e),
    };
    if small.get_width() != 9 || small.get_height() != 8 {
        return dhash_fail(
            "resize dims",
            format!("{}x{}", small.get_width(), small.get_height()),
        );
    }
    let grey = match ops::colourspace(&small, ops::Interpretation::BW) {
        Ok(i) => i,
        Err(e) => return dhash_fail("colourspace", e),
    };
    let grey = match ops::extract_band(&grey, 0) {
        Ok(i) => i,
        Err(e) => return dhash_fail("extract_band", e),
    };
    let grey = match ops::cast(&grey, ops::BandFormat::Uchar) {
        Ok(i) => i,
        Err(e) => return dhash_fail("cast", e),
    };
    let px = grey.image_write_to_memory();
    if px.len() < 9 * 8 {
        return dhash_fail("image_write_to_memory", format!("{} bytes (<72)", px.len()));
    }
    Some(dhash_from_luma(&px))
}

fn dhash_fail(step: &str, e: impl std::fmt::Display) -> Option<u64> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static LOGGED: AtomicBool = AtomicBool::new(false);
    if !LOGGED.swap(true, Ordering::Relaxed) {
        tracing::warn!(
            "dhash: {step} failed: {e} | vips: {} (further dhash failures silenced)",
            vips_last_error()
        );
    }
    None
}

/// The current libvips global error buffer text (empty if none). Read-only `unsafe` FFI.
fn vips_last_error() -> String {
    unsafe {
        let p = libvips::bindings::vips_error_buffer();
        if p.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

fn dhash_from_luma(px: &[u8]) -> u64 {
    let mut hash = 0u64;
    let mut bit = 0u32;
    for y in 0..8usize {
        for x in 0..8usize {
            if px[y * 9 + x] > px[y * 9 + x + 1] {
                hash |= 1u64 << bit;
            }
            bit += 1;
        }
    }
    hash
}

fn shard(id: &str) -> (&str, &str) {
    (&id[0..2], &id[2..4])
}

pub fn cache_path(data_dir: &Path, id: &str) -> PathBuf {
    let (aa, bb) = shard(id);
    data_dir
        .join("thumbs")
        .join(aa)
        .join(bb)
        .join(format!("{id}.webp"))
}

pub fn page_cache_path(data_dir: &Path, id: &str, page: usize) -> PathBuf {
    let (aa, bb) = shard(id);
    data_dir
        .join("page-thumbs")
        .join(aa)
        .join(bb)
        .join(id)
        .join(format!("{page}.webp"))
}

pub async fn remove_item(data_dir: &Path, structural_hash: &str) {
    let _ = tokio::fs::remove_file(cache_path(data_dir, structural_hash)).await;
    if let Some(dir) = page_cache_path(data_dir, structural_hash, 0).parent() {
        let _ = tokio::fs::remove_dir_all(dir).await;
    }
}

#[cfg(test)]
mod tests {
    use super::{dhash, dhash_from_luma, init_vips};

    #[test]
    #[ignore = "needs libvips process-global init; run with --ignored"]
    fn dhash_end_to_end_is_format_agnostic() {
        use libvips::ops;
        let _ = init_vips();
        let xyz = ops::xyz(256, 64).unwrap();
        let ramp = ops::cast(&ops::extract_band(&xyz, 0).unwrap(), ops::BandFormat::Uchar).unwrap();
        let mirror = ops::flip(&ramp, ops::Direction::Horizontal).unwrap();

        let hp = dhash(&ramp.image_write_to_buffer(".png").unwrap()).expect("png decodes");
        let hj = dhash(&ramp.image_write_to_buffer(".jpg[Q=90]").unwrap()).expect("jpeg decodes");
        let hm = dhash(&mirror.image_write_to_buffer(".png").unwrap()).expect("png decodes");

        assert_eq!(hp, hj, "same image, png vs jpeg, identical hash");
        assert_ne!(hp, hm, "a mirrored ramp hashes differently");
    }

    #[test]
    #[ignore = "needs libvips process-global init; run with --ignored"]
    fn avatar_normalizes_to_square_webp() {
        use libvips::{ops, VipsImage};
        let _ = super::init_vips();
        let xyz = ops::xyz(300, 120).unwrap();
        let ramp = ops::cast(&ops::extract_band(&xyz, 0).unwrap(), ops::BandFormat::Uchar).unwrap();
        let png = ramp.image_write_to_buffer(".png").unwrap();

        let webp = super::generate_avatar_webp(&png).expect("valid image processes");
        let round = VipsImage::new_from_buffer(&webp, "").expect("output decodes");
        assert_eq!((round.get_width(), round.get_height()), (256, 256));

        assert!(super::generate_avatar_webp(b"not an image").is_err());
    }

    #[test]
    fn is_tall_gates_at_4x() {
        use super::is_tall;
        assert!(is_tall(800, 3200), "exactly 4× is tall");
        assert!(is_tall(800, 8000), "a 10× webtoon strip is tall");
        assert!(!is_tall(800, 3199), "just under 4× is not");
        assert!(!is_tall(800, 1200), "a normal ~1.5× comic page is not");
        assert!(!is_tall(2000, 1000), "a wide (landscape) page is not");
        assert!(!is_tall(0, 8000), "guard against a zero width");
    }

    #[test]
    fn dhash_from_luma_orders_adjacent_pixels() {
        let mut px = [0u8; 72];

        for y in 0..8 {
            for x in 0..9 {
                px[y * 9 + x] = ((9 - x) * 20) as u8;
            }
        }
        assert_eq!(
            dhash_from_luma(&px),
            u64::MAX,
            "decreasing ramp, all bits set"
        );

        for y in 0..8 {
            for x in 0..9 {
                px[y * 9 + x] = (x * 20) as u8;
            }
        }
        assert_eq!(dhash_from_luma(&px), 0, "increasing ramp, no bits set");

        let mut flat = [100u8; 72];
        assert_eq!(dhash_from_luma(&flat), 0, "flat, no bits");
        flat[0] = 200;
        assert_eq!(dhash_from_luma(&flat), 1, "only the row0/col0 pair, bit 0");
    }
}
