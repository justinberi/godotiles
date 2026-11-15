extends SceneTree
## Headless smoke test for the Godot shell:
##   godot --headless --path . -s tests/godot/smoke.gd
## Seeds a tiny offline cache (one z9 tile: flat imagery and a flat 100 m Terrarium heightmap)
## and checks the streamer (quadtree, pinned tiles, rings, its own transform, a focus node) and
## the blocking TerrainTiles surface against it.

const HEIGHT_M := 100.0
const CELLS := 64


func _initialize() -> void:
	_run()


func _run() -> void:
	await process_frame
	var cache := ProjectSettings.globalize_path("user://smoke-cache")
	_seed_cache(cache)

	var root_node := Node3D.new()
	root.add_child(root_node)
	var camera := Camera3D.new()
	camera.near = 1.0
	camera.far = 400000.0
	root_node.add_child(camera)
	camera.current = true

	var terrain := TerrainStreamer.new()
	terrain.world.base_zoom = 9
	terrain.world.max_zoom = 9
	terrain.streaming.radius = 2
	terrain.network.cache_dir = cache
	terrain.network.threads = 1
	terrain.network.offline = true
	terrain.network.texture_url = "http://127.0.0.1:9/tex/:zoom:/:x:/:y:.png"
	terrain.network.heightmap_url = "http://127.0.0.1:9/hm/:zoom:/:x:/:y:.png"
	terrain.focus_path = camera.get_path()
	root_node.add_child(terrain)

	var start := terrain.initial_position(5000.0)
	camera.position = start
	camera.look_at(start + Vector3(1000, -500, 1000), Vector3.UP)

	var ok := true
	ok = await _wait_loaded(terrain, "initial load") and ok
	ok = _check(terrain.resident_count >= 1, "resident_count >= 1 (got %d)" % terrain.resident_count) and ok
	ok = _check(terrain.desired_count == 1, "desired_count == 1 (got %d)" % terrain.desired_count) and ok
	ok = _check(terrain.loading_progress == 1.0, "loading_progress == 1") and ok
	var h = terrain.ground_height(start)
	ok = _check(h != null and absf(float(h) - HEIGHT_M) < 0.01, "ground_height = %.0f m (got %s)" % [HEIGHT_M, str(h)]) and ok
	ok = _check(terrain.ground_height(Vector3(-1e6, 0, -1e6)) == null, "ground_height outside coverage is null") and ok
	var tiles := terrain.get_resident_tiles()
	ok = _check(tiles.size() == terrain.resident_count, "get_resident_tiles matches resident_count") and ok
	if tiles.size() > 0:
		ok = _check(tiles[0].zoom == 9 and tiles[0].size == terrain.world.tile_size, "resident tile is the z9 anchor tile") and ok
	var drawn := _drawn_tiles(terrain)
	ok = _check(drawn.size() == 1, "one tile mesh drawn under the streamer (got %d)" % drawn.size()) and ok
	if drawn.size() == 1:
		var material := drawn[0].material_override as StandardMaterial3D
		ok = _check(material != null and material.albedo_texture != null and material.albedo_texture.get_image().has_mipmaps(), "drawn with a mipmapped imagery StandardMaterial3D") and ok
		ok = _check(material.texture_filter == BaseMaterial3D.TEXTURE_FILTER_LINEAR_WITH_MIPMAPS_ANISOTROPIC, "imagery filtered anisotropically") and ok
		var skirted := (CELLS + 1) * (CELLS + 1) + 4 * CELLS
		ok = _check(drawn[0].mesh.surface_get_arrays(0)[Mesh.ARRAY_VERTEX].size() == skirted, "the drawn mesh carries its skirt") and ok

	terrain.world_offset = Vector3(1000, 0, 0)
	camera.position += Vector3(1000, 0, 0)
	await process_frame
	var t2 := terrain.get_resident_tiles()
	ok = _check(t2.size() > 0 and absf(t2[0].center.x - (tiles[0].center.x + 1000.0)) < 0.01, "tile centers follow world_offset") and ok
	ok = _check(terrain.ground_height(start + Vector3(1000, 0, 0)) != null, "ground_height honors world_offset") and ok
	drawn = _drawn_tiles(terrain)
	ok = _check(drawn.size() == 1 and absf(drawn[0].position.x - t2[0].center.x) < 0.01, "tile meshes follow world_offset") and ok

	terrain.rebuild()
	ok = _check(terrain.resident_count == 0 and terrain.is_loading, "rebuild resets residency + loading") and ok
	ok = await _wait_loaded(terrain, "reload after rebuild") and ok
	ok = _check(terrain.resident_count >= 1, "resident after rebuild") and ok

	var middle := Vector3(terrain.world.tile_size * 0.5, 0.0, terrain.world.tile_size * 0.5)
	ok = _check_tiles(terrain, start + Vector3(1000, 0, 0) + middle) and ok

	var pinned: Array[Vector3i] = [Vector3i(9, 0, 0)]
	terrain.pinned_tiles = pinned
	await _frames(3)
	ok = _check(terrain.desired_count == 0 and terrain.resident_count == 0, "a pinned tile is neither wanted nor drawn (desired %d, resident %d)" % [terrain.desired_count, terrain.resident_count]) and ok
	terrain.pinned_tiles = [] as Array[Vector3i]
	ok = await _wait_loaded(terrain, "unpinned tile streams again", true) and ok

	var focus := Node3D.new()
	root_node.add_child(focus)
	focus.position = camera.position
	terrain.focus_path = focus.get_path()
	terrain.position = Vector3(0, 50, 0)
	terrain.streaming.rings = [TerrainRing.create(9, 0, 2.0)] as Array[TerrainRing]
	terrain.streaming = terrain.streaming
	ok = await _wait_loaded(terrain, "rings policy load") and ok
	var ringed := terrain.get_resident_tiles()
	ok = _check(ringed.size() == 1 and absf(ringed[0].center.y + 2.0) < 1e-3 and ringed[0].drop == 2.0, "the ring's tile is drawn its drop down") and ok
	drawn = _drawn_tiles(terrain)
	ok = _check(drawn.size() == 1 and absf(drawn[0].global_position.y - 48.0) < 1e-3, "tiles live under the streamer's transform") and ok

	terrain.queue_free()
	await process_frame
	await process_frame

	print("smoke: %s" % ("PASS" if ok else "FAIL"))
	quit(0 if ok else 1)


func _check_tiles(terrain: TerrainStreamer, position: Vector3) -> bool:
	var ok := true
	var tiles := TerrainTiles.create(terrain.world, terrain.network)
	tiles.world_offset = terrain.world_offset
	var key := Vector3i(9, 0, 0)
	ok = _check(tiles.has_cached(TerrainTiles.KIND_HEIGHTMAP, 9, 306, 207) and tiles.has_cached(TerrainTiles.KIND_TEXTURE, 9, 306, 207), "tiles: has_cached finds the seeded assets") and ok
	ok = _check(not tiles.has_cached(TerrainTiles.KIND_TEXTURE, 9, 1, 1), "tiles: has_cached misses the others") and ok
	ok = _check(tiles.fetch(TerrainTiles.KIND_HEIGHTMAP, 9, 306, 207) == "", "tiles: fetch of a cached asset succeeds") and ok
	ok = _check("offline" in tiles.fetch(TerrainTiles.KIND_TEXTURE, 9, 1, 1), "tiles: fetch offline of a missing asset says why") and ok
	ok = _check(tiles.cache_path(TerrainTiles.KIND_TEXTURE, 9, 306, 207).ends_with("texture/9/306/207.png"), "tiles: cache_path in the default layout") and ok
	ok = _check(tiles.key_at(9, position) == key and tiles.provider_tile(key) == Vector2i(306, 207), "tiles: key_at and provider_tile") and ok

	var g := tiles.geometry(key)
	var vertices: PackedVector3Array = g.vertices
	var indices: PackedInt32Array = g.indices
	ok = _check(g.error == "" and vertices.size() == (CELLS + 1) * (CELLS + 1) and g.normals.size() == vertices.size() and g.uvs.size() == vertices.size(), "tiles: geometry has a vertex grid with normals and uvs") and ok
	ok = _check(indices.size() == CELLS * CELLS * 6, "tiles: geometry has two triangles a cell") and ok
	var flat := true
	for v in vertices:
		flat = flat and absf(v.y - HEIGHT_M) < 0.01
	ok = _check(flat, "tiles: every vertex at the seeded height") and ok
	var a := vertices[indices[0]]
	var b := vertices[indices[1]]
	var c := vertices[indices[2]]
	ok = _check((b - a).cross(c - a).y < 0.0, "tiles: triangles wound as Godot's front faces from above") and ok
	var origin: Vector3 = g.origin
	ok = _check(origin.is_equal_approx(tiles.tile_origin(key)) and absf(origin.x - (terrain.world.tile_size * 0.5 + 1000.0)) < 0.01, "tiles: origin is the tile centre in user space") and ok
	ok = _check(absf(float(g.absolute_x) - terrain.world.tile_size * 0.5) < 0.01, "tiles: absolute centre in f64") and ok

	var mesh := tiles.tile_mesh(key)
	ok = _check(mesh.get_surface_count() == 1 and mesh.surface_get_arrays(0)[Mesh.ARRAY_VERTEX].size() == (CELLS + 1) * (CELLS + 1) + 4 * CELLS, "tiles: tile_mesh carries skirts") and ok
	var material := tiles.tile_material(key)
	ok = _check(material.albedo_texture != null, "tiles: tile_material has the imagery") and ok
	var bare := tiles.tile_material(Vector3i(9, -200, -200))
	ok = _check(bare.albedo_texture == null and bare.albedo_color.is_equal_approx(tiles.rendering.no_imagery_color), "tiles: tile_material without imagery takes no_imagery_color") and ok
	var texture := tiles.imagery(9, 306, 207)
	ok = _check(texture != null and texture.get_image().has_mipmaps(), "tiles: imagery is mipmapped") and ok
	ok = _check(tiles.imagery(9, 1, 1) == null, "tiles: imagery missing is null") and ok
	var sea := tiles.geometry(Vector3i(9, -200, -200))
	ok = _check(sea.error != "" and absf(sea.vertices[0].y) < 1e-6, "tiles: geometry without elevation is at sea level with the reason") and ok

	var height = tiles.height_at(position, 9)
	ok = _check(height != null and absf(float(height) - HEIGHT_M) < 0.01, "tiles: height_at reads the height function") and ok
	var within := tiles.rings_within([TerrainRing.create(9, 1, 0.0)], position, 10.0, [])
	ok = _check(within.size() == 9 and key in within, "tiles: rings_within (got %d)" % within.size()) and ok
	var left := tiles.rings_within([TerrainRing.create(9, 1, 0.0)], position, 10.0, [key])
	ok = _check(left.size() == 8 and not (key in left), "tiles: rings_within leaves out pinned keys") and ok
	var heightmaps := tiles.heightmaps_for([key])
	ok = _check(Vector3i(9, 306, 207) in heightmaps and heightmaps.size() == 9, "tiles: heightmaps_for (got %d)" % heightmaps.size()) and ok
	ok = _check(tiles.world_position(10.0, 5.0, 20.0).is_equal_approx(Vector3(1010.0, 5.0, 20.0)), "tiles: world_position adds world_offset") and ok
	ok = _check(terrain.world.origin_world_offset() == -terrain.world.origin_offset, "world: origin_world_offset") and ok
	return ok


func _drawn_tiles(terrain: TerrainStreamer) -> Array[MeshInstance3D]:
	var out: Array[MeshInstance3D] = []
	for i in terrain.get_child_count(true):
		var holder := terrain.get_child(i, true)
		if holder.is_queued_for_deletion():
			continue
		for node in holder.get_children():
			if node is MeshInstance3D and not node.is_queued_for_deletion():
				out.append(node)
	return out


func _wait_loaded(terrain: TerrainStreamer, what: String, resident := false) -> bool:
	var frames := 0
	while (terrain.is_loading or (resident and terrain.resident_count == 0)) and frames < 600:
		await process_frame
		frames += 1
	return _check(not terrain.is_loading and (not resident or terrain.resident_count > 0), "%s (%d frames)" % [what, frames])


func _frames(n: int) -> void:
	for i in n:
		await process_frame


func _check(cond: bool, what: String) -> bool:
	print("  [%s] %s" % ["ok" if cond else "FAIL", what])
	return cond


func _seed_cache(cache: String) -> void:
	for d in ["texture/9/306", "heightmap/9/306"]:
		DirAccess.make_dir_recursive_absolute(cache + "/" + d)
	var tex := Image.create(256, 256, false, Image.FORMAT_RGB8)
	tex.fill(Color8(120, 140, 90))
	tex.save_png(cache + "/texture/9/306/207.png")
	var hm := Image.create(256, 256, false, Image.FORMAT_RGB8)
	hm.fill(Color8(128, int(HEIGHT_M), 0))
	hm.save_png(cache + "/heightmap/9/306/207.png")
