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
