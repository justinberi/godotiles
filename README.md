<div align="center">
    <img src="icon.png" alt="logo" width="150"/>
    <br />
    <h1>Godotiles</h1>
    <strong>3D geospatial engine for Godot 4</strong>
    <br />
    <br />
</div>

[![Linux Build](https://github.com/ziv/godotiles/actions/workflows/linux.yml/badge.svg)](https://github.com/ziv/godotiles/actions/workflows/linux.yml)
[![macOS Build](https://github.com/ziv/godotiles/actions/workflows/macos.yml/badge.svg)](https://github.com/ziv/godotiles/actions/workflows/macos.yml)
[![Windows Build](https://github.com/ziv/godotiles/actions/workflows/windows.yml/badge.svg)](https://github.com/ziv/godotiles/actions/workflows/windows.yml)

**Godotiles** is a 3D geospatial engine 🌎 for [Godot 4](https://godotengine.org/). Designed to
stream and render the real world in real time, it lets you visualize any location on Earth
directly inside your Godot games and applications.

Built for indie developers and professionals alike, Godotiles is a perfect fit for UAV
simulations, flight-planning software, lightweight GIS analysis, presentations, digital sand
tables, and any other geospatial visualization.

It provides precise, ground-truth altitude data, essential for accurate collision detection and
spawning mechanics in games, as well as for topographical analysis in GIS workflows.

Godotiles is the Godot port of [raytiles](https://github.com/ziv/raytiles) (raylib) and
[bevytiles](https://github.com/ziv/bevytiles) (Bevy): a Rust GDExtension with the same tile
cache format, LOD policy and defaults.

> **This fork** (branch `godot-v0.1.3-collision`) carries the CPU meshing of our bevytiles fork
> (`bevy-v0.2.0-collision`, commit `af12527`): every tile is meshed on the CPU from one height
> function per elevation zoom, so neighbours share their edge vertices and a Godot app can
> draw, collide with and sense the same terrain the Bevy simulator does. It replaces upstream's
> GPU displacement shader, normal-map tiles and heightmap synthesis above the native zoom.

## Features

- Streaming and rendering **any** location on Earth
- Adaptive quadtree **LOD**, zoom 9–22, or fixed **rings** around a ground vehicle
- Tiles **meshed on the CPU** with skirts, drawn with `StandardMaterial3D`: the scene's
  **lights, shadows and fog** apply
- Ground-truth **altitude queries** on the drawn triangles, and the same vertices for
  **colliders and sensors** (`TerrainTiles`)
- Configurable and **provider agnostic** (Esri / Mapbox imagery, Mapzen Terrarium elevation)
- Background downloading on worker threads with a persistent, resumable **on-disk cache**
  (any layout, so another program's cache can be shared; offline mode reads it alone)
- Large-world shifting for float-precision-safe worlds of any size
- Prebuilt binaries for **macOS, Linux and Windows**; usable from GDScript and C#
- Open source, MIT / Apache-2.0

## Screenshots

| cruise over the Grand Canyon | 400 m AGL | debug overlay (K / L) |
|---|---|---|
| ![cruise](screenshots/grand-canyon-cruise.jpg) | ![low](screenshots/grand-canyon-low.jpg) | ![overlay](screenshots/debug-overlay.jpg) |

## Run the demo

Requires Godot 4.4+ (developed on 4.7) and a Rust toolchain.

```sh
scripts/build.sh release        # builds the extension into addons/godotiles/bin/<platform>/
godot --path .                  # or open the project in the editor and press Play (addons/godotiles/demo/main.tscn)
```

Flies over the Grand Canyon. **A/D** roll, **Q/E** yaw, **W/S** pitch, **+/-** throttle, **R**
reset after crashing into the terrain, **K** zoom labels, **L** tile bounds. Tiles cache under
`.cache/`; the first run downloads them.

## Use in your project

Copy `addons/godotiles/` (with the `bin/` folder from a
[release](https://github.com/ziv/godotiles/releases) or from `scripts/build.sh`) into your
project. No plugin activation is needed — the classes are registered by the GDExtension. The addon folder also carries the
demo (`addons/godotiles/demo/`) — delete it if you do not want it in your project.

```gdscript
extends Node3D

func _ready() -> void:
    var terrain := TerrainStreamer.new()
    terrain.world = TerrainWorldConfig.from_lat_lon(46.206889, 9.497194)  # the Dolomites
    terrain.focus_path = $Camera3D.get_path()                              # what streaming follows
    add_child(terrain)
    add_child(DirectionalLight3D.new())                                    # light it as any mesh

    $Camera3D.near = 1.0
    $Camera3D.far = 400000.0     # see to the horizon (the inspector caps at 4000; code does not)
    $Camera3D.position = terrain.initial_position(5000.0)
    $Camera3D.look_at($Camera3D.position + Vector3(-1000, -300, -1000), Vector3.UP)
```

Configuration is four resources on the node — `world`, `streaming`, `rendering`, `network`.
`world.elevation_zoom` (15) is the zoom of the heightmaps every tile's heights come from; finer
tiles (up to `world.max_zoom`) sample it. `streaming.rings` (an array of `TerrainRing`: zoom,
radius, drop) replaces the quadtree when not empty. The terrain lives under the node's own
transform and follows `focus_path` (any Node3D; the viewport camera when empty);
`pinned_tiles` (`Vector3i(zoom, x, z)` keys) are tiles the app draws itself, never streamed or
covered. Queries: `ground_height(pos)` (a float, or `null` when no tile covers the point),
`is_loading` / `loading_progress`, `get_resident_tiles()`. Large worlds use the raytiles rebase
convention via `world_offset` (`absolute = user − world_offset`); see
`addons/godotiles/demo/demo.gd`.

`TerrainTiles` is the blocking side, for an app that builds tiles itself and preloads a cache:

```gdscript
var tiles := TerrainTiles.create(terrain.world, terrain.network)
tiles.world_offset = terrain.world_offset
var rings := [TerrainRing.create(19, 3, 0.0), TerrainRing.create(17, 3, 0.4)]
var keys := tiles.rings_within(rings, Vector3.ZERO, 300.0, [])          # Vector3i(zoom, x, z)
for key in keys:
    var p := tiles.provider_tile(key)
    tiles.fetch(TerrainTiles.KIND_TEXTURE, key.x, p.x, p.y)              # "" or the reason
for h in tiles.heightmaps_for(keys):                                     # Vector3i(zoom, x, y)
    tiles.fetch(TerrainTiles.KIND_HEIGHTMAP, h.x, h.y, h.z)

var g := tiles.geometry(tiles.key_at(19, Vector3.ZERO))  # vertices / normals / uvs / indices about g.origin
var faces := PackedVector3Array()
for i in g.indices:
    faces.append(g.vertices[i])
var shape := ConcavePolygonShape3D.new()                                 # a collider of the drawn triangles
shape.set_faces(faces)
```

See [`plan.md`](plan.md) for the design: coordinate spaces, the frame loop, the geometry, the
LOD policies and the tile lifecycle.

## Layout

| path | role |
|---|---|
| `rust/src/core/` | engine-agnostic core (no Godot types): config, LOD policy, CPU geometry, heights, frustum, source (blocking fetcher + worker pool + disk cache), tile lifecycle store |
| `rust/src/godot/` | the Godot shell: `TerrainStreamer`, `TerrainTiles`, config resources, mesh / material conversion |
| `addons/godotiles/` | the distributable addon |
| `addons/godotiles/demo/` | the demo scenes and scripts (ship with the addon) |
| `tests/godot/` | headless smoke test (`godot --headless --path . -s tests/godot/smoke.gd`) |
| `scripts/` | build script, cache pre-warmer (`node scripts/tiles-cache.mjs <x> <y>`) |

## Notes

- Data: imagery © Esri; elevation from the Mapzen / AWS terrain tiles. Mind their
  terms. Set `network.texture_url` to a Mapbox template with your token for Mapbox imagery.
- `cargo test --manifest-path rust/Cargo.toml` runs the full suite (LOD snapshots identical to
  the C++ and Bevy engines, rings and pinned tiles, geometry seams, store policy, source and
  cache integration against seeded caches — all offline). The headless smoke test needs the
  project imported once (`godot --headless --path . --import`).
- macOS: an unsigned download may need `xattr -dr com.apple.quarantine addons/godotiles/bin/macos`.

## License

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or https://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or https://opensource.org/licenses/MIT)
