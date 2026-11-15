//! `TerrainStreamer`: the Node3D that owns the store, drives the per-frame loop, draws each
//! resident tile as a mesh under its own transform and exposes the engine to GDScript / C#.

use super::configs::{
    key_from, TerrainNetworkConfig, TerrainRenderingConfig, TerrainStreamingConfig,
    TerrainWorldConfig,
};
use super::draw;
use crate::core::config::RenderingConfig;
use crate::core::frustum::{Frustum, Plane};
use crate::core::lod::TileKey;
use crate::core::source::TilePayload;
use crate::core::store::TileStore;
use godot::classes::geometry_instance_3d::ShadowCastingSetting;
use godot::classes::node::InternalMode;
use godot::classes::{Camera3D, INode3D, MeshInstance3D, Node3D};
use godot::prelude::*;
use std::collections::HashMap;
use std::time::Instant;

/// A drawn tile: its node and what culling and rebasing need of it.
struct DrawnTile {
    node: Gd<MeshInstance3D>,
    abs_x: f64,
    abs_z: f64,
    drop: f32,
    half: f32,
    low: f32,
    high: f32,
}

impl DrawnTile {
    /// The node's position: the tile's centre in user space (`abs + world_offset`, added in f64
    /// before the single f32 cast), `drop` meters down.
    fn position(&self, world_offset: Vector3) -> Vector3 {
        Vector3::new(
            (self.abs_x + f64::from(world_offset.x)) as f32,
            world_offset.y - self.drop,
            (self.abs_z + f64::from(world_offset.z)) as f32,
        )
    }
}

/// Streams real-world terrain tiles around a focus node and draws them as meshes under its own
/// transform, lit and fogged like the rest of the scene. Add one per world.
#[derive(GodotClass)]
#[class(base = Node3D)]
pub struct TerrainStreamer {
    /// World topology and height function. Assigning a new one rebuilds the world.
    #[export]
    #[var(set = set_world)]
    world: Option<Gd<TerrainWorldConfig>>,
    /// Streaming policy (quadtree or rings, budgets). Assigning a new one rebuilds the world.
    #[export]
    #[var(set = set_streaming)]
    streaming: Option<Gd<TerrainStreamingConfig>>,
    /// How tiles are drawn; read as each tile is drawn.
    #[export]
    rendering: Option<Gd<TerrainRenderingConfig>>,
    /// Download / cache parameters. Assigning a new one rebuilds the world.
    #[export]
    #[var(set = set_network)]
    network: Option<Gd<TerrainNetworkConfig>>,
    /// The Node3D streaming follows (a camera, a vehicle). Empty: the viewport's camera.
    #[export]
    focus_path: NodePath,
    /// Large-world rebase input: `absolute = user - world_offset`, user space being this node's
    /// own. Shift it together with your camera and every user-space node.
    #[export]
    world_offset: Vector3,
    /// Tiles the app draws itself, as `Vector3i(zoom, x, z)`: never streamed, and never drawn
    /// over by a coarser tile.
    #[export]
    #[var(set = set_pinned_tiles)]
    pinned_tiles: Array<Vector3i>,
    /// Pauses streaming and hides the terrain when false.
    #[export]
    #[var(set = set_enabled)]
    enabled: bool,

    /// True until the first desired set is fully resident (splash screens).
    #[var(no_set)]
    is_loading: bool,
    /// Fraction of the desired set that is resident, in [0, 1].
    #[var(no_set)]
    loading_progress: f32,
    /// Resident tiles.
    #[var(no_set)]
    resident_count: i32,
    /// Tiles the policy wants.
    #[var(no_set)]
    desired_count: i32,
    /// Tiles being fetched.
    #[var(no_set)]
    loading_count: i32,

    store: Option<TileStore>,
    tiles_root: Option<Gd<Node3D>>,
    drawn: HashMap<TileKey, DrawnTile>,
    baked_offset: Vector3,
    casting_shadows: bool,
    initialized: bool,
    load_complete_emitted: bool,
    base: Base<Node3D>,
}

#[godot_api]
impl INode3D for TerrainStreamer {
    fn init(base: Base<Node3D>) -> Self {
        Self {
            world: Some(TerrainWorldConfig::new_gd()),
            streaming: Some(TerrainStreamingConfig::new_gd()),
            rendering: Some(TerrainRenderingConfig::new_gd()),
            network: Some(TerrainNetworkConfig::new_gd()),
            focus_path: NodePath::default(),
            world_offset: Vector3::ZERO,
            pinned_tiles: Array::new(),
            enabled: true,
            is_loading: true,
            loading_progress: 0.0,
            resident_count: 0,
            desired_count: 0,
            loading_count: 0,
            store: None,
            tiles_root: None,
            drawn: HashMap::new(),
            baked_offset: Vector3::ZERO,
            casting_shadows: false,
            initialized: false,
            load_complete_emitted: false,
            base,
        }
    }

    fn ready(&mut self) {
        self.base_mut().set_process_priority(100);
        self.initialized = true;
        self.rebuild();
    }

    fn process(&mut self, _delta: f64) {
        if !self.enabled {
            return;
        }
        if self.store.is_none() {
            if self.initialized {
                self.rebuild();
            }
            if self.store.is_none() {
                return;
            }
        }
        self.frame();
    }

    fn exit_tree(&mut self) {
        self.teardown();
    }
}

#[godot_api]
impl TerrainStreamer {
    /// Emitted once, when the initial desired set became fully resident.
    #[signal]
    fn initial_load_complete();
    /// A tile became resident.
    #[signal]
    fn tile_promoted(zoom: i32, x: i32, z: i32);
    /// A resident tile was evicted.
    #[signal]
    fn tile_evicted(zoom: i32, x: i32, z: i32);
    /// A tile download / decode failed (it is retried at the next desired-set rebuild).
    #[signal]
    fn tile_failed(zoom: i32, x: i32, z: i32, reason: GString);

    /// A sensible initial camera position over the world anchor, `altitude` meters up.
    #[func]
    pub fn initial_position(&self, altitude: f32) -> Vector3 {
        match &self.world {
            Some(w) => w.bind().initial_position(altitude),
            None => Vector3::new(0.0, altitude, 0.0),
        }
    }

    /// Terrain altitude (meters) under a point in this node's space, on the drawn triangles of
    /// the finest resident tile there, or `null` when none covers it.
    #[func]
    pub fn ground_height(&self, position: Vector3) -> Variant {
        let abs = position - self.world_offset;
        match self
            .store
            .as_ref()
            .and_then(|s| s.ground_height(glam::Vec3::new(abs.x, abs.y, abs.z)))
        {
            Some(h) => (h + self.world_offset.y).to_variant(),
            None => Variant::nil(),
        }
    }

    /// Debug: one dictionary per resident tile — `zoom`, `x`, `z`, `center` (this node's
    /// space), `size`, `drop`, `desired`, `visible`.
    #[func]
    pub fn get_resident_tiles(&self) -> Array<VarDictionary> {
        let mut out = Array::new();
        let Some(store) = &self.store else {
            return out;
        };
        for (key, info) in store.resident_iter() {
            let mut d = VarDictionary::new();
            d.set("zoom", i32::from(key.zoom));
            d.set("x", key.x);
            d.set("z", key.z);
            d.set(
                "center",
                Vector3::new(
                    (info.abs_x + f64::from(self.world_offset.x)) as f32,
                    self.world_offset.y - info.drop,
                    (info.abs_z + f64::from(self.world_offset.z)) as f32,
                ),
            );
            d.set("size", info.size);
            d.set("drop", info.drop);
            d.set("desired", info.desired);
            d.set("visible", info.visible);
            out.push(&d);
        }
        out
    }

    /// Debug: whether a point in this node's space is inside the viewport camera's frustum by
    /// the engine's own culling test.
    #[func]
    pub fn debug_point_in_frustum(&self, position: Vector3) -> bool {
        match self.local_frustum() {
            Some(f) => f.contains_point(glam::Vec3::new(position.x, position.y, position.z)),
            None => false,
        }
    }

    /// Drop every tile and recreate the store and workers from the current configs.
    #[func]
    pub fn rebuild(&mut self) {
        self.teardown();
        if !self.initialized {
            return;
        }
        let world = self.world_or_default().bind().to_core();
        let streaming = self.streaming_or_default().bind().to_core();
        let network = self.network_or_default().bind().to_core();
        let mut store = match TileStore::new(world, streaming, &network) {
            Ok(s) => s,
            Err(e) => {
                godot_error!("godotiles: invalid configuration: {e}");
                self.enabled = false;
                return;
            }
        };
        store.set_pinned(self.pinned_keys());
        let mut root = Node3D::new_alloc();
        root.set_name("Tiles");
        root.set_visible(self.enabled);
        self.base_mut()
            .add_child_ex(&root)
            .internal(InternalMode::FRONT)
            .done();
        self.tiles_root = Some(root);
        self.baked_offset = self.world_offset;
        self.casting_shadows = self.rendering_or_default().bind().cast_shadows;
        self.store = Some(store);
        self.is_loading = true;
        self.loading_progress = 0.0;
        self.resident_count = 0;
        self.desired_count = 0;
        self.loading_count = 0;
        self.load_complete_emitted = false;
    }

    #[func]
    fn set_world(&mut self, world: Option<Gd<TerrainWorldConfig>>) {
        self.world = world;
        self.rebuild_if_live();
    }

    #[func]
    fn set_streaming(&mut self, streaming: Option<Gd<TerrainStreamingConfig>>) {
        self.streaming = streaming;
        self.rebuild_if_live();
    }

    #[func]
    fn set_network(&mut self, network: Option<Gd<TerrainNetworkConfig>>) {
        self.network = network;
        self.rebuild_if_live();
    }

    #[func]
    fn set_pinned_tiles(&mut self, tiles: Array<Vector3i>) {
        self.pinned_tiles = tiles;
        let keys = self.pinned_keys();
        if let Some(store) = &mut self.store {
            store.set_pinned(keys);
        }
    }

    #[func]
    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if let Some(root) = &mut self.tiles_root {
            root.set_visible(enabled);
        }
    }
}

impl TerrainStreamer {
    fn rebuild_if_live(&mut self) {
        if self.initialized && self.base().is_inside_tree() {
            self.rebuild();
        }
    }

    fn teardown(&mut self) {
        self.drawn.clear();
        if let Some(mut root) = self.tiles_root.take() {
            if root.is_instance_valid() {
                root.queue_free();
            }
        }
        self.store = None;
    }

    fn pinned_keys(&self) -> std::collections::HashSet<TileKey> {
        self.pinned_tiles.iter_shared().map(key_from).collect()
    }

    fn world_or_default(&mut self) -> Gd<TerrainWorldConfig> {
        self.world
            .get_or_insert_with(TerrainWorldConfig::new_gd)
            .clone()
    }

    fn streaming_or_default(&mut self) -> Gd<TerrainStreamingConfig> {
        self.streaming
            .get_or_insert_with(TerrainStreamingConfig::new_gd)
            .clone()
    }

    fn rendering_or_default(&mut self) -> Gd<TerrainRenderingConfig> {
        self.rendering
            .get_or_insert_with(TerrainRenderingConfig::new_gd)
            .clone()
    }

    fn network_or_default(&mut self) -> Gd<TerrainNetworkConfig> {
        self.network
            .get_or_insert_with(TerrainNetworkConfig::new_gd)
            .clone()
    }

    fn camera(&self) -> Option<Gd<Camera3D>> {
        self.base().get_viewport().and_then(|v| v.get_camera_3d())
    }

    fn focus(&self) -> Option<Gd<Node3D>> {
        if !self.focus_path.is_empty() {
            if let Some(node) = self.base().get_node_or_null(&self.focus_path) {
                if let Ok(focus) = node.try_cast::<Node3D>() {
                    return Some(focus);
                }
            }
        }
        self.camera().map(|c| c.upcast())
    }

    /// The viewport camera's frustum in this node's space.
    fn local_frustum(&self) -> Option<Frustum> {
        let planes = self.camera()?.get_frustum();
        if planes.len() != 6 {
            return None;
        }
        let to_global = self.base().get_global_transform();
        let mut out = [Plane {
            normal: glam::Vec3::Y,
            d: 0.0,
        }; 6];
        for (slot, p) in out.iter_mut().zip(planes.iter_shared()) {
            let normal = to_global.basis.transposed() * p.normal;
            *slot = Plane {
                normal: glam::Vec3::new(normal.x, normal.y, normal.z),
                d: p.d - p.normal.dot(to_global.origin),
            };
        }
        Some(Frustum { planes: out })
    }

    fn free_tile(&mut self, key: &TileKey) {
        if let Some(mut tile) = self.drawn.remove(key) {
            if tile.node.is_instance_valid() {
                tile.node.queue_free();
            }
        }
    }

    fn rebake(&mut self) {
        let offset = self.world_offset;
        self.baked_offset = offset;
        for tile in self.drawn.values_mut() {
            let position = tile.position(offset);
            tile.node.set_position(position);
        }
    }

    /// The per-frame loop: reconcile → promote → update desired → rebase → cull → status.
    fn frame(&mut self) {
        let Some(focus) = self.focus() else {
            return;
        };
        let local =
            self.base().get_global_transform().affine_inverse() * focus.get_global_position();
        let abs = local - self.world_offset;
        let abs_focus = glam::Vec3::new(abs.x, abs.y, abs.z);

        let mut evicted = self
            .store
            .as_mut()
            .expect("store exists while processing")
            .reconcile(abs_focus);
        for key in &evicted {
            self.free_tile(key);
        }

        let rendering = self.rendering_or_default().bind().to_core();
        let world_offset = self.world_offset;
        let mut promoted = Vec::new();
        if let (Some(store), Some(root)) = (self.store.as_mut(), self.tiles_root.as_mut()) {
            store.drain();
            let budget = store.streaming().upload_budget;
            let cap = store.streaming().max_uploads_per_frame;
            let start = Instant::now();
            while promoted.len() < cap && start.elapsed() < budget {
                let Some(payload) = store.next_promotion() else {
                    break;
                };
                let key = payload.key;
                let drop = store.streaming().policy.drop(key.zoom);
                match draw_tile(root, &payload, &rendering, drop, world_offset) {
                    Some(tile) => {
                        if let Some(mut old) = self.drawn.insert(key, tile).map(|t| t.node) {
                            old.queue_free();
                        }
                        store.mark_resident(key, payload.geometry);
                        promoted.push(key);
                    }
                    None => {
                        godot_warn!("godotiles: failed to create the texture of tile {key:?}");
                    }
                }
            }
            store.maybe_update_desired(abs_focus);
        }

        if self.world_offset != self.baked_offset {
            self.rebake();
        }

        let casting = self.rendering_or_default().bind().cast_shadows;
        if casting != self.casting_shadows {
            self.casting_shadows = casting;
            for tile in self.drawn.values_mut() {
                tile.node.set_cast_shadows_setting(shadow_setting(casting));
            }
        }

        let frustum = self.local_frustum();
        let offset = self.world_offset;
        let visibility: Vec<(TileKey, bool)> = self
            .drawn
            .iter()
            .map(|(key, tile)| {
                let visible = frustum.as_ref().is_none_or(|f| {
                    let c = tile.position(offset);
                    f.intersects_box(
                        glam::Vec3::new(c.x - tile.half, c.y + tile.low, c.z - tile.half),
                        glam::Vec3::new(c.x + tile.half, c.y + tile.high, c.z + tile.half),
                    )
                });
                (*key, visible)
            })
            .collect();
        let store = self.store.as_mut().expect("store exists while processing");
        for (key, visible) in visibility {
            store.set_visible(key, visible);
        }

        let status = store.status();
        let failures = store.take_failures();
        self.is_loading = status.loading;
        self.loading_progress = status.progress;
        self.resident_count = status.resident as i32;
        self.desired_count = status.desired as i32;
        self.loading_count = status.loading_count as i32;

        for key in promoted {
            self.signals()
                .tile_promoted()
                .emit(i32::from(key.zoom), key.x, key.z);
        }
        for key in evicted.drain(..) {
            self.signals()
                .tile_evicted()
                .emit(i32::from(key.zoom), key.x, key.z);
        }
        for (key, reason) in failures {
            godot_warn!(
                "godotiles: tile {}/{}/{} failed: {reason}",
                key.zoom,
                key.x,
                key.z
            );
            self.signals().tile_failed().emit(
                i32::from(key.zoom),
                key.x,
                key.z,
                &GString::from(&reason),
            );
        }
        if !status.loading && !self.load_complete_emitted {
            self.load_complete_emitted = true;
            self.signals().initial_load_complete().emit();
        }
    }
}

fn shadow_setting(cast: bool) -> ShadowCastingSetting {
    if cast {
        ShadowCastingSetting::ON
    } else {
        ShadowCastingSetting::OFF
    }
}

/// A payload drawn under `root`: its skirted mesh under its imagery, at its place in user
/// space. `None` when the imagery makes no texture.
fn draw_tile(
    root: &mut Gd<Node3D>,
    payload: &TilePayload,
    rendering: &RenderingConfig,
    drop: f32,
    world_offset: Vector3,
) -> Option<DrawnTile> {
    let texture = draw::imagery_texture(&payload.albedo)?;
    let geometry = &payload.geometry;
    let skirt =
        rendering.skirt_cells.max(0.0) * (geometry.size / f64::from(geometry.segments)) as f32;
    let (low, high) = geometry.height_range();
    let (abs_x, abs_z) = geometry.centre();
    let mut node = MeshInstance3D::new_alloc();
    node.set_mesh(&draw::tile_mesh(geometry, rendering.skirt_cells));
    node.set_material_override(&draw::imagery_material(&texture));
    node.set_cast_shadows_setting(shadow_setting(rendering.cast_shadows));
    let tile = DrawnTile {
        node: node.clone(),
        abs_x,
        abs_z,
        drop,
        half: (geometry.size * 0.5) as f32,
        low: low - skirt,
        high,
    };
    node.set_position(tile.position(world_offset));
    root.add_child(&node);
    Some(tile)
}
