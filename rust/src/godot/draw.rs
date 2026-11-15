//! Turning the core's tile geometry and imagery into Godot resources: an `ArrayMesh` wound as
//! Godot's front faces, a mipmapped `ImageTexture`, and the `StandardMaterial3D` the terrain is
//! drawn with. Main thread only.

use crate::core::geometry::{TileGeometry, TileMesh};
use godot::classes::base_material_3d::{Flags, TextureFilter, TextureParam};
use godot::classes::image::Format;
use godot::classes::mesh::{ArrayType, PrimitiveType};
use godot::classes::{ArrayMesh, Image, ImageTexture, StandardMaterial3D};
use godot::prelude::*;

/// Positions or normals as Godot vectors.
pub fn vectors(values: &[[f32; 3]]) -> PackedVector3Array {
    values
        .iter()
        .map(|v| Vector3::new(v[0], v[1], v[2]))
        .collect()
}

/// Texture coordinates as Godot vectors.
pub fn uvs(values: &[[f32; 2]]) -> PackedVector2Array {
    values.iter().map(|v| Vector2::new(v[0], v[1])).collect()
}

/// Triangle indices wound as Godot's front faces: the core winds counter-clockwise seen from
/// outside, Godot clockwise.
pub fn godot_indices(indices: &[u32]) -> PackedInt32Array {
    indices
        .chunks_exact(3)
        .flat_map(|t| [t[0] as i32, t[2] as i32, t[1] as i32])
        .collect()
}

/// A tile mesh as a one-surface `ArrayMesh`.
pub fn array_mesh(mesh: &TileMesh) -> Gd<ArrayMesh> {
    let mut arrays = VarArray::new();
    arrays.resize(ArrayType::MAX.ord() as usize, &Variant::nil());
    arrays.set(
        ArrayType::VERTEX.ord() as usize,
        &vectors(&mesh.positions).to_variant(),
    );
    arrays.set(
        ArrayType::NORMAL.ord() as usize,
        &vectors(&mesh.normals).to_variant(),
    );
    arrays.set(
        ArrayType::TEX_UV.ord() as usize,
        &uvs(&mesh.uvs).to_variant(),
    );
    arrays.set(
        ArrayType::INDEX.ord() as usize,
        &godot_indices(&mesh.indices).to_variant(),
    );
    let mut out = ArrayMesh::new_gd();
    out.add_surface_from_arrays(PrimitiveType::TRIANGLES, &arrays);
    out
}

/// A tile's skirted mesh about its centre.
pub fn tile_mesh(geometry: &TileGeometry, skirt_cells: f32) -> Gd<ArrayMesh> {
    array_mesh(&geometry.skirted_mesh(skirt_cells))
}

/// The imagery as a texture with its whole mip chain.
pub fn imagery_texture(albedo: &image::RgbaImage) -> Option<Gd<ImageTexture>> {
    let data = PackedByteArray::from(albedo.as_raw().as_slice());
    let mut img = Image::create_from_data(
        albedo.width() as i32,
        albedo.height() as i32,
        false,
        Format::RGBA8,
        &data,
    )?;
    img.generate_mipmaps();
    ImageTexture::create_from_image(&img)
}

/// The terrain's material: fully rough, without specular (the photograph already holds the
/// ground's shading), clamped at the tile's edges, trilinear and anisotropic.
pub fn imagery_material(albedo: &Gd<ImageTexture>) -> Gd<StandardMaterial3D> {
    let mut material = plain_material(Color::WHITE);
    material.set_texture(TextureParam::ALBEDO, albedo);
    material
}

/// The terrain's material in one colour, for a tile without imagery.
pub fn plain_material(color: Color) -> Gd<StandardMaterial3D> {
    let mut material = StandardMaterial3D::new_gd();
    material.set_albedo(color);
    material.set_roughness(1.0);
    material.set_specular(0.0);
    material.set_flag(Flags::USE_TEXTURE_REPEAT, false);
    material.set_texture_filter(TextureFilter::LINEAR_WITH_MIPMAPS_ANISOTROPIC);
    material
}
