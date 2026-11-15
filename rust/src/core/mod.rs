//! Engine-agnostic core: configuration, LOD policy, CPU terrain geometry, height queries,
//! frustum test, the tile source (blocking fetcher + worker pool) and the tile lifecycle policy.
//!
//! **Rule:** nothing in this module tree may import `godot::`.

pub mod config;
pub mod frustum;
pub mod geometry;
pub mod height;
pub mod lod;
pub mod source;
pub mod store;
pub mod terrarium;
