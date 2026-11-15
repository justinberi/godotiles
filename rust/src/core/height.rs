//! CPU-side heights for `ground_height` queries: every resident tile's [`TileGeometry`],
//! sampled on its drawn triangles, so the answer is the ground the camera sees (built on the
//! worker threads along with the mesh).

use super::config::{WorldConfig, MAX_ZOOM};
use super::geometry::TileGeometry;
use super::lod::TileKey;
use glam::Vec3;
use std::collections::HashMap;

/// All resident tiles' geometry, keyed like the resident set. Kept in lockstep with residency
/// by the store.
#[derive(Default, Debug)]
pub struct HeightGrids(pub HashMap<TileKey, TileGeometry>);

/// Terrain altitude under `abs` (absolute space): walks zooms finest→coarsest and samples the
/// first resident tile holding the XZ point, on its triangles (a ring's drop is not included).
pub fn ground_height(grids: &HeightGrids, world: &WorldConfig, abs: Vec3) -> Option<f32> {
    for zoom in (world.base_zoom..=MAX_ZOOM).rev() {
        let key = world.key_at(zoom, f64::from(abs.x), f64::from(abs.z));
        if let Some(geometry) = grids.0.get(&key) {
            let size = geometry.size;
            let u = ((f64::from(abs.x) - f64::from(key.x) * size) / size) as f32;
            let v = ((f64::from(abs.z) - f64::from(key.z) * size) / size) as f32;
            return Some(geometry.height_at(u, v));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ground_height_samples_the_finest_resident_tile() {
        let world = WorldConfig::default();
        let flat = |key: TileKey, h: f32| TileGeometry {
            key,
            size: world.zoom_size(key.zoom),
            segments: 1,
            heights: vec![h; 4],
            normals: vec![[0.0, 1.0, 0.0]; 4],
        };
        let mut grids = HeightGrids::default();
        let coarse = TileKey {
            zoom: 9,
            x: 0,
            z: 0,
        };
        let fine = TileKey {
            zoom: 10,
            x: 1,
            z: 0,
        };
        let finest = TileKey {
            zoom: 19,
            x: 0,
            z: 0,
        };
        grids.0.insert(coarse, flat(coarse, 100.0));
        grids.0.insert(fine, flat(fine, 300.0));
        grids.0.insert(finest, flat(finest, 700.0));
        let at = |x: f32, z: f32| ground_height(&grids, &world, Vec3::new(x, 0.0, z));
        assert_eq!(at(10.0, 10.0), Some(700.0));
        assert_eq!(at(1000.0, 10.0), Some(100.0));
        assert_eq!(at(40_000.0, 10.0), Some(300.0));
        assert_eq!(at(-10.0, 10.0), None);
    }

    #[test]
    fn ground_height_is_on_the_drawn_triangles() {
        let world = WorldConfig::default();
        let key = TileKey {
            zoom: 9,
            x: 0,
            z: 0,
        };
        let mut grids = HeightGrids::default();
        grids.0.insert(
            key,
            TileGeometry {
                key,
                size: world.zoom_size(9),
                segments: 1,
                heights: vec![0.0, 10.0, 20.0, 30.0],
                normals: vec![[0.0, 1.0, 0.0]; 4],
            },
        );
        let size = world.zoom_size(9) as f32;
        let h = ground_height(&grids, &world, Vec3::new(0.25 * size, 0.0, 0.25 * size));
        assert!((h.unwrap() - 7.5).abs() < 1e-3);
    }
}
