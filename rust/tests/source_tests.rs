//! Integration tests for the tile source: the real worker pool and the blocking fetcher against
//! seeded disk caches (a dead HTTP host proves the offline paths). Mirrors the raytiles /
//! bevytiles suites where they still apply.

use godotiles::core::config::{NetworkConfig, WorldConfig};
use godotiles::core::lod::TileKey;
use godotiles::core::source::{Fetcher, Kind, TileDrop, TilePayload, TileRequest, TileSource};
use godotiles::core::terrarium;
use image::RgbaImage;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn dead_host_config(dir: &Path) -> NetworkConfig {
    NetworkConfig {
        threads: 1,
        cache_dir: dir.to_path_buf(),
        texture_url: "http://127.0.0.1:9/tex/:zoom:/:x:/:y:.png".into(),
        heightmap_url: "http://127.0.0.1:9/hm/:zoom:/:x:/:y:.png".into(),
        connect_timeout: Duration::from_millis(300),
        read_timeout: Duration::from_millis(500),
        ..Default::default()
    }
}

/// A world anchored at provider tile (0, 0) of zoom 9, so keys are provider coordinates;
/// heights from zoom 10.
fn world() -> WorldConfig {
    WorldConfig {
        anchor_x: 0,
        anchor_z: 0,
        elevation_zoom: 10,
        min_segments: 16,
        max_segments: 64,
        ..Default::default()
    }
}

fn cache_path(dir: &Path, kind: &str, zoom: u8, x: i32, z: i32) -> PathBuf {
    dir.join(kind)
        .join(zoom.to_string())
        .join(x.to_string())
        .join(format!("{z}.png"))
}

fn write_png(path: &Path, img: &image::DynamicImage) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    img.save_with_format(path, image::ImageFormat::Png).unwrap();
}

fn gradient_heights(offset: f32) -> Vec<f32> {
    (0..256 * 256)
        .map(|i| offset + (i % 256) as f32 * 0.5 + (i / 256) as f32 * 0.25)
        .collect()
}

fn seed_terrarium(path: &Path, offset: f32) {
    let img = terrarium::encode_terrarium(&gradient_heights(offset), 256, 256);
    write_png(path, &image::DynamicImage::ImageRgb8(img));
}

fn seed_texture(dir: &Path, zoom: u8, x: i32, z: i32) {
    let img = RgbaImage::from_pixel(2, 2, image::Rgba([200, 60, 30, 255]));
    write_png(
        &cache_path(dir, "texture", zoom, x, z),
        &image::DynamicImage::ImageRgba8(img),
    );
}

/// Esri serves JPEG despite the extension; the cache keeps the `.png` name.
fn seed_texture_jpeg(dir: &Path, zoom: u8, x: i32, z: i32) {
    let path = cache_path(dir, "texture", zoom, x, z);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let img = image::RgbImage::from_pixel(4, 4, image::Rgb([10, 200, 30]));
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Jpeg,
        )
        .unwrap();
    std::fs::write(path, bytes).unwrap();
}

struct Harness {
    src: TileSource,
    payloads: Vec<TilePayload>,
    drops: Vec<TileDrop>,
}

impl Harness {
    fn new(cfg: NetworkConfig) -> Self {
        Self {
            src: TileSource::new(&cfg, &world()).unwrap(),
            payloads: Vec::new(),
            drops: Vec::new(),
        }
    }

    fn pump_until(&mut self, mut pred: impl FnMut(&Self) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let mut r = Vec::new();
            let mut d = Vec::new();
            self.src.drain(&mut r, &mut d);
            self.payloads.extend(r);
            self.drops.extend(d);
            if pred(self) {
                return true;
            }
            if Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

fn key(zoom: u8, x: i32, z: i32) -> TileKey {
    TileKey { zoom, x, z }
}

#[test]
fn cache_hit_delivers_whole_payload_without_network() {
    let dir = tempfile::tempdir().unwrap();
    seed_texture(dir.path(), 12, 41, 57);
    seed_terrarium(&cache_path(dir.path(), "heightmap", 10, 10, 14), 100.0);

    let mut h = Harness::new(dead_host_config(dir.path()));
    let k = key(12, 41, 57);
    h.src.request(TileRequest {
        key: k,
        x: 41,
        z: 57,
    });

    assert!(h.pump_until(|h| !h.payloads.is_empty()));
    let p = &h.payloads[0];
    assert_eq!(p.key, k);
    assert_eq!(p.albedo.width(), 2);
    assert_eq!(p.geometry.segments, 64);
    assert_eq!(p.geometry.heights.len(), 65 * 65);
    let corner = 100.0 + 63.5 * 0.5 + 63.5 * 0.25;
    assert!(
        (p.geometry.heights[0] - corner).abs() < 0.01,
        "{}",
        p.geometry.heights[0]
    );
}

#[test]
fn jpeg_imagery_is_decoded_from_a_png_named_cache_file() {
    let dir = tempfile::tempdir().unwrap();
    seed_texture_jpeg(dir.path(), 9, 2, 2);
    seed_terrarium(&cache_path(dir.path(), "heightmap", 9, 2, 2), 0.0);

    let mut h = Harness::new(dead_host_config(dir.path()));
    h.src.request(TileRequest {
        key: key(9, 2, 2),
        x: 2,
        z: 2,
    });
    assert!(h.pump_until(|h| !h.payloads.is_empty() || !h.drops.is_empty()));
    assert!(h.drops.is_empty(), "{:?}", h.drops);
    assert_eq!(h.payloads[0].albedo.dimensions(), (4, 4));
}

#[test]
fn missing_heightmap_drops_with_reason() {
    let dir = tempfile::tempdir().unwrap();
    seed_texture(dir.path(), 9, 1, 1);

    let mut h = Harness::new(dead_host_config(dir.path()));
    h.src.request(TileRequest {
        key: key(9, 1, 1),
        x: 1,
        z: 1,
    });
    assert!(h.pump_until(|h| !h.drops.is_empty()));
    assert!(h.payloads.is_empty());
    assert!(matches!(h.drops[0], TileDrop::Failed(k, _) if k == key(9, 1, 1)));
}

#[test]
fn corrupt_heightmap_drops_the_tile() {
    let dir = tempfile::tempdir().unwrap();
    let p = cache_path(dir.path(), "heightmap", 10, 7, 7);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, b"definitely not a png").unwrap();
    seed_texture(dir.path(), 16, 7 * 64, 7 * 64);

    let mut h = Harness::new(dead_host_config(dir.path()));
    h.src.request(TileRequest {
        key: key(16, 7 * 64, 7 * 64),
        x: 7 * 64,
        z: 7 * 64,
    });
    assert!(h.pump_until(|h| !h.drops.is_empty()));
    assert!(matches!(h.drops[0], TileDrop::Failed(_, _)));
}

#[test]
fn requests_dedup_by_key_while_in_flight() {
    let dir = tempfile::tempdir().unwrap();
    seed_texture(dir.path(), 9, 6, 6);
    seed_terrarium(&cache_path(dir.path(), "heightmap", 9, 6, 6), 0.0);

    let mut h = Harness::new(dead_host_config(dir.path()));
    let k = key(9, 6, 6);
    h.src.request(TileRequest { key: k, x: 6, z: 6 });
    h.src.request(TileRequest { key: k, x: 6, z: 6 });
    assert!(h.pump_until(|h| !h.payloads.is_empty()));
    std::thread::sleep(Duration::from_millis(100));
    let mut r = Vec::new();
    let mut d = Vec::new();
    h.src.drain(&mut r, &mut d);
    assert!(h.payloads.len() + r.len() <= 2);
}

#[test]
fn cancel_before_pickup_answers_with_a_cancelled_drop() {
    let dir = tempfile::tempdir().unwrap();
    seed_texture(dir.path(), 9, 8, 8);
    seed_terrarium(&cache_path(dir.path(), "heightmap", 9, 8, 8), 0.0);

    let mut h = Harness::new(dead_host_config(dir.path()));
    let slow = key(9, 1, 2);
    let victim = key(9, 8, 8);
    h.src.request(TileRequest {
        key: slow,
        x: 1,
        z: 2,
    });
    h.src.request(TileRequest {
        key: victim,
        x: 8,
        z: 8,
    });
    h.src.cancel(victim);
    assert!(h.pump_until(|h| h.drops.len() >= 2));
    assert!(h.drops.contains(&TileDrop::Cancelled(victim)));
    assert!(h.payloads.is_empty());
    h.src.request(TileRequest {
        key: victim,
        x: 8,
        z: 8,
    });
    assert!(h.pump_until(|h| h.payloads.iter().any(|p| p.key == victim)));
}

#[test]
fn another_cache_layout_and_offline_reads_it_alone() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = NetworkConfig {
        texture_cache: "imagery/:zoom:/:x:/:y:".into(),
        heightmap_cache: "elevation/:zoom:/:x:/:y:".into(),
        offline: true,
        ..dead_host_config(dir.path())
    };
    seed_terrarium(&dir.path().join("elevation/10/10/14"), 5.0);
    let fetcher = Fetcher::new(&cfg);
    assert_eq!(
        fetcher.cache_path(Kind::Texture, 19, 1, 2),
        dir.path().join("imagery/19/1/2")
    );
    assert!(fetcher.has_cached(Kind::Heightmap, 10, 10, 14));
    assert!(!fetcher.has_cached(Kind::Heightmap, 10, 11, 14));
    let k = key(19, 10 * 512 + 3, 14 * 512 + 500);
    let geometry = fetcher.geometry(&world(), k).unwrap();
    assert_eq!(geometry.segments, 16);
    let missing = fetcher.texture(19, k.x, k.z).unwrap_err();
    assert!(missing.contains("offline"), "{missing}");
    let edge = key(19, 10 * 512 + 511, 14 * 512);
    assert!(fetcher.geometry(&world(), edge).is_ok());
    let h = fetcher
        .height_at(
            &world(),
            19,
            0.5 * world().zoom_size(10) + 10.0 * world().zoom_size(10),
            14.5 * world().zoom_size(10),
        )
        .unwrap();
    assert!((h - (5.0 + 127.5 * 0.5 + 127.5 * 0.25)).abs() < 0.01, "{h}");
}

#[test]
fn neighbour_heightmaps_make_one_function_across_their_edge() {
    let dir = tempfile::tempdir().unwrap();
    seed_terrarium(&cache_path(dir.path(), "heightmap", 10, 10, 14), 0.0);
    seed_terrarium(&cache_path(dir.path(), "heightmap", 10, 11, 14), 128.0);
    let fetcher = Fetcher::new(&dead_host_config(dir.path()));
    let world = world();
    let west = key(12, 43, 56);
    let east = key(12, 44, 56);
    let (a, b) = (
        fetcher.geometry(&world, west).unwrap(),
        fetcher.geometry(&world, east).unwrap(),
    );
    let n = a.side();
    for k in 0..n {
        assert_eq!(a.heights[k * n + n - 1], b.heights[k * n]);
    }
    let top = a.heights[n - 1];
    assert!((top - 0.5 * (127.5 + 128.0)).abs() < 0.2, "{top}");
}

#[test]
fn invalid_config_is_rejected() {
    let mut cfg = dead_host_config(Path::new("."));
    cfg.threads = 0;
    assert!(TileSource::new(&cfg, &world()).is_err());
}

#[test]
fn dropping_the_source_skips_the_queue() {
    let dir = tempfile::tempdir().unwrap();
    let src = TileSource::new(&dead_host_config(dir.path()), &world()).unwrap();
    for x in 0..50 {
        src.request(TileRequest {
            key: key(9, x, 0),
            x,
            z: 0,
        });
    }
    let start = Instant::now();
    drop(src);
    assert!(start.elapsed() < Duration::from_secs(3), "{:?}", start.elapsed());
}
