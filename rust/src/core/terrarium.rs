//! The Terrarium heightmap encoding: heights from a tile's RGB, and back (for tests and for
//! seeding caches). No I/O, no engine.
//!
//! Never interpolate Terrarium RGB per channel: adjacent heights can be distant in channel
//! space (g wraps 255→0 as r carries), so encoding quantizes once and splits bytes.

use image::RgbImage;

/// Terrarium: h = r·256 + g + b/256 − 32768 (meters).
pub fn decode_terrarium_floats(img: &RgbImage) -> Vec<f32> {
    img.pixels()
        .map(|p| f32::from(p[0]) * 256.0 + f32::from(p[1]) + f32::from(p[2]) / 256.0 - 32768.0)
        .collect()
}

/// Encode heights (meters) into Terrarium RGB. Carry-safe: quantize once to 24-bit fixed point
/// and split bytes — never round per channel (that spikes at g-wrap boundaries).
pub fn encode_terrarium(heights: &[f32], w: u32, h: u32) -> RgbImage {
    let mut img = RgbImage::new(w, h);
    for (i, px) in img.pixels_mut().enumerate() {
        let fixed = ((f64::from(heights[i]) + 32768.0) * 256.0)
            .round()
            .clamp(0.0, f64::from(0xFF_FFFF)) as u32;
        px[0] = (fixed >> 16) as u8;
        px[1] = ((fixed >> 8) & 0xFF) as u8;
        px[2] = (fixed & 0xFF) as u8;
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_quarter_meter() {
        let heights = [
            0.0f32,
            8848.0,
            -415.0,
            100.5,
            1234.25,
            255.996_1,
            256.0,
            -0.003_906_25,
        ];
        let img = encode_terrarium(&heights, heights.len() as u32, 1);
        let decoded = decode_terrarium_floats(&img);
        for (a, b) in heights.iter().zip(&decoded) {
            assert!((a - b).abs() <= 1.0 / 256.0 + 1e-4, "{a} vs {b}");
        }
    }

    #[test]
    fn carry_safe_at_g_wrap() {
        let img = encode_terrarium(&[255.998], 1, 1);
        let p = img.get_pixel(0, 0);
        let fixed = (u32::from(p[0]) << 16) + (u32::from(p[1]) << 8) + u32::from(p[2]);
        assert!(fixed == 8_454_143 || fixed == 8_454_144, "fixed = {fixed}");
    }
}
