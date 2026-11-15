//! Rebuild the current pack's movement projection through authenticated native source context.
use color_eyre::{Result, eyre::WrapErr};
#[cfg(test)]
use converter::esm::{exporter::create_tables, types::ArchivedRecordData};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

fn main() -> Result<()> {
    color_eyre::install()?;
    let path = PathBuf::from(std::env::args_os().nth(1).ok_or_else(|| {
        color_eyre::eyre::eyre!("usage: movement-profile-annotate <skyrim_world.db>")
    })?);
    annotate(&path)?;
    println!(
        "movement profile projected from winning records in {}",
        path.display()
    );
    Ok(())
}

/// Authenticate the new generation before opening the existing file for projection writes.
fn annotate(path: &Path) -> Result<()> {
    let root = path
        .parent()
        .filter(|root| !root.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let _lock = shared::asset_lock::AssetLock::acquire_exclusive(root)
        .wrap_err("close asset readers before rebuilding the movement profile")?;
    shared::world_assets::validate_world_database(path)?;
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .wrap_err_with(|| format!("opening {}", path.display()))?;
    converter::esm::movement::rebuild_movement_projection(&connection)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use shared::records::ArchivedSubRecord;

    fn record_blob(subrecords: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
        rkyv::to_bytes::<rkyv::rancor::Error>(&ArchivedRecordData {
            subrecords: subrecords
                .iter()
                .map(|(tag, data)| ArchivedSubRecord {
                    tag: *tag,
                    data: data.clone(),
                })
                .collect(),
        })
        .unwrap()
        .to_vec()
    }

    fn insert_record(
        connection: &Connection,
        form_id: u32,
        record_type: &str,
        fields: &[([u8; 4], Vec<u8>)],
    ) {
        let identity = shared::pack_identity::PackIdentity::read_sql(connection).unwrap();
        let key = identity.runtime_key(form_id).unwrap().unwrap();
        let owner = identity.order()[0];
        connection
            .execute(
                "INSERT INTO records(form_id,record_type,load_order,data) VALUES(?1,?2,0,?3)",
                params![key.to_sql_i64(), record_type, record_blob(fields)],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO record_context VALUES(?1,?2,?3,NULL,NULL,?4)",
                params![key.to_sql_i64(), owner.get(), form_id, fields.len()],
            )
            .unwrap();
        for (ordinal, (tag, bytes)) in fields.iter().enumerate() {
            connection
                .execute(
                    "INSERT INTO record_field_origins VALUES(?1,?2,?2,?3,1)",
                    params![key.to_sql_i64(), ordinal, tag.as_slice()],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO record_source_fields VALUES(?1,?2,?3,?4)",
                    params![key.to_sql_i64(), ordinal, tag.as_slice(), bytes],
                )
                .unwrap();
            if matches!(tag, b"WKMV" | b"RNMV") {
                let raw = u32::from_le_bytes(bytes.as_slice().try_into().unwrap());
                let target = shared::records::source_key(&identity, owner, raw).unwrap();
                connection
                    .execute(
                        "INSERT INTO resolved_links VALUES(?1,?2,0,?3,?4,?5)",
                        params![
                            key.to_sql_i64(),
                            ordinal,
                            raw,
                            target.map(shared::form_key::FormKey::to_sql_i64),
                            if raw == 0 { "null" } else { "resolved" }
                        ],
                    )
                    .unwrap();
            }
        }
    }

    /// A package holding the movement records the pipeline writes (one `MOVT`, two `GMST`s)
    /// plus the given races, each linked to the same movement type.
    fn package_with_races(races: &[(u32, u32)]) -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let pack = directory.path().join("pack");
        std::fs::create_dir(&pack).unwrap();
        let path = pack.join("skyrim_world.db");
        let connection = Connection::open(&path).unwrap();
        create_tables(&connection).unwrap();
        connection
            .execute(
                "UPDATE schema_info SET version=?1",
                params![shared::WORLD_DATABASE_SCHEMA_VERSION],
            )
            .unwrap();
        let table = shared::plugin_table::PluginTable::fresh(vec![(
            "Skyrim.esm".into(),
            shared::plugin_table::PluginFacts {
                content_hash: Some([0x64; 32]),
                ..Default::default()
            },
        )])
        .unwrap();
        let owner = table.id("Skyrim.esm").unwrap();
        let identity = shared::pack_identity::PackIdentity::new(table, vec![owner]).unwrap();
        let transaction = connection.unchecked_transaction().unwrap();
        shared::records::create_record_context(&transaction, &identity).unwrap();
        transaction.commit().unwrap();
        for (form_id, walk_movt_id) in races {
            insert_record(
                &connection,
                *form_id,
                "RACE",
                &[
                    (*b"EDID", b"TestRace\0".to_vec()),
                    (*b"WKMV", walk_movt_id.to_le_bytes().to_vec()),
                ],
            );
        }
        insert_record(
            &connection,
            0x0003_580D,
            "MOVT",
            &[
                (*b"EDID", b"NPC_Default_MT\0".to_vec()),
                (
                    *b"SPED",
                    [100.0_f32; 10]
                        .iter()
                        .flat_map(|speed| speed.to_le_bytes())
                        .collect(),
                ),
            ],
        );
        for (form_id, editor_id) in [
            (0x0001_EC72_u32, &b"fMoveCharWalkBase\0"[..]),
            (0x000A_BEF6, &b"fJumpHeightMin\0"[..]),
        ] {
            insert_record(
                &connection,
                form_id,
                "GMST",
                &[
                    (*b"EDID", editor_id.to_vec()),
                    (*b"DATA", 100.0_f32.to_le_bytes().to_vec()),
                ],
            );
        }
        (directory, path)
    }

    #[test]
    fn projects_a_link_for_every_race_in_the_package() {
        // The player race and a modded race the old hand-picked selection did not name.
        let (_directory, path) =
            package_with_races(&[(0x0000_1234, 0x0003_580D), (0x0001_3746, 0x0003_580D)]);
        // A repeated run replaces the projection: the exact rows and counts below would
        // grow if a run kept stale or duplicate rows.
        annotate(&path).unwrap();
        annotate(&path).unwrap();
        let connection = Connection::open(&path).unwrap();
        shared::world_assets::install_runtime_identity_functions(&connection).unwrap();
        let links: Vec<(u32, Option<u32>)> = connection
            .prepare("SELECT runtime_id(id),runtime_id(walk_movt_id) FROM race_movement_links ORDER BY id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            links,
            vec![
                (0x0000_1234, Some(0x0003_580D)),
                (0x0001_3746, Some(0x0003_580D))
            ]
        );
        // The MOVT and GMST projections still come from the package's own records.
        let count = |table: &str| {
            connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get::<_, u32>(0)
                })
                .unwrap()
        };
        assert_eq!(count("movement_types"), 1);
        assert_eq!(count("movement_game_settings"), 2);
    }

    #[test]
    fn old_schema_refuses_before_reading_or_replacing_movement_payloads() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("skyrim_world.db");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TABLE schema_info(version INTEGER); INSERT INTO schema_info VALUES(9); CREATE TABLE sentinel(value TEXT); INSERT INTO sentinel VALUES('unchanged')").unwrap();
        let error = annotate(&path).unwrap_err().to_string();
        assert!(error.contains("convert again"), "{error}");
        assert_eq!(
            connection
                .query_row("SELECT value FROM sentinel", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "unchanged"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM sqlite_schema WHERE name='movement_types'",
                    [],
                    |row| row.get::<_, usize>(0)
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn refuses_a_package_that_does_not_exist_without_creating_one() {
        let (_directory, path) = package_with_races(&[]);
        std::fs::remove_file(&path).unwrap();
        assert!(annotate(&path).is_err());
        assert!(!path.exists(), "the tool created {}", path.display());
    }
}
