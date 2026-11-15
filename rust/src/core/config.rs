//! Engine-agnostic configuration: plain Rust mirrors of the Godot config resources, their
//! defaults, validation, and geo anchoring. Mirrors raytiles' nested
//! `config { world, streaming, rendering, network }`.

use super::lod::{LodPolicy, TileKey};
use glam::Vec3;
use std::path::PathBuf;
use std::time::Duration;

/// Lowest zoom level the engine supports; `WorldConfig::base_zoom` must not go below it.
pub const MIN_ZOOM: u8 = 9;
/// Highest zoom level the engine supports; `WorldConfig::max_zoom` and every ring's zoom must
/// not exceed it.
pub const MAX_ZOOM: u8 = 22;
/// Number of zoom levels in `[MIN_ZOOM, MAX_ZOOM]`; sizes every per-zoom array. Slot `i`
/// applies to zoom `base_zoom + i`; slots beyond `max_zoom - base_zoom` are ignored.
pub const ZOOM_LEVELS: usize = (MAX_ZOOM - MIN_ZOOM + 1) as usize;

/// The WGS84 equator, which web-mercator tiles divide.
pub const EQUATOR_CIRCUMFERENCE_M: f64 = 40_075_016.686;

/// Default per-zoom subdivision thresholds (meters), z9 → z22.
pub const DEFAULT_THRESHOLDS: [f32; ZOOM_LEVELS] = [
    100_000.0, 80_000.0, 40_000.0, 20_000.0, 10_000.0, 5_000.0, 2_500.0, 1_250.0, 625.0, 312.0,
    156.0, 78.0, 39.0, 20.0,
];

/// World topology and the terrain's height function. Effectively immutable once streaming
/// started.
#[derive(Clone, Debug, PartialEq)]
pub struct WorldConfig {
    /// Anchor tile X at `base_zoom`: the world origin sits at this tile's corner.
    pub anchor_x: i32,
    /// Anchor tile Z (slippy-map `y`) at `base_zoom`.
    pub anchor_z: i32,
    /// Lowest LOD zoom ever loaded (>= MIN_ZOOM).
    pub base_zoom: u8,
    /// Highest quadtree LOD zoom (<= MAX_ZOOM). Imagery must exist this deep; heights above
    /// `elevation_zoom` are sampled from it.
    pub max_zoom: u8,
    /// World size (meters) of one tile at `base_zoom`.
    pub tile_size: f32,
    /// World-space offset of the anchor point inside its anchor tile; filled by `from_lat_lon`
    /// so the origin sits exactly on the coordinate.
    pub origin_offset: Vec3,
    /// Zoom of the Terrarium heightmaps every tile's heights come from: a tile at or below it
    /// samples its own zoom's, a finer tile this one's. One continuous height function per
    /// zoom, read across tile edges, so neighbours meet exactly (see
    /// [`geometry`](super::geometry)).
    pub elevation_zoom: u8,
    /// Terrain relief exaggeration applied to every height.
    pub height_scale: f32,
    /// Fewest mesh cells along a tile's side (a fine tile over coarse elevation still bends
    /// with it).
    pub min_segments: u32,
    /// Most mesh cells along a tile's side.
    pub max_segments: u32,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self {
            anchor_x: 306,
            anchor_z: 207,
            base_zoom: MIN_ZOOM,
            max_zoom: 15,
            tile_size: 66_400.0,
            origin_offset: Vec3::ZERO,
            elevation_zoom: 15,
            height_scale: 1.0,
            min_segments: 4,
            max_segments: 64,
        }
    }
}

/// Result of the web-mercator anchoring math, kept separate so the Godot resource can apply it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeoAnchor {
    /// Anchor tile X at `MIN_ZOOM`.
    pub anchor_x: i32,
    /// Anchor tile Z at `MIN_ZOOM`.
    pub anchor_z: i32,
    /// World size (meters) of one tile at `MIN_ZOOM`.
    pub tile_size: f64,
    /// The coordinate's offset inside its anchor tile.
    pub origin_offset: Vec3,
}

/// Derive the anchor tile (at `MIN_ZOOM`), tile size, and origin offset for a geographic
/// coordinate in degrees (web-mercator, same math as raytiles / bevytiles).
pub fn geo_anchor(lat: f64, lon: f64) -> GeoAnchor {
    geo_anchor_on(lat, lon, EQUATOR_CIRCUMFERENCE_M)
}

/// [`geo_anchor`] with the meters of a given sphere's circumference, to agree with another
/// system's geodesy (a GPS model's sphere, say) on how far a degree is.
pub fn geo_anchor_on(lat: f64, lon: f64, circumference_m: f64) -> GeoAnchor {
    let n = 2f64.powi(MIN_ZOOM as i32);
    let lat_rad = lat.to_radians();
    let x = (lon + 180.0) / 360.0 * n;
    let y = (1.0 - (lat_rad.tan() + 1.0 / lat_rad.cos()).ln() / std::f64::consts::PI) / 2.0 * n;
    let anchor_x = x.floor() as i32;
    let anchor_z = y.floor() as i32;
    let tile_size = circumference_m * lat_rad.cos() / n;
    GeoAnchor {
        anchor_x,
        anchor_z,
        tile_size,
        origin_offset: Vec3::new(
            ((x - f64::from(anchor_x)) * tile_size) as f32,
            0.0,
            ((y - f64::from(anchor_z)) * tile_size) as f32,
        ),
    }
}

impl WorldConfig {
    /// Anchor the world at a geographic coordinate (degrees). Assumes `base_zoom == MIN_ZOOM`.
    pub fn from_lat_lon(lat: f64, lon: f64) -> Self {
        Self::from_lat_lon_on(lat, lon, EQUATOR_CIRCUMFERENCE_M)
    }

    /// [`from_lat_lon`](Self::from_lat_lon) on a sphere of the given circumference.
    pub fn from_lat_lon_on(lat: f64, lon: f64, circumference_m: f64) -> Self {
        let g = geo_anchor_on(lat, lon, circumference_m);
        Self {
            anchor_x: g.anchor_x,
            anchor_z: g.anchor_z,
            tile_size: g.tile_size as f32,
            origin_offset: g.origin_offset,
            ..Default::default()
        }
    }

    /// A sensible initial camera position over the anchor, `altitude` m up.
    pub fn initial_position(&self, altitude: f32) -> Vec3 {
        self.origin_offset + Vec3::Y * altitude
    }

    /// World size (meters) of one tile at `zoom`: halves per level above base.
    pub fn zoom_size(&self, zoom: u8) -> f64 {
        f64::from(self.tile_size) / 2f64.powi(i32::from(zoom) - i32::from(self.base_zoom))
    }

    /// Absolute position (x, z meters) of a provider tile coordinate at `zoom` (fractional:
    /// `x + u` within tile `x`). f64 throughout.
    pub fn tile_to_absolute(&self, zoom: u8, x: f64, y: f64) -> (f64, f64) {
        let scale = 2f64.powi(i32::from(zoom) - i32::from(self.base_zoom));
        let size = f64::from(self.tile_size);
        (
            (x / scale - f64::from(self.anchor_x)) * size,
            (y / scale - f64::from(self.anchor_z)) * size,
        )
    }

    /// The provider tile coordinate at `zoom` (fractional) of an absolute position: the inverse
    /// of [`tile_to_absolute`](Self::tile_to_absolute).
    pub fn absolute_to_tile(&self, zoom: u8, x: f64, z: f64) -> (f64, f64) {
        let scale = 2f64.powi(i32::from(zoom) - i32::from(self.base_zoom));
        let size = f64::from(self.tile_size);
        (
            (x / size + f64::from(self.anchor_x)) * scale,
            (z / size + f64::from(self.anchor_z)) * scale,
        )
    }

    /// The anchor-relative key of the tile at `zoom` holding an absolute position.
    pub fn key_at(&self, zoom: u8, x: f64, z: f64) -> TileKey {
        let size = self.zoom_size(zoom);
        TileKey {
            zoom,
            x: (x / size).floor() as i32,
            z: (z / size).floor() as i32,
        }
    }

    /// Absolute provider tile (x, y) of an anchor-relative key.
    pub fn provider_tile(&self, key: TileKey) -> (i32, i32) {
        let scale = 1i32 << (key.zoom - self.base_zoom);
        (key.x + self.anchor_x * scale, key.z + self.anchor_z * scale)
    }

    /// The zoom of the heightmaps a tile at `zoom` samples.
    pub fn elevation_zoom_for(&self, zoom: u8) -> u8 {
        zoom.min(self.elevation_zoom)
    }

    /// Checks the zoom range, tile size and segment bounds.
    pub fn validate(&self) -> Result<(), String> {
        if self.base_zoom < MIN_ZOOM {
            return Err(format!(
                "base_zoom {} is below min supported zoom {MIN_ZOOM}",
                self.base_zoom
            ));
        }
        if self.max_zoom > MAX_ZOOM {
            return Err(format!(
                "max_zoom {} is above max supported zoom {MAX_ZOOM}",
                self.max_zoom
            ));
        }
        if self.max_zoom < self.base_zoom {
            return Err(format!(
                "max_zoom {} is below base_zoom {}",
                self.max_zoom, self.base_zoom
            ));
        }
        if !(self.base_zoom..=MAX_ZOOM).contains(&self.elevation_zoom) {
            return Err(format!(
                "elevation_zoom {} outside [{}, {MAX_ZOOM}]",
                self.elevation_zoom, self.base_zoom
            ));
        }
        if self.tile_size <= 0.0 || self.tile_size.is_nan() {
            return Err(format!("tile_size {} must be positive", self.tile_size));
        }
        if self.min_segments == 0 {
            return Err("min_segments must be >= 1".into());
        }
        Ok(())
    }
}

/// Which tiles are kept resident and how aggressively the set updates.
#[derive(Clone, Debug, PartialEq)]
pub struct StreamingConfig {
    /// Radius, in base-zoom tiles, of the disc loaded around the focus (quadtree policy).
    pub radius: i32,
    /// Per-zoom subdivision distance thresholds (meters), `thresholds[zoom - base_zoom]`
    /// (quadtree policy).
    pub thresholds: [f32; ZOOM_LEVELS],
    /// Which tiles the focus wants: the adaptive quadtree, or fixed rings.
    pub policy: LodPolicy,
    /// Focus travel (meters) that triggers a desired-set rebuild.
    pub update_distance: f32,
    /// Wall-clock budget per frame for promoting tiles to the GPU.
    pub upload_budget: Duration,
    /// Hard cap on promotions per frame, on top of the budget.
    pub max_uploads_per_frame: usize,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            radius: 6,
            thresholds: DEFAULT_THRESHOLDS,
            policy: LodPolicy::Quadtree,
            update_distance: 500.0,
            upload_budget: Duration::from_micros(2000),
            max_uploads_per_frame: 8,
        }
    }
}

impl StreamingConfig {
    /// Checks the radius, the per-frame cap and every ring.
    pub fn validate(&self, world: &WorldConfig) -> Result<(), String> {
        if matches!(self.policy, LodPolicy::Quadtree) && self.radius < 1 {
            return Err(format!("radius {} must be >= 1", self.radius));
        }
        if self.max_uploads_per_frame == 0 {
            return Err("max_uploads_per_frame must be >= 1".into());
        }
        if let LodPolicy::Rings(rings) = &self.policy {
            for ring in rings {
                if !(world.base_zoom..=MAX_ZOOM).contains(&ring.zoom) {
                    return Err(format!(
                        "ring zoom {} outside [{}, {MAX_ZOOM}]",
                        ring.zoom, world.base_zoom
                    ));
                }
                if ring.radius < 0 {
                    return Err(format!("ring radius {} must be >= 0", ring.radius));
                }
            }
        }
        Ok(())
    }
}

/// Rendering parameters, read when a tile is promoted. The terrain is drawn with Godot's
/// `StandardMaterial3D`, so lights, shadows and fog are the scene's.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderingConfig {
    /// Depth of the skirt hung from every tile's edges, in the tile's mesh cells. Hides the
    /// cracks where zoom levels meet; 0 disables.
    pub skirt_cells: f32,
    /// Whether tiles cast shadows (they always receive them).
    pub cast_shadows: bool,
    /// RGBA (sRGB) of a tile drawn without imagery.
    pub no_imagery_color: [f32; 4],
}

impl Default for RenderingConfig {
    fn default() -> Self {
        Self {
            skirt_cells: 0.5,
            cast_shadows: false,
            no_imagery_color: [0.35, 0.4, 0.32, 1.0],
        }
    }
}

/// Tile download / cache parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct NetworkConfig {
    /// Number of background download / geometry worker threads.
    pub threads: usize,
    /// Root of the on-disk cache.
    pub cache_dir: PathBuf,
    /// Imagery URL template with `:zoom:`/`:x:`/`:y:` tokens. The Esri default uses `zoom/y/x`
    /// order — that swap is intentional.
    pub texture_url: String,
    /// Terrarium heightmap URL template (`zoom/x/y` order).
    pub heightmap_url: String,
    /// Where an imagery response body is kept, under `cache_dir`, with the same tokens. Another
    /// layout shares the cache with another program.
    pub texture_cache: String,
    /// Where a heightmap response body is kept, under `cache_dir`.
    pub heightmap_cache: String,
    /// Read the cache alone, never HTTP: a tile missing from it fails.
    pub offline: bool,
    /// The User-Agent header of every request.
    pub user_agent: String,
    /// HTTP connection timeout per request.
    pub connect_timeout: Duration,
    /// HTTP read timeout per request.
    pub read_timeout: Duration,
}

/// Esri World Imagery.
pub const DEFAULT_TEXTURE_URL: &str =
    "https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/:zoom:/:y:/:x:";
/// Mapzen Terrarium heightmaps on AWS.
pub const DEFAULT_HEIGHTMAP_URL: &str =
    "https://s3.amazonaws.com/elevation-tiles-prod/terrarium/:zoom:/:x:/:y:.png";
/// The default imagery cache layout.
pub const DEFAULT_TEXTURE_CACHE: &str = "texture/:zoom:/:x:/:y:.png";
/// The default heightmap cache layout.
pub const DEFAULT_HEIGHTMAP_CACHE: &str = "heightmap/:zoom:/:x:/:y:.png";
/// The default User-Agent.
pub const DEFAULT_USER_AGENT: &str = concat!("godotiles/", env!("CARGO_PKG_VERSION"));

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            threads: 4,
            cache_dir: PathBuf::from(".cache"),
            texture_url: DEFAULT_TEXTURE_URL.into(),
            heightmap_url: DEFAULT_HEIGHTMAP_URL.into(),
            texture_cache: DEFAULT_TEXTURE_CACHE.into(),
            heightmap_cache: DEFAULT_HEIGHTMAP_CACHE.into(),
            offline: false,
            user_agent: DEFAULT_USER_AGENT.into(),
            connect_timeout: Duration::from_secs(5),
            read_timeout: Duration::from_secs(3),
        }
    }
}

impl NetworkConfig {
    /// Checks the thread count, the URL schemes and the cache templates.
    pub fn validate(&self) -> Result<(), String> {
        if self.threads == 0 {
            return Err("threads must be >= 1".into());
        }
        for (name, url) in [
            ("texture_url", &self.texture_url),
            ("heightmap_url", &self.heightmap_url),
        ] {
            if !url.contains("://") {
                return Err(format!("{name} has no scheme: {url}"));
            }
        }
        for (name, template) in [
            ("texture_cache", &self.texture_cache),
            ("heightmap_cache", &self.heightmap_cache),
        ] {
            if template.is_empty() {
                return Err(format!("{name} is empty"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::lod::Ring;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn geo_anchors_match_reference_values() {
        let cases = [
            (
                "dolomites",
                46.206889,
                9.497194,
                269,
                181,
                54_168.30,
                27_469.85,
                39_307.58,
            ),
            (
                "grand canyon",
                35.97391,
                -113.76892,
                94,
                201,
                63_343.93,
                12_371.94,
                6_394.49,
            ),
            (
                "negev",
                30.82691969172123,
                34.91386235622426,
                305,
                209,
                67_213.26,
                44_042.89,
                58_796.15,
            ),
            (
                "london", 51.5074, 0.1278, 256, 170, 48_717.25, 8_854.85, 12_328.76,
            ),
        ];
        for (name, lat, lon, ax, az, ts, ox, oz) in cases {
            let g = geo_anchor(lat, lon);
            assert_eq!((g.anchor_x, g.anchor_z), (ax, az), "{name} anchor");
            assert!(
                close(g.tile_size, ts, 0.01),
                "{name} tile_size {}",
                g.tile_size
            );
            assert!(
                close(f64::from(g.origin_offset.x), ox, 0.1),
                "{name} offset x"
            );
            assert!(
                close(f64::from(g.origin_offset.z), oz, 0.1),
                "{name} offset z"
            );
            assert_eq!(g.origin_offset.y, 0.0);
        }
    }

    #[test]
    fn another_spheres_metres_scale_the_tile() {
        let earth = geo_anchor(-27.455, 153.102);
        let gps = geo_anchor_on(-27.455, 153.102, std::f64::consts::TAU * 6_371_000.0);
        assert_eq!(
            (earth.anchor_x, earth.anchor_z),
            (gps.anchor_x, gps.anchor_z)
        );
        let ratio = std::f64::consts::TAU * 6_371_000.0 / EQUATOR_CIRCUMFERENCE_M;
        assert!(close(gps.tile_size, earth.tile_size * ratio, 1e-6));
    }

    #[test]
    fn from_lat_lon_keeps_other_defaults() {
        let w = WorldConfig::from_lat_lon(46.206889, 9.497194);
        assert_eq!(w.base_zoom, MIN_ZOOM);
        assert_eq!(w.max_zoom, 15);
        assert_eq!(w.elevation_zoom, 15);
        assert_eq!(
            w.initial_position(5000.0),
            w.origin_offset + Vec3::Y * 5000.0
        );
        assert!(w.validate().is_ok());
    }

    #[test]
    fn tile_and_absolute_coordinates_invert_each_other() {
        let w = WorldConfig::from_lat_lon(-27.455, 153.102);
        let (ox, oz) = (f64::from(w.origin_offset.x), f64::from(w.origin_offset.z));
        let (fx, fy) = w.absolute_to_tile(19, ox, oz);
        let (x, z) = w.tile_to_absolute(19, fx, fy);
        assert!(close(x, ox, 1e-6) && close(z, oz, 1e-6));
        let key = w.key_at(19, ox, oz);
        assert_eq!(w.provider_tile(key), (fx.floor() as i32, fy.floor() as i32));
        assert_eq!(w.elevation_zoom_for(19), 15);
        assert_eq!(w.elevation_zoom_for(12), 12);
    }

    #[test]
    fn validation_rejects_bad_configs() {
        let world = |f: fn(&mut WorldConfig)| {
            let mut w = WorldConfig::default();
            f(&mut w);
            w.validate()
        };
        assert!(world(|w| w.max_zoom = 8).is_err());
        assert!(world(|w| w.max_zoom = 23).is_err());
        assert!(world(|w| w.tile_size = 0.0).is_err());
        assert!(world(|w| w.elevation_zoom = 23).is_err());
        assert!(world(|w| w.min_segments = 0).is_err());
        let net = |f: fn(&mut NetworkConfig)| {
            let mut n = NetworkConfig::default();
            f(&mut n);
            n.validate()
        };
        assert!(net(|n| n.texture_url = "no-scheme".into()).is_err());
        assert!(net(|n| n.threads = 0).is_err());
        assert!(net(|n| n.heightmap_cache = String::new()).is_err());
        let streaming = |policy: LodPolicy| {
            StreamingConfig {
                policy,
                ..Default::default()
            }
            .validate(&WorldConfig::default())
        };
        let ring = |zoom, radius| Ring {
            zoom,
            radius,
            drop: 0.0,
        };
        assert!(streaming(LodPolicy::Rings(vec![ring(19, 3)])).is_ok());
        assert!(streaming(LodPolicy::Rings(vec![ring(8, 3)])).is_err());
        assert!(streaming(LodPolicy::Rings(vec![ring(23, 3)])).is_err());
        assert!(streaming(LodPolicy::Rings(vec![ring(19, -1)])).is_err());
    }

    #[test]
    fn zoom_size_halves_per_level() {
        let w = WorldConfig::default();
        assert_eq!(w.zoom_size(9), 66_400.0);
        assert_eq!(w.zoom_size(10), 33_200.0);
        assert_eq!(w.zoom_size(15), 66_400.0 / 64.0);
    }
}
