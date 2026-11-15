# Godotiles design (this fork)

The Godot shell over an engine-agnostic core, ported from our bevytiles fork's CPU meshing
(`bevy-v0.2.0-collision`, `af12527`). Upstream's GPU displacement, normal-map tiles and
heightmap synthesis are gone.

## Coordinate spaces

One convention: **`absolute = user − world_offset`**. Absolute space has its origin at the anchor
tile's north-west corner at `base_zoom`, x east, y up, z south (slippy-map `y`), meters, f64 for
tile math. User space is the `TerrainStreamer` node's own space: each tile is a `MeshInstance3D`
under it at `abs + world_offset` (added in f64, cast once), so the node's transform places the
terrain in the scene. A tile key is `TileKey { zoom, x, z }`, anchor-relative; GDScript sees
`Vector3i(zoom, x, z)`. Provider tiles (`x, y`) are absolute slippy-map coordinates.

## Geometry (`core/geometry.rs`)

Every tile's heights come from the Terrarium heightmaps of one zoom,
`WorldConfig::elevation_zoom_for(zoom) = min(zoom, elevation_zoom)`: bilinear between texel
centres and read across heightmap edges, a missing neighbour clamped by a rule of the point
alone. That is one continuous height function per zoom, so neighbouring tiles of any zoom
sample the same points on a shared edge and meet exactly. Normals come from the function's
slope (a central difference half a texel wide), not from each mesh, so lighting has no seam.
A tile has `CELLS_PER_TEXEL` (4) cells per elevation texel it spans, clamped to
`[min_segments, max_segments]`. `TileGeometry` gives local and absolute (f64) vertices, UVs,
indices and `height_at(u, v)` on the drawn triangles; `skirted_mesh` hangs a skirt
`skirt_cells` cells deep from the edges to hide cracks between zoom levels.

## LOD policies (`core/lod.rs`)

- `Quadtree`: raytiles' disc of base tiles subdivided by distance thresholds and the horizon;
  snapshots pin it to the C++ and Bevy engines.
- `Rings(Vec<Ring { zoom, radius, drop }>)`: a square of tiles per zoom around the focus's tile,
  overlapping, each coarser ring drawn `drop` meters lower so the finer one wins.
  `rings_within` lists every tile a focus anywhere in a square needs, for preloads.

Pinned tiles (the app's own) are never requested; the quadtree subdivides through their
ancestors so nothing coarser covers them.

## Source (`core/source/`)

`Fetcher` is blocking: cache first (paths from `texture_cache` / `heightmap_cache` templates
under `cache_dir`), HTTP via ureq 3 otherwise, written through atomically, unless `offline`. It
decodes imagery and heightmaps (memoised) and builds geometry. `TileSource` runs worker threads
around one fetcher: dedup by key, cancel flags checked between assets, every request answered
by one payload (imagery + geometry) or one drop.

## Frame loop (`godot/streamer.rs`)

reconcile (evict undesired tiles: under rings all of them; under the quadtree those off-screen,
beyond the horizon, at base zoom or covered) → drain and promote within the upload budget
(mesh + mipmapped anisotropic imagery on a `StandardMaterial3D`) → movement-gated desired-set
rebuild (forced when the pinned set changes) → rebake on a `world_offset` change → cull (the
viewport camera's frustum taken into the node's space against each tile's box) → status and
signals. The loading flag flips only once a desired set exists and is fully serviced.

## Blocking surface (`godot/tiles.rs`)

`TerrainTiles` exposes the fetcher and geometry to GDScript for tiles the app builds itself
(colliders, sensor meshes, pinned tiles drawn as the streamer draws them) and for preloading.
