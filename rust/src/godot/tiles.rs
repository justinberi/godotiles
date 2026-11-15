//! `TerrainTiles`: the blocking, GDScript-facing side of the engine, for an app that builds some
//! tiles itself (colliders, sensor meshes, pinned tiles drawn as the streamer draws the rest)
//! and preloads a cache. Every call reads the cache first and, unless the network config is
//! offline, falls back to HTTP on the caller's thread.

use super::configs::{
    key_from, key_to, keys_from, rings_from, zoom_of, TerrainNetworkConfig, TerrainRenderingConfig,
    TerrainWorldConfig,
};
use super::draw;
use crate::core::config::{RenderingConfig, WorldConfig};
use crate::core::geometry::{self, Elevation, TileGeometry};
use crate::core::lod::{self, LodOptions, LodPolicy, TileKey};
use crate::core::source::{Fetcher, Kind};
use godot::classes::{ArrayMesh, ImageTexture, StandardMaterial3D};
use godot::prelude::*;
use std::collections::BTreeSet;

/// Builds tiles, fetches and checks the cache, and answers geometry questions for one world,
/// blocking. Tile keys are `Vector3i(zoom, x, z)`, anchor-relative; `x, y` arguments are
/// provider tile coordinates. Positions are in user space: `absolute = user - world_offset`.
#[derive(GodotClass)]
#[class(init, base = RefCounted)]
pub struct TerrainTiles {
    /// The world the tiles belong to.
    #[var]
    #[init(val = Some(TerrainWorldConfig::new_gd()))]
    world: Option<Gd<TerrainWorldConfig>>,
    /// Where tiles come from and are cached.
    #[var]
    #[init(val = Some(TerrainNetworkConfig::new_gd()))]
    network: Option<Gd<TerrainNetworkConfig>>,
    /// How `tile_mesh` and `tile_material` draw a tile.
    #[var]
    #[init(val = Some(TerrainRenderingConfig::new_gd()))]
    rendering: Option<Gd<TerrainRenderingConfig>>,
    /// The user-space offset of the world origin, as the streamer's.
    #[var]
    world_offset: Vector3,
    fetcher: Option<Fetcher>,
}

#[godot_api]
impl TerrainTiles {
    /// The imagery asset, for `fetch`, `has_cached` and `cache_path`.
    #[constant]
    const KIND_TEXTURE: i32 = 0;
    /// The Terrarium heightmap asset.
    #[constant]
    const KIND_HEIGHTMAP: i32 = 1;

    /// Tiles of a world from a network config.
    #[func]
    fn create(
        world: Option<Gd<TerrainWorldConfig>>,
        network: Option<Gd<TerrainNetworkConfig>>,
    ) -> Gd<Self> {
        let mut tiles = Self::new_gd();
        {
            let mut t = tiles.bind_mut();
            t.world = world;
            t.network = network;
        }
        tiles
    }

    /// Fetches an asset into the cache unless it is there: "" when it is had, else the reason.
    #[func]
    fn fetch(&mut self, kind: i32, zoom: i32, x: i32, y: i32) -> GString {
        let result = match asset(kind) {
            Some(kind) => self.fetcher().bytes(kind, zoom_of(zoom), x, y).map(|_| ()),
            None => Err(format!("no asset kind {kind}")),
        };
        GString::from(result.err().unwrap_or_default().as_str())
    }

    /// Whether the cache holds an asset.
    #[func]
    fn has_cached(&mut self, kind: i32, zoom: i32, x: i32, y: i32) -> bool {
        asset(kind).is_some_and(|kind| self.fetcher().has_cached(kind, zoom_of(zoom), x, y))
    }

    /// Where an asset is kept in the cache.
    #[func]
    fn cache_path(&mut self, kind: i32, zoom: i32, x: i32, y: i32) -> GString {
        match asset(kind) {
            Some(kind) => GString::from(
                self.fetcher()
                    .cache_path(kind, zoom_of(zoom), x, y)
                    .to_string_lossy()
                    .as_ref(),
            ),
            None => GString::new(),
        }
    }

    /// A tile's terrain, the vertices the streamer draws: `vertices`, `normals` (about the tile's
    /// centre), `uvs`, `indices` (wound as Godot's front faces), `origin` (the centre in user
    /// space), `absolute_x` / `absolute_z` (the centre in absolute space, f64), `size`,
    /// `segments`, and `error` ("" when its elevation was had; else the reason, and the tile is
    /// at sea level).
    #[func]
    fn geometry(&mut self, key: Vector3i) -> VarDictionary {
        let (geometry, error) = self.build(key_from(key));
        let (abs_x, abs_z) = geometry.centre();
        let mut d = VarDictionary::new();
        d.set("vertices", &draw::vectors(&geometry.local_positions()));
        d.set("normals", &draw::vectors(&geometry.normals));
        d.set("uvs", &draw::uvs(&geometry.uvs()));
        d.set("indices", &draw::godot_indices(&geometry.indices()));
        d.set("origin", self.origin_of(abs_x, abs_z));
        d.set("absolute_x", abs_x);
        d.set("absolute_z", abs_z);
        d.set("size", geometry.size);
        d.set("segments", geometry.segments as i32);
        d.set("error", &GString::from(error.as_str()));
        d
    }

    /// A tile's mesh about its centre with its skirts (`rendering.skirt_cells`), as the streamer
    /// draws it; at sea level, with a warning, when its elevation can't be had.
    #[func]
    fn tile_mesh(&mut self, key: Vector3i) -> Gd<ArrayMesh> {
        let (geometry, error) = self.build(key_from(key));
        if !error.is_empty() {
            godot_warn!("godotiles: tile {key} at sea level: {error}");
        }
        draw::tile_mesh(&geometry, self.rendering_config().skirt_cells)
    }

    /// A tile's material as the streamer draws it: its imagery, or `rendering.no_imagery_color`
    /// when that can't be had.
    #[func]
    fn tile_material(&mut self, key: Vector3i) -> Gd<StandardMaterial3D> {
        let key = key_from(key);
        let (x, y) = self.world_config().provider_tile(key);
        match self.imagery(i32::from(key.zoom), x, y) {
            Some(texture) => draw::imagery_material(&texture),
            None => {
                let c = self.rendering_config().no_imagery_color;
                draw::plain_material(Color::from_rgba(c[0], c[1], c[2], c[3]))
            }
        }
    }

    /// A tile's centre in user space: where `tile_mesh` goes.
    #[func]
    fn tile_origin(&self, key: Vector3i) -> Vector3 {
        let key = key_from(key);
        let world = self.world_config();
        let size = world.zoom_size(key.zoom);
        self.origin_of(
            (f64::from(key.x) + 0.5) * size,
            (f64::from(key.z) + 0.5) * size,
        )
    }

    /// A provider tile's imagery, mipmapped, or null when it can't be had.
    #[func]
    fn imagery(&mut self, zoom: i32, x: i32, y: i32) -> Option<Gd<ImageTexture>> {
        let albedo = self.fetcher().texture(zoom_of(zoom), x, y).ok()?;
        draw::imagery_texture(&albedo)
    }

    /// Every tile `rings` (an array of `TerrainRing`) want for a focus anywhere in the square of
    /// `radius` meters about `centre`, leaving out `pinned` keys: what a preload must fetch for a
    /// vehicle that stays within it.
    #[func]
    fn rings_within(
        &self,
        rings: VarArray,
        centre: Vector3,
        radius: f32,
        pinned: VarArray,
    ) -> Array<Vector3i> {
        let world = self.world_config();
        let rings = rings_from(&rings);
        let opts = LodOptions {
            base_zoom: world.base_zoom,
            max_zoom: world.max_zoom,
            base_tile_size: world.tile_size,
            radius: 0,
            thresholds: Default::default(),
            policy: LodPolicy::Rings(rings.clone()),
        };
        let abs = centre - self.world_offset;
        lod::rings_within(
            &opts,
            &rings,
            glam::Vec3::new(abs.x, abs.y, abs.z),
            radius,
            &keys_from(&pinned),
        )
        .into_iter()
        .map(key_to)
        .collect()
    }

    /// The heightmaps the geometry of `keys` (an array of `Vector3i` tile keys) reads, each
    /// once, as `Vector3i(zoom, x, y)` provider tiles: what a preload must fetch besides the
    /// keys' imagery.
    #[func]
    fn heightmaps_for(&self, keys: VarArray) -> Array<Vector3i> {
        let world = self.world_config();
        let set: BTreeSet<(u8, i32, i32)> = keys_from(&keys)
            .into_iter()
            .flat_map(|key| {
                let zoom = world.elevation_zoom_for(key.zoom);
                geometry::heightmaps_for(&world, key)
                    .into_iter()
                    .map(move |(x, y)| (zoom, x, y))
            })
            .collect();
        set.into_iter()
            .map(|(zoom, x, y)| Vector3i::new(i32::from(zoom), x, y))
            .collect()
    }

    /// The terrain's height function at a user-space point as tiles of `zoom` sample it
    /// (meters, user space), or null when its elevation can't be had.
    #[func]
    fn height_at(&mut self, position: Vector3, zoom: i32) -> Variant {
        let world = self.world_config();
        let (x, z) = self.absolute_of(position);
        match self.fetcher().height_at(&world, zoom_of(zoom), x, z) {
            Ok(h) => ((h + f64::from(self.world_offset.y)) as f32).to_variant(),
            Err(_) => Variant::nil(),
        }
    }

    /// The key of the tile at `zoom` holding a user-space point.
    #[func]
    fn key_at(&self, zoom: i32, position: Vector3) -> Vector3i {
        let (x, z) = self.absolute_of(position);
        key_to(self.world_config().key_at(zoom_of(zoom), x, z))
    }

    /// The provider tile (x, y) of a key.
    #[func]
    fn provider_tile(&self, key: Vector3i) -> Vector2i {
        let (x, y) = self.world_config().provider_tile(key_from(key));
        Vector2i::new(x, y)
    }

    /// The user-space position of an absolute point (meters, f64), cast to f32 once.
    #[func]
    fn world_position(&self, absolute_x: f64, height: f64, absolute_z: f64) -> Vector3 {
        Vector3::new(
            (absolute_x + f64::from(self.world_offset.x)) as f32,
            (height + f64::from(self.world_offset.y)) as f32,
            (absolute_z + f64::from(self.world_offset.z)) as f32,
        )
    }
}

impl TerrainTiles {
    fn world_config(&self) -> WorldConfig {
        self.world
            .as_ref()
            .map(|w| w.bind().to_core())
            .unwrap_or_default()
    }

    fn rendering_config(&self) -> RenderingConfig {
        self.rendering
            .as_ref()
            .map(|r| r.bind().to_core())
            .unwrap_or_default()
    }

    /// The fetcher for the current network config, remade when the config changed.
    fn fetcher(&mut self) -> &Fetcher {
        let cfg = self
            .network
            .as_ref()
            .map(|n| n.bind().to_core())
            .unwrap_or_default();
        if self.fetcher.as_ref().is_none_or(|f| *f.config() != cfg) {
            self.fetcher = Some(Fetcher::new(&cfg));
        }
        self.fetcher.as_ref().expect("made above")
    }

    /// A tile's geometry from the cache (or the network), else at sea level with the reason.
    fn build(&mut self, key: TileKey) -> (TileGeometry, String) {
        let world = self.world_config();
        match self.fetcher().geometry(&world, key) {
            Ok(geometry) => (geometry, String::new()),
            Err(error) => {
                let none = Elevation {
                    zoom: world.elevation_zoom_for(key.zoom),
                    ..Default::default()
                };
                (TileGeometry::build(&world, key, &none), error)
            }
        }
    }

    fn origin_of(&self, abs_x: f64, abs_z: f64) -> Vector3 {
        self.world_position(abs_x, 0.0, abs_z)
    }

    fn absolute_of(&self, position: Vector3) -> (f64, f64) {
        (
            f64::from(position.x) - f64::from(self.world_offset.x),
            f64::from(position.z) - f64::from(self.world_offset.z),
        )
    }
}

fn asset(kind: i32) -> Option<Kind> {
    match kind {
        0 => Some(Kind::Texture),
        1 => Some(Kind::Heightmap),
        _ => None,
    }
}
