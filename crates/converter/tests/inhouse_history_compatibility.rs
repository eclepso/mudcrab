//! Current native history retains chain facts; obsolete unbound snapshots require conversion again.
use converter::records::history::read_history;
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

fn sub(tag: &[u8; 4], bytes: &[u8]) -> Vec<u8> {
    [tag.as_slice(), &(bytes.len() as u16).to_le_bytes(), bytes].concat()
}

fn frame(tag: &[u8; 4], id: u32, payload: &[u8]) -> Vec<u8> {
    [
        tag.as_slice(),
        &(payload.len() as u32).to_le_bytes(),
        &0u32.to_le_bytes(),
        &id.to_le_bytes(),
        &[0; 8],
        payload,
    ]
    .concat()
}

/// Author the actual eighteen-column predecessor independently of the current writer.
/// Optional historical columns do not authenticate this obsolete unbound snapshot as current.
fn old_snapshot(root: &Path, omitted: &[&str], schema_hash: Option<&str>) -> std::path::PathBuf {
    let pack = root.join("history.db");
    let conn = Connection::open(&pack).unwrap();
    conn.execute_batch(
        "CREATE TABLE plugins(id INTEGER PRIMARY KEY,name TEXT,priority INTEGER,checksum BLOB);
         CREATE TABLE record_overrides(
            form_id INTEGER,chain_position INTEGER,plugin_id INTEGER,source_form_id INTEGER,
            deleted INTEGER,record_type TEXT,flags INTEGER,source_record_offset INTEGER,
            source_record_size INTEGER,status TEXT,reason TEXT,source_cell_form_id INTEGER,
            source_worldspace_form_id INTEGER,source_topic_form_id INTEGER,topic_group_offset INTEGER,
            source_path TEXT,schema_sha256 TEXT,plugin_masters TEXT);
         CREATE TABLE inhouse_source_records(form_id INTEGER,record_type TEXT);
         INSERT INTO inhouse_source_records VALUES(6144,'STAT'),(6145,'STAT');",
    )
    .unwrap();
    for (name, kind) in [
        ("source_group_end", "INTEGER"),
        ("frame_complete", "INTEGER"),
        ("record_signature", "BLOB"),
    ] {
        if !omitted.contains(&name) {
            conn.execute_batch(&format!(
                "ALTER TABLE record_overrides ADD COLUMN {name} {kind}"
            ))
            .unwrap();
        }
    }
    for (index, name, masters) in [(0u32, "base.esm", "[]"), (1, "patch.esp", "[\"BASE.ESM\"]")] {
        let mut header = sub(b"HEDR", &[0; 12]);
        if index == 1 {
            header.extend(sub(b"MAST", b"BASE.ESM\0"));
            header.extend(sub(b"DATA", &[0; 8]));
        }
        let mut original = frame(b"TES4", 0, &header);
        let offset = original.len() as u64;
        let payload = sub(
            b"EDID",
            if index == 0 {
                b"base_version\0"
            } else {
                b"patch_version\0"
            },
        );
        original.extend(frame(b"STAT", 0x1800, &payload));
        let neighbor_offset = original.len() as u64;
        let neighbor = sub(b"EDID", b"usable_neighbor\0");
        if index == 0 {
            original.extend(frame(b"STAT", 0x1801, &neighbor));
        }
        let source = root.join(name);
        fs::write(&source, &original).unwrap();
        let checksum = Sha256::digest(&original).to_vec();
        conn.execute(
            "INSERT INTO plugins VALUES(?1,?2,?1,?3)",
            params![index, name, checksum],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO record_overrides(plugin_id,record_type,flags,source_record_offset,source_path,schema_sha256,plugin_masters) VALUES(?1,'TES4',0,0,?2,?3,?4)",
            params![index, source.to_string_lossy(), schema_hash, masters],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO record_overrides(form_id,chain_position,plugin_id,source_form_id,deleted,record_type,flags,source_record_offset,source_record_size,status,reason) VALUES(6144,?1,?1,6144,0,'STAT',0,?2,?3,?4,?5)",
            params![index, offset, payload.len() as u32, if index == 0 { "accepted" } else { "skipped" }, (index == 1).then_some("retained rejection")],
        )
        .unwrap();
        if index == 0 {
            conn.execute(
                "INSERT INTO record_overrides(form_id,chain_position,plugin_id,source_form_id,deleted,record_type,flags,source_record_offset,source_record_size,status) VALUES(6145,0,0,6145,0,'STAT',0,?1,?2,'accepted')",
                params![neighbor_offset, neighbor.len() as u32],
            )
            .unwrap();
        }
        if !omitted.contains(&"source_group_end") {
            conn.execute(
                "UPDATE record_overrides SET source_group_end=?1 WHERE plugin_id=?2",
                params![original.len() as u64, index],
            )
            .unwrap();
        }
        if !omitted.contains(&"frame_complete") {
            conn.execute(
                "UPDATE record_overrides SET frame_complete=1 WHERE plugin_id=?1",
                [index],
            )
            .unwrap();
        }
        if !omitted.contains(&"record_signature") {
            conn.execute(
                "UPDATE record_overrides SET record_signature=?1 WHERE plugin_id=?2 AND record_type='STAT'",
                params![b"STAT".as_slice(), index],
            )
            .unwrap();
        }
    }
    pack
}

struct CurrentFixture {
    pack: std::path::PathBuf,
    data: std::path::PathBuf,
}

/// Publish original plugins through the real current native writer, including its paired context.
fn current_fixture(root: &Path, rejected_patch: bool) -> CurrentFixture {
    let data = root.join("Data");
    fs::create_dir(&data).unwrap();
    let header = sub(b"HEDR", &[0; 12]);
    let base = [
        frame(b"TES4", 0, &header),
        frame(b"STAT", 0x1800, &sub(b"EDID", b"base_version\0")),
        frame(b"STAT", 0x1801, &sub(b"EDID", b"usable_neighbor\0")),
    ]
    .concat();
    let patch_header = [header, sub(b"MAST", b"BASE.ESM\0"), sub(b"DATA", &[0; 8])].concat();
    let mut patch_payload = sub(b"EDID", b"patch_version\0");
    if rejected_patch {
        // Bounded native record, but its optional MODL exceeds the actual body.
        // Real admission must keep the complete predecessor, not a manufactured journal status.
        patch_payload.extend_from_slice(b"MODL\xff\xffbad");
    }
    fs::write(data.join("base.esm"), base).unwrap();
    fs::write(
        data.join("patch.esp"),
        [
            frame(b"TES4", 0, &patch_header),
            frame(b"STAT", 0x1800, &patch_payload),
        ]
        .concat(),
    )
    .unwrap();
    let plugins = root.join("plugins.txt");
    fs::write(&plugins, "*base.esm\n*patch.esp\n").unwrap();
    let pack = root.join("pack");
    converter::esm::inhouse::export_record_bundle(&data, &plugins, &pack).unwrap();
    let db = Connection::open(pack.join("skyrim_world.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT version FROM schema_info", [], |row| row
            .get::<_, u32>(0))
            .unwrap(),
        shared::WORLD_DATABASE_SCHEMA_VERSION
    );
    let context = shared::records::RecordContextReader::new(&db).unwrap();
    let identity = context.identity();
    let owner = identity.runtime_key(0x1800).unwrap().unwrap();
    assert_eq!(
        identity.table().named(owner).unwrap().to_string(),
        "001800:base.esm"
    );
    assert_eq!(context.record(owner).unwrap().source_header, 0x1800);
    assert_eq!(
        db.query_row("SELECT count(*) FROM records", [], |row| row
            .get::<_, usize>(0))
            .unwrap(),
        2
    );
    let columns = db
        .prepare("PRAGMA table_info(record_overrides)")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, bool>(3)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(columns.len(), 23);
    assert!(!columns.iter().any(|(name, _)| name == "plugin_masters"));
    for required in ["source_group_end", "frame_complete", "record_signature"] {
        assert!(
            columns
                .iter()
                .any(|(name, nonnull)| name == required && *nonnull)
        );
    }
    assert_eq!(
        db.query_row(
            "SELECT master_name FROM inhouse_plugin_masters WHERE plugin_id=1 AND master_index=0",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        "BASE.ESM"
    );
    CurrentFixture { pack, data }
}

fn source_bytes(root: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    ["base.esm", "patch.esp"]
        .into_iter()
        .map(|name| {
            let path = root.join(name);
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect()
}

fn assert_source_bytes(before: &[(std::path::PathBuf, Vec<u8>)]) {
    for (path, bytes) in before {
        assert_eq!(
            &fs::read(path).unwrap(),
            bytes,
            "history must not change original source {}",
            path.display()
        );
    }
}

#[test]
fn retained_eighteen_column_history_requires_conversion_without_writes() {
    let fixture = tempfile::tempdir().unwrap();
    let hash = converter::cache::hash_bytes(converter::records::SCHEMA_BYTES);
    let pack = old_snapshot(
        fixture.path(),
        &["source_group_end", "frame_complete", "record_signature"],
        Some(&hash),
    );
    let conn = Connection::open(&pack).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM pragma_table_info('record_overrides')",
            [],
            |row| row.get::<_, u32>(0)
        )
        .unwrap(),
        18
    );
    assert_eq!(conn.query_row("SELECT count(*) FROM sqlite_schema WHERE type='table' AND name IN ('schema_info','pack_identity','record_context')", [], |row| row.get::<_, u32>(0)).unwrap(), 0);
    let available = conn.prepare("SELECT chain_position,plugin_id,source_form_id,status,reason FROM record_overrides WHERE form_id=6144 ORDER BY chain_position").unwrap()
        .query_map([], |row| Ok((row.get::<_, u32>(0)?,row.get::<_, u32>(1)?,row.get::<_, u32>(2)?,row.get::<_, String>(3)?,row.get::<_, Option<String>>(4)?))).unwrap()
        .collect::<rusqlite::Result<Vec<_>>>().unwrap();
    assert_eq!(
        available,
        vec![
            (0, 0, 0x1800, "accepted".into(), None),
            (
                1,
                1,
                0x1800,
                "skipped".into(),
                Some("retained rejection".into())
            )
        ]
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM record_overrides WHERE form_id=6145 AND status='accepted'",
            [],
            |row| row.get::<_, u32>(0)
        )
        .unwrap(),
        1
    );
    drop(conn);
    let before = fs::read(&pack).unwrap();
    let sources = source_bytes(fixture.path());
    for (id, data) in [(0x1800, None), (0x1801, Some(fixture.path()))] {
        let error = read_history(&pack, data, id).unwrap_err();
        assert!(format!("{error:#}").contains("convert again"), "{error:#}");
        assert_eq!(
            fs::read(&pack).unwrap(),
            before,
            "obsolete snapshot refusal must be read-only"
        );
        assert_source_bytes(&sources);
    }
}

#[test]
fn current_native_history_keeps_rejected_chain_and_usable_neighbor() {
    let root = tempfile::tempdir().unwrap();
    let fixture = current_fixture(root.path(), true);
    let database = fixture.pack.join("skyrim_world.db");
    let before = fs::read(&database).unwrap();
    let sources = source_bytes(&fixture.data);
    let report = read_history(&fixture.pack, None, 0x1800).unwrap();
    assert_eq!(report.versions.len(), 2);
    assert_eq!(
        report
            .versions
            .iter()
            .map(|v| v.chain_position)
            .collect::<Vec<_>>(),
        [0, 1]
    );
    assert_eq!(
        report
            .versions
            .iter()
            .map(|v| v.plugin.as_str())
            .collect::<Vec<_>>(),
        ["base.esm", "patch.esp"]
    );
    assert_eq!(report.versions[0].status, "accepted");
    assert!(report.versions[0].winner);
    assert_eq!(report.versions[0].decode_status, "decoded");
    assert_eq!(report.versions[0].record.as_ref().unwrap().form_id, 0x1800);
    assert_eq!(
        report.versions[0]
            .record
            .as_ref()
            .unwrap()
            .fields
            .iter()
            .find(|field| field.signature == *b"EDID")
            .unwrap()
            .value,
        converter::records::Value::String("base_version".into())
    );
    assert_eq!(report.versions[1].status, "skipped");
    assert_eq!(
        report.versions[1].reason.as_deref(),
        Some("incomplete override keeps the earlier complete version")
    );
    assert!(!report.versions[1].winner);
    assert_eq!(report.versions[1].decode_status, "undecodable");
    assert!(report.versions[1].record.is_none());
    assert!(
        report.versions[1]
            .decode_reason
            .as_deref()
            .unwrap()
            .contains("undecodable fields or incomplete subrecord framing")
    );
    assert!(
        report
            .versions
            .iter()
            .all(|version| version.source_form_id == 0x1800
                && version.source_record_offset > 24
                && version.source_record_size > 0)
    );
    let neighbor = read_history(&fixture.pack, Some(&fixture.data), 0x1801).unwrap();
    assert_eq!(neighbor.versions.len(), 1);
    assert!(neighbor.versions[0].winner);
    assert_eq!(neighbor.versions[0].source_form_id, 0x1801);
    assert_eq!(neighbor.versions[0].decode_status, "decoded");
    assert_eq!(
        neighbor.versions[0].record.as_ref().unwrap().form_id,
        0x1801
    );
    assert_eq!(
        neighbor.versions[0]
            .record
            .as_ref()
            .unwrap()
            .fields
            .iter()
            .find(|field| field.signature == *b"EDID")
            .unwrap()
            .value,
        converter::records::Value::String("usable_neighbor".into())
    );
    assert_eq!(fs::read(database).unwrap(), before);
    assert_source_bytes(&sources);
}

#[test]
fn replay_requires_each_authenticated_column_and_matching_schema() {
    for (damage, expected) in [
        (None, "decoded"),
        (Some("source_group_end"), "containing-group bounds"),
        (Some("frame_complete"), "frame completeness"),
        (Some("record_signature"), "raw record signature"),
        (Some("older_schema"), "diagnostic schema differs"),
        (Some("null_schema"), "schema_sha256"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let fixture = current_fixture(root.path(), false);
        let database = fixture.pack.join("skyrim_world.db");
        let conn = Connection::open(&database).unwrap();
        match damage {
            None => {}
            Some("source_group_end" | "frame_complete" | "record_signature") => {
                // Deliberately damage one journal authority field in a genuinely generated
                // current world. This is not an obsolete-layout adapter or a positive format stamp.
                conn.execute_batch(&format!(
                    "ALTER TABLE record_overrides DROP COLUMN {}",
                    damage.unwrap()
                ))
                .unwrap();
            }
            Some("older_schema") => {
                conn.execute("UPDATE record_overrides SET schema_sha256='older_schema' WHERE record_type='TES4'", []).unwrap();
            }
            Some("null_schema") => {
                conn.execute(
                    "UPDATE record_overrides SET schema_sha256=NULL WHERE record_type='TES4'",
                    [],
                )
                .unwrap();
            }
            Some(other) => panic!("unknown fixture damage {other}"),
        }
        drop(conn);
        let before = fs::read(&database).unwrap();
        let sources = source_bytes(&fixture.data);
        let report = read_history(&fixture.pack, None, 0x1800).unwrap();
        assert_eq!(report.versions.len(), 2);
        assert_eq!(
            report
                .versions
                .iter()
                .map(|v| v.chain_position)
                .collect::<Vec<_>>(),
            [0, 1]
        );
        assert_eq!(
            report
                .versions
                .iter()
                .map(|v| v.plugin.as_str())
                .collect::<Vec<_>>(),
            ["base.esm", "patch.esp"]
        );
        for version in &report.versions {
            assert_eq!(version.status, "accepted");
            assert_eq!(version.source_form_id, 0x1800);
            assert!(version.source_record_offset > 24 && version.source_record_size > 0);
            if expected == "decoded" {
                assert_eq!(version.decode_status, "decoded");
                assert_eq!(version.record.as_ref().unwrap().form_id, 0x1800);
                assert_eq!(
                    version
                        .record
                        .as_ref()
                        .unwrap()
                        .fields
                        .iter()
                        .find(|field| field.signature == *b"EDID")
                        .unwrap()
                        .value,
                    converter::records::Value::String(
                        if version.chain_position == 0 {
                            "base_version"
                        } else {
                            "patch_version"
                        }
                        .into()
                    )
                );
            } else {
                assert_eq!(version.decode_status, "undecodable");
                assert!(version.record.is_none());
                assert!(
                    version.decode_reason.as_deref().unwrap().contains(expected),
                    "{:?}",
                    version.decode_reason
                );
            }
        }
        assert!(!report.versions[0].winner);
        assert!(report.versions[1].winner);
        if damage.is_none() {
            assert!(
                report
                    .fields
                    .iter()
                    .any(|field| field.field.starts_with("EDID:") && field.different)
            );
        } else {
            assert!(report.fields.is_empty());
        }
        assert_eq!(fs::read(database).unwrap(), before);
        assert_source_bytes(&sources);
    }
}
