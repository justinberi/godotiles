//! The configuration resources exposed to Godot. Each mirrors a plain Rust config struct in
//! `core::config` and converts to it with `to_core()`. A tile key crosses to GDScript as a
//! `Vector3i(zoom, x, z)`, anchor-relative.

use crate::core::config::{
    geo_anchor_on, NetworkConfig, RenderingConfig, StreamingConfig, WorldConfig,
    DEFAULT_HEIGHTMAP_CACHE, DEFAULT_HEIGHTMAP_URL, DEFAULT_TEXTURE_CACHE, DEFAULT_TEXTURE_URL,
    DEFAULT_THRESHOLDS, DEFAULT_USER_AGENT, EQUATOR_CIRCUMFERENCE_M, ZOOM_LEVELS,
};
use crate::core::lod::{LodPolicy, Ring, TileKey};
use godot::classes::{IResource, ProjectSettings, Resource};
use godot::prelude::*;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

fn packed(values: &[f32]) -> PackedFloat32Array {
    PackedFloat32Array::from(values)
}

/// Copy up to 14 slots from a packed array; missing slots fall back to `defaults` (never to 0).
fn per_zoom(values: &PackedFloat32Array, defaults: &[f32; ZOOM_LEVELS]) -> [f32; ZOOM_LEVELS] {
    let mut out = *defaults;
    for (slot, v) in out.iter_mut().zip(values.as_slice()) {
        *slot = *v;
    }
    out
}

/// A zoom from GDScript's int, clamped into `u8`.
pub fn zoom_of(value: i32) -> u8 {
    value.clamp(0, i32::from(u8::MAX)) as u8
}

fn seconds(value: f32) -> Duration {
    Duration::from_secs_f64(f64::from(value.max(0.0)))
}

/// A tile key from its GDScript form, `Vector3i(zoom, x, z)`.
pub fn key_from(v: Vector3i) -> TileKey {
    TileKey {
        zoom: zoom_of(v.x),
        x: v.y,
        z: v.z,
    }
}

/// A tile key in its GDScript form, `Vector3i(zoom, x, z)`.
pub fn key_to(key: TileKey) -> Vector3i {
    Vector3i::new(i32::from(key.zoom), key.x, key.z)
}

/// The keys of a GDScript array of `Vector3i`, skipping anything else.
pub fn keys_from(values: &VarArray) -> HashSet<TileKey> {
    values
        .iter_shared()
        .filter_map(|v| v.try_to::<Vector3i>().ok())
        .map(key_from)
        .collect()
}

/// The rings of a GDScript array of `TerrainRing`, skipping anything else.
pub fn rings_from(values: &VarArray) -> Vec<Ring> {
    values
        .iter_shared()
        .filter_map(|v| v.try_to::<Gd<TerrainRing>>().ok())
        .map(|r| r.bind().to_core())
        .collect()
}

/// World topology and the terrain's height function: anchor tile, zoom range, tile size,
/// elevation zoom, relief and mesh density. Effectively immutable once streaming started
/// (assigning a new one to the streamer rebuilds it).
#[derive(GodotClass)]
#[class(init, base = Resource)]
pub struct TerrainWorldConfig {
    /// Anchor tile X at `base_zoom`: the world origin sits at this tile's corner.
    #[export]
    #[init(val = 306)]
    pub anchor_x: i32,
    /// Anchor tile row (slippy-map `y`) at `base_zoom`.
    #[export]
    #[init(val = 207)]
    pub anchor_z: i32,
    /// Lowest LOD zoom ever loaded.
    #[export(range = (9.0, 22.0))]
    #[init(val = 9)]
    pub base_zoom: i32,
    /// Highest quadtree LOD zoom. Imagery must exist this deep.
    #[export(range = (9.0, 22.0))]
    #[init(val = 15)]
    pub max_zoom: i32,
    /// World size (meters) of one tile at `base_zoom`.
    #[export]
    #[init(val = 66_400.0)]
    pub tile_size: f32,
    /// Offset of the anchor point inside its anchor tile (filled by `from_lat_lon`).
    #[export]
    #[init(val = Vector3::ZERO)]
    pub origin_offset: Vector3,
    /// Zoom of the heightmaps every tile's heights come from; finer tiles sample this one's.
    #[export(range = (9.0, 22.0))]
    #[init(val = 15)]
    pub elevation_zoom: i32,
    /// Terrain relief exaggeration applied to every height.
    #[export]
    #[init(val = 1.0)]
    pub height_scale: f32,
    /// Fewest mesh cells along a tile's side.
    #[export(range = (1.0, 1024.0))]
    #[init(val = 4)]
    pub min_segments: i32,
    /// Most mesh cells along a tile's side.
    #[export(range = (1.0, 1024.0))]
    #[init(val = 64)]
    pub max_segments: i32,
    base: Base<Resource>,
}

#[godot_api]
impl TerrainWorldConfig {
    /// Anchor the world at a geographic coordinate (degrees): derives the anchor tile, tile
    /// size and origin offset so the world origin sits exactly on the coordinate.
    #[func]
    pub fn from_lat_lon(lat: f64, lon: f64) -> Gd<Self> {
        Self::from_lat_lon_on(lat, lon, EQUATOR_CIRCUMFERENCE_M)
    }

    /// `from_lat_lon` with the meters of a sphere of `circumference_m`, to agree with another
    /// system's geodesy (a GPS model's sphere, say) on how far a degree is.
    #[func]
    pub fn from_lat_lon_on(lat: f64, lon: f64, circumference_m: f64) -> Gd<Self> {
        let g = geo_anchor_on(lat, lon, circumference_m);
        let mut res = Self::new_gd();
        {
            let mut r = res.bind_mut();
            r.anchor_x = g.anchor_x;
            r.anchor_z = g.anchor_z;
            r.tile_size = g.tile_size as f32;
            r.origin_offset = Vector3::new(g.origin_offset.x, 0.0, g.origin_offset.z);
        }
        res
    }

    /// A sensible initial camera position over the anchor, `altitude` meters up.
    #[func]
    pub fn initial_position(&self, altitude: f32) -> Vector3 {
        self.origin_offset + Vector3::new(0.0, altitude, 0.0)
    }

    /// The `world_offset` that puts the anchored coordinate at the user-space origin.
    #[func]
    pub fn origin_world_offset(&self) -> Vector3 {
        -self.origin_offset
    }

    /// The plain Rust config.
    pub fn to_core(&self) -> WorldConfig {
        WorldConfig {
            anchor_x: self.anchor_x,
            anchor_z: self.anchor_z,
            base_zoom: zoom_of(self.base_zoom),
            max_zoom: zoom_of(self.max_zoom),
            tile_size: self.tile_size,
            origin_offset: glam::Vec3::new(
                self.origin_offset.x,
                self.origin_offset.y,
                self.origin_offset.z,
            ),
            elevation_zoom: zoom_of(self.elevation_zoom),
            height_scale: self.height_scale,
            min_segments: self.min_segments.max(0) as u32,
            max_segments: self.max_segments.max(0) as u32,
        }
    }
}

/// One ring of the rings policy: the square of tiles within `radius` tiles of the focus's tile
/// at `zoom`, drawn `drop` meters below the ground so that a finer ring over it wins.
#[derive(GodotClass)]
#[class(init, base = Resource)]
pub struct TerrainRing {
    /// Zoom of the ring's tiles.
    #[export(range = (9.0, 22.0))]
    #[init(val = 15)]
    pub zoom: i32,
    /// Tiles each side of the focus's.
    #[export(range = (0.0, 64.0))]
    #[init(val = 2)]
    pub radius: i32,
    /// Meters the ring is drawn below the true ground.
    #[export]
    #[init(val = 0.0)]
    pub drop: f32,
    base: Base<Resource>,
}

#[godot_api]
impl TerrainRing {
    /// A ring of `radius` tiles at `zoom`, `drop` meters down.
    #[func]
    pub fn create(zoom: i32, radius: i32, drop: f32) -> Gd<Self> {
        let mut ring = Self::new_gd();
        {
            let mut r = ring.bind_mut();
            r.zoom = zoom;
            r.radius = radius;
            r.drop = drop;
        }
        ring
    }

    /// The plain Rust ring.
    pub fn to_core(&self) -> Ring {
        Ring {
            zoom: zoom_of(self.zoom),
            radius: self.radius,
            drop: self.drop,
        }
    }
}

/// Which tiles are kept resident and how aggressively the set updates.
#[derive(GodotClass)]
#[class(init, base = Resource)]
pub struct TerrainStreamingConfig {
    /// Radius, in base-zoom tiles, of the disc loaded around the focus (quadtree).
    #[export(range = (1.0, 32.0))]
    #[init(val = 6)]
    pub radius: i32,
    /// Per-zoom subdivision distance thresholds in meters (14 slots, `[zoom - base_zoom]`).
    #[export]
    #[init(val = packed(&DEFAULT_THRESHOLDS))]
    pub thresholds: PackedFloat32Array,
    /// Fixed rings around the focus in place of the quadtree, when not empty.
    #[export]
    pub rings: Array<Gd<TerrainRing>>,
    /// Focus travel (meters) that triggers a desired-set rebuild.
    #[export]
    #[init(val = 500.0)]
    pub update_distance: f32,
    /// Wall-clock budget per frame (milliseconds) for promoting tiles.
    #[export]
    #[init(val = 2.0)]
    pub upload_budget_ms: f32,
    /// Hard cap on tile promotions per frame.
    #[export(range = (1.0, 64.0))]
    #[init(val = 8)]
    pub max_uploads_per_frame: i32,
    base: Base<Resource>,
}

#[godot_api]
impl TerrainStreamingConfig {
    /// The plain Rust config.
    pub fn to_core(&self) -> StreamingConfig {
        let rings: Vec<Ring> = self
            .rings
            .iter_shared()
            .map(|r| r.bind().to_core())
            .collect();
        StreamingConfig {
            radius: self.radius,
            thresholds: per_zoom(&self.thresholds, &DEFAULT_THRESHOLDS),
            policy: if rings.is_empty() {
                LodPolicy::Quadtree
            } else {
                LodPolicy::Rings(rings)
            },
            update_distance: self.update_distance,
            upload_budget: Duration::from_secs_f64(
                f64::from(self.upload_budget_ms.max(0.0)) / 1000.0,
            ),
            max_uploads_per_frame: self.max_uploads_per_frame.max(1) as usize,
        }
    }
}

/// How tiles are drawn. The terrain is a `StandardMaterial3D` per tile, so the scene's lights,
/// shadows and fog apply. Read when a tile is drawn; `cast_shadows` applies to live tiles too.
#[derive(GodotClass)]
#[class(init, base = Resource)]
pub struct TerrainRenderingConfig {
    /// Depth of the skirt hung from every tile's edges, in the tile's mesh cells; hides the
    /// cracks where zoom levels meet. 0 disables.
    #[export]
    #[init(val = 0.5)]
    pub skirt_cells: f32,
    /// Whether tiles cast shadows (they always receive them).
    #[export]
    #[init(val = false)]
    pub cast_shadows: bool,
    /// Colour of a tile drawn without imagery.
    #[export]
    #[init(val = Color::from_rgb(0.35, 0.4, 0.32))]
    pub no_imagery_color: Color,
    base: Base<Resource>,
}

#[godot_api]
impl TerrainRenderingConfig {
    /// The plain Rust config.
    pub fn to_core(&self) -> RenderingConfig {
        let c = self.no_imagery_color;
        RenderingConfig {
            skirt_cells: self.skirt_cells,
            cast_shadows: self.cast_shadows,
            no_imagery_color: [c.r, c.g, c.b, c.a],
        }
    }
}

/// Tile download / cache parameters.
#[derive(GodotClass)]
#[class(init, base = Resource)]
pub struct TerrainNetworkConfig {
    /// Number of background download / geometry worker threads.
    #[export(range = (1.0, 64.0))]
    #[init(val = 4)]
    pub threads: i32,
    /// Root of the on-disk tile cache. Relative paths resolve against the project directory.
    #[export]
    #[init(val = GString::from(".cache"))]
    pub cache_dir: GString,
    /// Imagery URL template with `:zoom:` / `:x:` / `:y:` tokens (Esri uses `zoom/y/x`).
    #[export]
    #[init(val = GString::from(DEFAULT_TEXTURE_URL))]
    pub texture_url: GString,
    /// Terrarium heightmap URL template.
    #[export]
    #[init(val = GString::from(DEFAULT_HEIGHTMAP_URL))]
    pub heightmap_url: GString,
    /// Where an imagery response is kept under `cache_dir`, with the same tokens; another
    /// layout shares another program's cache.
    #[export]
    #[init(val = GString::from(DEFAULT_TEXTURE_CACHE))]
    pub texture_cache: GString,
    /// Where a heightmap response is kept under `cache_dir`.
    #[export]
    #[init(val = GString::from(DEFAULT_HEIGHTMAP_CACHE))]
    pub heightmap_cache: GString,
    /// Read the cache alone, never HTTP: a tile missing from it fails.
    #[export]
    #[init(val = false)]
    pub offline: bool,
    /// The User-Agent header of every request.
    #[export]
    #[init(val = GString::from(DEFAULT_USER_AGENT))]
    pub user_agent: GString,
    /// HTTP connection timeout (seconds).
    #[export]
    #[init(val = 5.0)]
    pub connect_timeout_sec: f32,
    /// HTTP read timeout (seconds).
    #[export]
    #[init(val = 3.0)]
    pub read_timeout_sec: f32,
    base: Base<Resource>,
}

#[godot_api]
impl TerrainNetworkConfig {
    /// The plain Rust config, `cache_dir` made absolute.
    pub fn to_core(&self) -> NetworkConfig {
        let raw = PathBuf::from(self.cache_dir.to_string());
        let cache_dir = if raw.is_absolute() {
            raw
        } else {
            let root = ProjectSettings::singleton()
                .globalize_path("res://")
                .to_string();
            PathBuf::from(root).join(raw)
        };
        NetworkConfig {
            threads: self.threads.max(1) as usize,
            cache_dir,
            texture_url: self.texture_url.to_string(),
            heightmap_url: self.heightmap_url.to_string(),
            texture_cache: self.texture_cache.to_string(),
            heightmap_cache: self.heightmap_cache.to_string(),
            offline: self.offline,
            user_agent: self.user_agent.to_string(),
            connect_timeout: seconds(self.connect_timeout_sec),
            read_timeout: seconds(self.read_timeout_sec),
        }
    }
}

#[godot_api]
impl IResource for TerrainWorldConfig {}
#[godot_api]
impl IResource for TerrainRing {}
#[godot_api]
impl IResource for TerrainStreamingConfig {}
#[godot_api]
impl IResource for TerrainRenderingConfig {}
#[godot_api]
impl IResource for TerrainNetworkConfig {}
