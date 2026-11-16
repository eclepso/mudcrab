//! SQL determinism checks include every table, including SQLite shadow tables.
use converter::esm::EsmParser;
use rusqlite::Connection;

/// Quote an SQLite identifier without assuming application-owned names.
fn identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Capture all schema objects and sorted rows of every table, without implicit rowids.
fn logical_dump(conn: &Connection) -> Vec<String> {
    let mut dump = Vec::new();
    let mut schema = conn
        .prepare("SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY type, name")
        .unwrap();
    let objects = schema
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    for (kind, name, table, sql) in objects {
        dump.push(format!("{kind:?} {name:?} {table:?} {sql:?}"));
        if kind != "table" {
            continue;
        }
        let mut query = conn
            .prepare(&format!("SELECT * FROM {}", identifier(&name)))
            .unwrap();
        let columns = query.column_count();
        let mut rows = query.query([]).unwrap();
        let mut values = Vec::new();
        while let Some(row) = rows.next().unwrap() {
            let fields: Vec<_> = (0..columns)
                .map(|index| format!("{:?}", row.get_ref(index).unwrap()))
                .collect();
            values.push(format!("{fields:?}"));
        }
        values.sort();
        dump.extend(values);
    }
    dump
}

/// Repeated independent native merges keep exact bytes and all tables deterministic.
/// Genuine full/light source priorities deliberately differ from pack allocation order.
#[test]
fn cell_exports_have_identical_bytes_and_all_table_rows() {
    let temp = tempfile::tempdir().unwrap();
    let names = ["Z.esm", "First.esl", "A.esm", "Last.esl"];
    let paths = names.map(|name| temp.path().join(name));
    for (supplier, path) in paths.iter().enumerate() {
        let mut bytes = dummy_content::inhouse_items::record(b"TES4", 0, &[]);
        if names[supplier].ends_with(".esl") {
            bytes[8..12].copy_from_slice(&0x200u32.to_le_bytes());
        }
        for index in (supplier as u32..256).step_by(4) {
            bytes.extend(dummy_content::inhouse_items::record(
                b"CELL",
                0x800 + index,
                &[(b"FULL", format!("Cell {index}\0").into_bytes())],
            ));
        }
        std::fs::write(path, bytes).unwrap();
    }
    let mut baseline = None;
    for pass in 0..12 {
        let path = temp.path().join(format!("{pass}.db"));
        // Each public conversion constructs independent native winner/projection maps;
        // source bytes and physical provenance remain identical on all twelve runs.
        EsmParser::convert_plugins(&paths, &path).unwrap();
        let conn = Connection::open(&path).unwrap();
        let identity = shared::pack_identity::PackIdentity::read_sql(&conn).unwrap();
        shared::records::RecordContextReader::new(&conn).unwrap();
        let priorities = identity
            .order()
            .iter()
            .map(|id| identity.table().entry(*id).unwrap().display_name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(priorities, names);
        assert_ne!(identity.order()[0], identity.table().entries()[0].id);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM records", [], |row| row
                .get::<_, usize>(0))
                .unwrap(),
            256
        );
        let light =
            shared::form_key::FormKey::new(identity.table().id("First.esl").unwrap(), 0x801)
                .unwrap();
        assert_eq!(
            identity.runtime_id(light).unwrap(),
            0xfe00_0801,
            "unsigned gameplay handles remain checked views, never stored SQL identities"
        );
        let logical = logical_dump(&conn);
        drop(conn);
        let current = (std::fs::read(path).unwrap(), logical);
        if let Some(expected) = &baseline {
            assert_eq!(&current, expected, "independent native export {pass}");
        } else {
            baseline = Some(current);
        }
    }
}

/// Full typed exports cover native history, terrain offsets/layers and every bundle payload.
#[test]
fn typed_history_terrain_bundles_are_reproducible() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("Data");
    dummy_content::inhouse_history::generate(&data).unwrap();
    let mut baseline = None;
    for pass in 0..4 {
        let output = temp.path().join(format!("bundle-{pass}"));
        converter::esm::inhouse::export_record_bundle(&data, &data.join("plugins.txt"), &output)
            .unwrap();
        let db = output.join("skyrim_world.db");
        let conn = Connection::open(&db).unwrap();
        for table in [
            "record_overrides",
            "inhouse_terrain_source_order",
            "inhouse_terrain_layers",
        ] {
            let count: u32 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert!(count > 0, "fixture must exercise {table}");
        }
        let logical = logical_dump(&conn);
        drop(conn);
        let mut files = std::collections::BTreeMap::new();
        collect_payloads(&output, &output, &mut files);
        let current = (files, logical);
        if let Some(expected) = &baseline {
            assert_eq!(&current, expected, "full typed bundle {pass}");
        } else {
            baseline = Some(current);
        }
    }
}

/// Include SQLite bytes and all serialized payloads; measured durations are diagnostic only.
fn collect_payloads(
    root: &std::path::Path,
    directory: &std::path::Path,
    files: &mut std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>,
) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_payloads(root, &path, files);
        } else if path.file_name().unwrap() != "inhouse-database-profile.json" {
            files.insert(
                path.strip_prefix(root).unwrap().to_owned(),
                std::fs::read(path).unwrap(),
            );
        }
    }
}
