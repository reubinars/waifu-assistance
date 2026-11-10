//! Accent colour from album art.

/// The dominant colour of a BGRA image as RGB, ignoring transparent, near-black and
/// near-white pixels. `None` when nothing qualifies (the caller falls back to white).
pub fn dominant_color(bgra: &[u8], width: u32, height: u32) -> Option<[u8; 3]> {
    let pixels = (width as usize * height as usize).min(bgra.len() / 4);
    // per 4-bit-per-channel bucket: [count, sum r, sum g, sum b]
    let mut buckets = vec![[0u32; 4]; 4096];
    for &[b, g, r, a] in bgra[..pixels * 4].as_chunks::<4>().0 {
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        if a < 128 || max < 30 || min > 225 {
            continue;
        }
        let key = ((r as usize >> 4) << 8) | ((g as usize >> 4) << 4) | (b as usize >> 4);
        let e = &mut buckets[key];
        e[0] += 1;
        e[1] += r as u32;
        e[2] += g as u32;
        e[3] += b as u32;
    }
    let best = buckets.iter().filter(|e| e[0] > 0).max_by_key(|e| e[0])?;
    let n = best[0];
    Some([(best[1] / n) as u8, (best[2] / n) as u8, (best[3] / n) as u8])
}

/// `rgb` scaled up, hue kept, until its brightest channel is at least 200, so it stays visible
/// on the black island.
pub fn brighten(rgb: [u8; 3]) -> [u8; 3] {
    let max = rgb.into_iter().max().unwrap_or(0).max(1) as f32;
    if max >= 200.0 {
        return rgb;
    }
    rgb.map(|c| (c as f32 * 200.0 / max).round() as u8)
}

/// Largest image side turned into a bitmap: every Direct3D feature level accepts 2048, and
/// media thumbnails are a few hundred pixels.
pub const MAX_IMAGE_SIDE: u32 = 2048;

/// True when `bgra` holds a whole `width`×`height` 32-bit image that can become a bitmap:
/// non-empty, within [`MAX_IMAGE_SIDE`], and long enough to read `width * height * 4` bytes.
pub fn bgra_fits(bgra: &[u8], width: u32, height: u32) -> bool {
    let sides_ok = (1..=MAX_IMAGE_SIDE).contains(&width) && (1..=MAX_IMAGE_SIDE).contains(&height);
    sides_ok && bgra.len() >= width as usize * height as usize * 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brighten_keeps_hue_and_bright_colours() {
        assert_eq!(brighten([80, 20, 20]), [200, 50, 50]);
        assert_eq!(brighten([0, 0, 0]), [0, 0, 0]);
        assert_eq!(brighten([255, 10, 10]), [255, 10, 10]);
        assert_eq!(brighten([200, 200, 200]), [200, 200, 200]);
    }

    #[test]
    fn dominant_colour() {
        let red: Vec<u8> = [0u8, 0, 255, 255].repeat(16);
        assert_eq!(dominant_color(&red, 4, 4), Some([255, 0, 0]));
        let mut half = [0u8, 0, 0, 255].repeat(8);
        half.extend([255u8, 0, 0, 255].repeat(8));
        assert_eq!(dominant_color(&half, 4, 4), Some([0, 0, 255]));
        let white: Vec<u8> = [255u8; 4].repeat(16);
        assert_eq!(dominant_color(&white, 4, 4), None);
    }

    #[test]
    fn transparent_and_short_buffers_are_safe() {
        let clear: Vec<u8> = [0u8, 0, 255, 0].repeat(16);
        assert_eq!(dominant_color(&clear, 4, 4), None);
        // fewer bytes than width*height*4: only whole pixels present are read
        let short: Vec<u8> = [0u8, 0, 255, 255].repeat(3);
        assert_eq!(dominant_color(&short, 4, 4), Some([255, 0, 0]));
        assert_eq!(dominant_color(&[], 0, 0), None);
    }

    #[test]
    fn bgra_fits_only_whole_nonempty_images() {
        assert!(bgra_fits(&[0; 16], 2, 2));
        assert!(bgra_fits(&[0; 20], 2, 2), "extra bytes are harmless");
        assert!(!bgra_fits(&[0; 15], 2, 2), "short buffer would be read past its end");
        assert!(!bgra_fits(&[], 0, 0));
        assert!(!bgra_fits(&[0; 16], 0, 4));
        assert!(!bgra_fits(&[0; 16], 4, 0));
        assert!(!bgra_fits(&[0; 16], u32::MAX, u32::MAX), "size overflow");
        assert!(!bgra_fits(&vec![0; 4 * 2049], 2049, 1), "wider than every feature level accepts");
    }
}
