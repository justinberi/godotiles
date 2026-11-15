//! # godotiles
//!
//! Geo-spatial terrain streaming for Godot 4 — a GDExtension port of the
//! [raytiles](https://github.com/ziv/raytiles) / [bevytiles](https://github.com/ziv/bevytiles)
//! engine. Streams satellite imagery and Terrarium heightmaps around a moving focus and draws
//! them as terrain meshed on the CPU, so physics and sensors can share its vertices.
//!
//! - [`core`] — engine-agnostic: config, LOD policy, geometry, heights, source, store.
//! - [`godot`] — the Godot shell: `TerrainStreamer`, `TerrainTiles`, the config resources.

pub mod core;
pub mod godot;

use ::godot::prelude::*;

struct GodotilesExtension;

#[gdextension]
unsafe impl ExtensionLibrary for GodotilesExtension {}
