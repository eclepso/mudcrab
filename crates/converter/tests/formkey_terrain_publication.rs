//! This production assertion uses only APIs that also exist on the step 2 baseline.
#[path = "common/native_terrain.rs"]
mod fixture;

#[tokio::test]
async fn both_readers_publish_native_neighbor_keys_after_broken_land() {
    for reader in [
        converter::config::RecordReader::Legacy,
        converter::config::RecordReader::Inhouse,
    ] {
        let publication = fixture::publish(reader, true, false).await;
        assert!(publication.output.join("cell_cache.rkyv").is_file());
        let database =
            rusqlite::Connection::open(publication.output.join("skyrim_world.db")).unwrap();
        let neighbors: i64 = database
            .query_row(
                "SELECT count(*) FROM records WHERE form_id IN (50333697,33556496)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            neighbors, 2,
            "Zulu-owned 000801 and Patch-owned 000810 must keep allocated owner/local identities"
        );
        let damaged: i64 = database
            .query_row(
                "SELECT count(*) FROM records WHERE form_id IN (33556482,33556483)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            damaged, 0,
            "bounded malformed LAND records must stay local failures"
        );
        let height: Vec<u8> = database
            .query_row(
                "SELECT heightmap FROM land WHERE cell_id=50333441",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(f32::from_le_bytes(height[..4].try_into().unwrap()), 2.5);
        assert!(
            publication
                .report
                .warnings
                .iter()
                .any(|line| line.to_ascii_lowercase().contains("patch.esp")),
            "{:?}",
            publication.report.warnings
        );
    }
}

/// Normal replacement must retain the same selected geometry without declaring data loss.
async fn assert_clean_land_override_is_advisory(reader: converter::config::RecordReader) {
    let publication = fixture::publish_clean_competing_lands(reader).await;
    let database = rusqlite::Connection::open(publication.output.join("skyrim_world.db")).unwrap();
    let selected: Vec<u8> = database
        .query_row(
            "SELECT heightmap FROM land WHERE cell_id=50333440",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(f32::from_le_bytes(selected[..4].try_into().unwrap()), 3.5);
    let neighbor: Vec<u8> = database
        .query_row(
            "SELECT heightmap FROM land WHERE cell_id=50333441",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(f32::from_le_bytes(neighbor[..4].try_into().unwrap()), 2.5);
    let cache = converter::esm::cell_cache::read_form_key_cell_cache(
        &publication.output.join("cell_cache.rkyv"),
    )
    .unwrap();
    assert_eq!(cache.cells.len(), 2);
    let selected = cache
        .cells
        .iter()
        .find(|cell| cell.cell_id == 0x0300_0700)
        .unwrap();
    assert_eq!(selected.heights, vec![28.0; 33 * 33]);
    assert_eq!(selected.normals, [1, 2, 126].repeat(33 * 33));
    let neighbor = cache
        .cells
        .iter()
        .find(|cell| cell.cell_id == 0x0300_0701)
        .unwrap();
    assert_eq!(neighbor.heights, vec![20.0; 33 * 33]);
    let expected = "warning: CELL 000700:Zulu.esm: LAND 000800:Zulu.esm from source priority 2 wins; dropped LAND 000804:Patch.esp from source priority 2";
    assert_eq!(
        publication
            .report
            .terrain_warnings
            .iter()
            .filter(|message| message.as_str() == expected)
            .count(),
        1,
        "the ordinary terrain advisory must remain visible after the geometry checks"
    );
    assert!(
        publication
            .report
            .warnings
            .iter()
            .all(|message| message != expected),
        "ordinary LAND override must not become a data-loss warning"
    );
    assert_eq!(
        publication
            .report
            .notices
            .iter()
            .filter(|message| message.as_str() == expected)
            .count(),
        1,
        "the advisory also survives cache-only and warm publication"
    );
    assert!(
        publication.report.complete,
        "{:?}",
        publication.report.warnings
    );
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(publication.output.join("conversion-manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["complete"], true);
    assert_eq!(manifest["record_data_complete"], true);
}

/// The handwritten reader preserves complete publication for a valid LAND override.
#[tokio::test]
async fn legacy_land_overrides_are_advisories_and_keep_publication_complete() {
    assert_clean_land_override_is_advisory(converter::config::RecordReader::Legacy).await;
}

/// The typed reader preserves complete publication for the same valid LAND override.
#[tokio::test]
async fn inhouse_land_overrides_are_advisories_and_keep_publication_complete() {
    assert_clean_land_override_is_advisory(converter::config::RecordReader::Inhouse).await;
}
