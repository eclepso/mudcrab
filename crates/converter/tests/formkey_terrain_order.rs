//! Native terrain ordering witnesses with supplier order distinct from pack allocation.
use converter::terrain_chunks::{
    PlacedCellMesh, read_form_key_terrain_chunk, write_form_key_terrain_chunks,
};
use shared::{
    form_key::FormKey,
    pack_identity::PackIdentity,
    plugin_table::{PluginFacts, PluginTable},
    terrain_mesh::{CELL_MESH_VERTICES, TerrainCellMesh, terrain_chunk_path},
};

/// Allocate alphabetically while retaining the independently authored supplying order.
fn identity(names: &[&str]) -> PackIdentity {
    let table = PluginTable::fresh(
        names
            .iter()
            .enumerate()
            .map(|(priority, name)| {
                (
                    (*name).to_string(),
                    PluginFacts {
                        content_hash: Some([priority as u8 + 1; 32]),
                        ..Default::default()
                    },
                )
            })
            .collect(),
    )
    .unwrap();
    let order = names.iter().map(|name| table.id(name).unwrap()).collect();
    PackIdentity::new(table, order).unwrap()
}

/// Name one native record without constructing a legacy runtime handle.
fn key(identity: &PackIdentity, owner: &str, local: u32) -> FormKey {
    FormKey::new(identity.table().id(owner).unwrap(), local).unwrap()
}

/// A literal flat CPU mesh uses only the default texture and no source decoding helpers.
fn placed(world: FormKey, cell: FormKey, grid_x: i32) -> PlacedCellMesh {
    PlacedCellMesh {
        worldspace_id: world.to_packed(),
        grid_x,
        grid_y: 0,
        mesh: TerrainCellMesh {
            cell_id: cell.to_packed(),
            key: [0x55; 32],
            heights: vec![0.0; CELL_MESH_VERTICES],
            normals: (0..CELL_MESH_VERTICES).flat_map(|_| [0, 0, 127]).collect(),
            vertex_colors: Vec::new(),
            slot_stride: 8,
            quadrant_slots: vec![0; 32],
            quadrant_slot_counts: [1; 4],
            weights: vec![0; 8 * CELL_MESH_VERTICES],
        },
    }
}

/// Decode names solely to make expected original owners independent of allocated numbers.
fn name(identity: &PackIdentity, packed: u64) -> String {
    identity
        .table()
        .named(FormKey::from_packed(packed).unwrap())
        .unwrap()
        .to_string()
}

/// An ESL suffix is sufficient for light ordering even when the source light flag is absent.
#[test]
fn chunk_cells_put_full_owners_before_extension_only_light_owners() {
    let identity = identity(&["Early.esl", "Late.esm"]);
    let early_owner = identity.table().id("Early.esl").unwrap();
    assert!(
        !identity
            .table()
            .entry(early_owner)
            .unwrap()
            .facts
            .flags
            .light
    );
    assert!(identity.table().is_light(early_owner));
    let world = key(&identity, "Late.esm", 0x60);
    let early = key(&identity, "Early.esl", 0x11);
    let late = key(&identity, "Late.esm", 0x11);
    let output = tempfile::tempdir().unwrap();
    let report = write_form_key_terrain_chunks(
        output.path(),
        &identity,
        vec![placed(world, early, 0), placed(world, late, 1)],
    )
    .unwrap();
    assert_eq!(report.cells, 2);
    assert_eq!(report.worldspaces.len(), 1);
    let path = output
        .path()
        .join(terrain_chunk_path(world.to_packed(), 0, 0));
    let chunk = read_form_key_terrain_chunk(&path, &identity).unwrap();
    assert_eq!(chunk.worldspace_id, world.to_packed());
    let actual = chunk
        .cells
        .iter()
        .map(|cell| name(&identity, cell.cell_id))
        .collect::<Vec<_>>();
    assert_eq!(
        actual,
        ["000011:Late.esm", "000011:Early.esl"],
        "terrain chunk cells must use full-before-extension-light owner order"
    );
}

/// Report worldspace order follows original owners rather than alphabetic pack allocation.
#[test]
fn report_worldspaces_keep_original_full_owner_order() {
    let identity = identity(&["Zulu.esm", "Alpha.esm"]);
    let zulu_world = key(&identity, "Zulu.esm", 0x60);
    let alpha_world = key(&identity, "Alpha.esm", 0x60);
    let zulu_cell = key(&identity, "Zulu.esm", 0x11);
    let alpha_cell = key(&identity, "Alpha.esm", 0x11);
    assert!(alpha_world.to_packed() < zulu_world.to_packed());
    let output = tempfile::tempdir().unwrap();
    let report = write_form_key_terrain_chunks(
        output.path(),
        &identity,
        vec![
            placed(zulu_world, zulu_cell, 0),
            placed(alpha_world, alpha_cell, 0),
        ],
    )
    .unwrap();
    assert_eq!(report.cells, 2);
    for (world, cell) in [(zulu_world, zulu_cell), (alpha_world, alpha_cell)] {
        let path = output
            .path()
            .join(terrain_chunk_path(world.to_packed(), 0, 0));
        let chunk = read_form_key_terrain_chunk(&path, &identity).unwrap();
        assert_eq!(chunk.worldspace_id, world.to_packed());
        assert_eq!(chunk.cells.len(), 1);
        assert_eq!(chunk.cells[0].cell_id, cell.to_packed());
    }
    let actual = report
        .worldspaces
        .iter()
        .map(|world| name(&identity, world.worldspace_id))
        .collect::<Vec<_>>();
    assert_eq!(
        actual,
        ["000060:Zulu.esm", "000060:Alpha.esm"],
        "terrain report worldspaces must follow original full-owner order"
    );
}
