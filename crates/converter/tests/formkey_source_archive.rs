//! Native, generated-at-test-time fixtures; no installed game data or runtime slots.
use converter::esm::{EsmParser, form_key_store::SourceArchive};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

/// Archive group keys use original MAST order, independently of owner allocation.
fn grouped_conversion(root: &Path) -> converter::esm::form_key_merge::FormKeyConversion {
    plugin(root, "Zulu.esm", &[], false, &[]);
    plugin(root, "Alpha.esm", &[], false, &[]);
    plugin(
        root,
        "Patch.esp",
        &["Alpha.esm", "Zulu.esm"],
        false,
        &group(
            0x0000_5678,
            1,
            &group(0x0100_1234, 6, &record(b"REFR", 0x0200_0912, &[])),
        ),
    );
    EsmParser::merge_plugins_form_keys(
        &["Zulu.esm", "Alpha.esm", "Patch.esp"].map(|name| root.join(name)),
    )
    .unwrap()
}

/// A valid key or NULL cannot replace the supplier-relative group identity.
#[test]
fn archive_reader_refuses_group_context_tampering() {
    let root = tempfile::tempdir().unwrap();
    let converted = grouped_conversion(root.path());
    let mut connection = Connection::open_in_memory().unwrap();
    converted.write_source_archive(&mut connection).unwrap();
    for update in [
        "UPDATE source_records SET cell_key=cell_key+1",
        "UPDATE source_records SET cell_key=16781876",
        "UPDATE source_records SET cell_key=NULL",
        "UPDATE source_records SET world_key=world_key+1",
        "UPDATE source_records SET world_key=NULL",
        "UPDATE source_records SET source_cell=NULL",
    ] {
        connection.execute_batch("SAVEPOINT tamper").unwrap();
        connection.execute_batch(update).unwrap();
        assert!(
            SourceArchive::read(&connection).is_err(),
            "group context accepted: {update}"
        );
        connection
            .execute_batch("ROLLBACK TO tamper; RELEASE tamper")
            .unwrap();
    }
    assert_eq!(SourceArchive::read(&connection).unwrap().records.len(), 1);
}

/// Inconsistent internal group keys roll back the complete archive and its allocation.
#[test]
fn archive_writer_rolls_back_inconsistent_group_context() {
    let root = tempfile::tempdir().unwrap();
    let mut converted = grouped_conversion(root.path());
    converted.records.values_mut().next().unwrap().cell = None;
    let mut connection = Connection::open_in_memory().unwrap();
    assert!(
        converted.write_source_archive(&mut connection).is_err(),
        "writer accepted a missing group key"
    );
    assert_eq!(
        connection
            .query_row::<i64, _, _>(
                "SELECT count(*) FROM sqlite_schema WHERE type='table'",
                [],
                |row| row.get(0)
            )
            .unwrap(),
        0
    );
}

/// Caller-owned tables do not participate in archive referential-integrity checks.
#[test]
fn archive_reader_preserves_unrelated_caller_foreign_keys() {
    let root = tempfile::tempdir().unwrap();
    let converted = grouped_conversion(root.path());
    let mut connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("PRAGMA foreign_keys=OFF; CREATE TABLE caller_parent (id INTEGER PRIMARY KEY); CREATE TABLE caller_child (parent INTEGER REFERENCES caller_parent(id)); INSERT INTO caller_child VALUES(77)").unwrap();
    converted.write_source_archive(&mut connection).unwrap();
    assert!(
        SourceArchive::read(&connection).is_ok(),
        "caller-owned orphan rejected a valid archive"
    );
    assert_eq!(
        connection
            .query_row::<i64, _, _>("SELECT parent FROM caller_child", [], |row| row.get(0))
            .unwrap(),
        77
    );
    connection
        .execute_batch("UPDATE source_records SET supplier=999")
        .unwrap();
    assert!(SourceArchive::read(&connection).is_err());
}

/// Step 2's outfit, skin and alternate-texture self links retain physical source sites.
#[test]
fn archive_keeps_merged_step2_self_link_offsets() {
    let root = tempfile::tempdir().unwrap();
    let word = 0xFF00_0812u32.to_le_bytes();
    let mods = [
        1u32.to_le_bytes(),
        4u32.to_le_bytes(),
        *b"Rock",
        word,
        3u32.to_le_bytes(),
    ]
    .concat();
    let body = [
        record(b"STAT", 0x812, &[]),
        record(b"NPC_", 0x820, &sub(b"DOFT", &word)),
        record(b"OTFT", 0x821, &sub(b"INAM", &[word, word].concat())),
        record(
            b"ARMA",
            0x822,
            &[sub(b"NAM0", &word), sub(b"NAM1", &word)].concat(),
        ),
        record(b"STAT", 0x823, &sub(b"MODS", &mods)),
    ]
    .concat();
    plugin(root.path(), "Links.esp", &[], false, &body);
    let mut connection = Connection::open_in_memory().unwrap();
    EsmParser::export_form_key_source_archive(&[root.path().join("Links.esp")], &mut connection)
        .unwrap();
    let archive = SourceArchive::read(&connection).unwrap();
    let sites = archive
        .records
        .iter()
        .flat_map(|row| {
            row.links.iter().map(move |link| {
                (
                    row.key.local(),
                    link.subrecord,
                    link.byte_offset,
                    link.source_word,
                    link.key.unwrap().to_packed(),
                )
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        sites,
        [
            (0x820, 0, 0, 0xFF00_0812, 0x0100_0812),
            (0x821, 0, 0, 0xFF00_0812, 0x0100_0812),
            (0x821, 0, 4, 0xFF00_0812, 0x0100_0812),
            (0x822, 0, 0, 0xFF00_0812, 0x0100_0812),
            (0x822, 1, 0, 0xFF00_0812, 0x0100_0812),
            (0x823, 0, 12, 0xFF00_0812, 0x0100_0812),
        ]
    );
}

/// Encode a bounded native subrecord during the test.
fn sub(tag: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    [
        tag.as_slice(),
        &(payload.len() as u16).to_le_bytes(),
        payload,
    ]
    .concat()
}

/// Encode a native record header with an explicit supplier-relative word.
fn record(tag: &[u8; 4], id: u32, payload: &[u8]) -> Vec<u8> {
    [
        tag.as_slice(),
        &(payload.len() as u32).to_le_bytes(),
        &[0; 4],
        &id.to_le_bytes(),
        &[0; 8],
        payload,
    ]
    .concat()
}

/// Nest test records beneath an original source group label.
fn group(label: u32, kind: i32, payload: &[u8]) -> Vec<u8> {
    [
        b"GRUP".as_slice(),
        &((payload.len() + 24) as u32).to_le_bytes(),
        &label.to_le_bytes(),
        &kind.to_le_bytes(),
        &[0; 8],
        payload,
    ]
    .concat()
}

/// Write a generated plugin with original master order and header flags.
fn plugin(root: &Path, name: &str, masters: &[&str], light: bool, body: &[u8]) {
    let mut header = sub(
        b"HEDR",
        &[1.71f32.to_le_bytes(), [0; 4], 0x800u32.to_le_bytes()].concat(),
    );
    for master in masters {
        header.extend(sub(b"MAST", format!("{master}\0").as_bytes()));
        header.extend(sub(b"DATA", &[0; 8]));
    }
    let mut bytes = record(b"TES4", 0, &header);
    bytes[8..12].copy_from_slice(&if light { 0x200u32 } else { 0 }.to_le_bytes());
    bytes.extend_from_slice(body);
    fs::write(root.join(name), bytes).unwrap();
}

/// Two same-valued object-array items and a scalar, with nonzero alias/padding words.
fn vmad(word: u32) -> Vec<u8> {
    [
        5u16.to_le_bytes().as_slice(),
        &2u16.to_le_bytes(),
        &1u16.to_le_bytes(),
        &1u16.to_le_bytes(),
        b"S",
        &[0],
        &2u16.to_le_bytes(),
        &1u16.to_le_bytes(),
        b"P",
        &[11, 1],
        &2u32.to_le_bytes(),
        &[7, 0, 0xA1, 0xB2],
        &word.to_le_bytes(),
        &[9, 0, 0xC3, 0xD4],
        &word.to_le_bytes(),
        &1u16.to_le_bytes(),
        b"Q",
        &[1, 1],
        &[11, 0, 0xE5, 0xF6],
        &word.to_le_bytes(),
        // Opaque fragment suffix: a matching word is not another primary-script object.
        &word.to_le_bytes(),
    ]
    .concat()
}

#[test]
fn native_archive_round_trip_keeps_supplier_sites_and_portable_names() {
    let root = tempfile::tempdir().unwrap();
    plugin(
        root.path(),
        "Zulu.esm",
        &[],
        false,
        &record(b"STAT", 0x811, &sub(b"EDID", b"zulu\0")),
    );
    plugin(
        root.path(),
        "Alpha.esm",
        &[],
        false,
        &record(b"STAT", 0x811, &sub(b"EDID", b"alpha\0")),
    );
    let word = 0x0000_0811u32; // Original Patch MAST[0] is Alpha, not supplier priority zero.
    let payload = [
        sub(b"EDID", b"placed\0"),
        sub(b"NAME", &word.to_le_bytes()),
        sub(
            b"XAPR",
            &[word.to_le_bytes(), 1.25f32.to_le_bytes()].concat(),
        ),
        sub(b"ZZZZ", &word.to_le_bytes()),
        sub(
            b"XAPR",
            &[word.to_le_bytes(), 2.5f32.to_le_bytes()].concat(),
        ),
        sub(b"XLKR", &[word.to_le_bytes(), word.to_le_bytes()].concat()),
    ]
    .concat();
    let scripts = vmad(word);
    let body = [
        group(
            0x0000_5678,
            1,
            &group(0x0100_1234, 6, &record(b"REFR", 0x0200_0912, &payload)),
        ),
        record(
            b"ACTI",
            0xFF00_0913,
            &[sub(b"EDID", b"scripted\0"), sub(b"VMAD", &scripts)].concat(),
        ),
    ]
    .concat();
    plugin(
        root.path(),
        "Patch.esp",
        &["Alpha.esm", "Zulu.esm"],
        false,
        &body,
    );
    let paths = ["Zulu.esm", "Alpha.esm", "Patch.esp"].map(|name| root.path().join(name));
    let mut connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "CREATE TABLE caller_notes (note TEXT); INSERT INTO caller_notes VALUES('retain me')",
        )
        .unwrap();
    EsmParser::export_form_key_source_archive(&paths, &mut connection).unwrap();
    let archive = SourceArchive::read(&connection).unwrap();
    assert_eq!(
        archive
            .records
            .iter()
            .map(|row| row.key.to_packed())
            .collect::<Vec<_>>(),
        [0x0300_0811, 0x0100_0811, 0x0200_0912, 0x0200_0913]
    );
    let placed = &archive.records[2];
    assert_eq!(placed.source.form_id, 0x0200_0912);
    assert_eq!(placed.source.cell_form_id, Some(0x0100_1234));
    assert_eq!(placed.cell.unwrap().to_packed(), 0x0300_1234);
    assert_eq!(placed.source.worldspace_form_id, Some(0x0000_5678));
    assert_eq!(placed.worldspace.unwrap().to_packed(), 0x0100_5678);
    assert_eq!(
        archive
            .identity
            .table()
            .entry(placed.supplier)
            .unwrap()
            .display_name,
        "Patch.esp"
    );
    assert_eq!(
        placed
            .links
            .iter()
            .map(|link| (
                link.subrecord,
                link.byte_offset,
                link.key.unwrap().to_packed()
            ))
            .collect::<Vec<_>>(),
        [
            (1, 0, 0x0100_0811),
            (2, 0, 0x0100_0811),
            (4, 0, 0x0100_0811),
            (5, 0, 0x0100_0811),
            (5, 4, 0x0100_0811)
        ]
    );
    assert_eq!(
        placed.source.subrecords[3],
        (b"ZZZZ".to_vec(), word.to_le_bytes().to_vec())
    );
    let scripted = &archive.records[3];
    assert_eq!(scripted.source.subrecords[1].1, scripts);
    assert_eq!(
        scripted
            .links
            .iter()
            .map(|link| (link.subrecord, link.byte_offset))
            .collect::<Vec<_>>(),
        [(1, 25), (1, 33), (1, 46)]
    );
    assert_eq!(scripted.source.form_id, 0xFF00_0913); // >= MAST count is supplier self.
    let portable = archive.portable_json().unwrap();
    assert_eq!(portable["records"][2]["key"], "000912:Patch.esp");
    assert_eq!(
        portable["records"][2]["links"][0]["target"],
        "000811:Alpha.esm"
    );
    assert_eq!(
        portable["records"][3]["links"][2]["target"],
        "000811:Alpha.esm"
    );
    assert_eq!(
        archive
            .identity
            .table()
            .entry(placed.supplier)
            .unwrap()
            .facts
            .content_hash
            .unwrap(),
        <[u8; 32]>::from(Sha256::digest(fs::read(&paths[2]).unwrap()))
    );
    assert_eq!(
        connection
            .query_row::<String, _, _>("SELECT note FROM caller_notes", [], |row| row.get(0))
            .unwrap(),
        "retain me"
    );
    assert!(EsmParser::export_form_key_source_archive(&paths, &mut connection).is_err());
    assert_eq!(SourceArchive::read(&connection).unwrap().records.len(), 4);
}

#[test]
fn broken_mod_archive_publication_skips_bad_candidate_warns_and_retains_neighbors() {
    let root = tempfile::tempdir().unwrap();
    let body = [
        record(b"STAT", 0x811, &sub(b"EDID", b"before\0")),
        record(b"REFR", 0x812, &sub(b"NAME", &[0x11, 0x22, 0x33])),
        record(b"STAT", 0x813, &sub(b"EDID", b"after\0")),
    ]
    .concat();
    plugin(root.path(), "Broken.esp", &[], false, &body);
    let database = root.path().join("source.db");
    let mut connection = Connection::open(&database).unwrap();
    EsmParser::export_form_key_source_archive(&[root.path().join("Broken.esp")], &mut connection)
        .unwrap();
    drop(connection);
    let archive = SourceArchive::read(&Connection::open(database).unwrap()).unwrap();
    assert_eq!(
        archive
            .records
            .iter()
            .map(|record| record.key.local())
            .collect::<Vec<_>>(),
        [0x811, 0x813]
    );
    assert_eq!(archive.records[0].source.subrecords[0].1, b"before\0");
    assert_eq!(archive.records[1].source.subrecords[0].1, b"after\0");
    assert!(
        archive
            .warnings
            .iter()
            .any(|warning| warning.contains("Broken.esp")
                && warning.contains("unusable record candidate")
                && warning.contains("NAME")),
        "{:?}",
        archive.warnings
    );
}

#[test]
fn archive_refuses_old_world_and_rolls_back_invalid_internal_sites() {
    let root = tempfile::tempdir().unwrap();
    plugin(
        root.path(),
        "A.esm",
        &[],
        false,
        &record(b"REFR", 0x811, &sub(b"NAME", &0x812u32.to_le_bytes())),
    );
    let mut converted = EsmParser::merge_plugins_form_keys(&[root.path().join("A.esm")]).unwrap();
    let mut old = Connection::open_in_memory().unwrap();
    old.execute_batch(
        "CREATE TABLE records (form_id INTEGER, data BLOB); INSERT INTO records VALUES(7,x'1122')",
    )
    .unwrap();
    assert!(
        SourceArchive::read(&old)
            .unwrap_err()
            .to_string()
            .contains("convert again")
    );
    assert!(
        converted
            .write_source_archive(&mut old)
            .unwrap_err()
            .to_string()
            .contains("fresh destination")
    );
    assert_eq!(
        old.query_row::<String, _, _>("SELECT hex(data) FROM records", [], |row| row.get(0))
            .unwrap(),
        "1122"
    );
    converted.records.values_mut().next().unwrap().links[0].byte_offset = 1;
    let mut fresh = Connection::open_in_memory().unwrap();
    assert!(converted.write_source_archive(&mut fresh).is_err());
    assert_eq!(
        fresh
            .query_row::<i64, _, _>(
                "SELECT count(*) FROM sqlite_schema WHERE type='table'",
                [],
                |row| row.get(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn archive_reader_refuses_unknown_table_and_bad_physical_coordinates() {
    let root = tempfile::tempdir().unwrap();
    plugin(
        root.path(),
        "A.esm",
        &[],
        false,
        &record(b"REFR", 0x811, &sub(b"NAME", &0x812u32.to_le_bytes())),
    );
    let mut connection = Connection::open_in_memory().unwrap();
    EsmParser::export_form_key_source_archive(&[root.path().join("A.esm")], &mut connection)
        .unwrap();
    connection
        .execute_batch("UPDATE source_archive SET version=0")
        .unwrap();
    assert!(
        SourceArchive::read(&connection)
            .unwrap_err()
            .to_string()
            .contains("convert again")
    );
    connection
        .execute_batch("UPDATE source_archive SET version=1")
        .unwrap();
    connection
        .execute_batch("UPDATE source_links SET byte_offset=1")
        .unwrap();
    assert!(SourceArchive::read(&connection).is_err());
    connection.execute_batch("UPDATE source_links SET byte_offset=0; UPDATE source_links SET target_key=target_key+1").unwrap();
    assert!(
        SourceArchive::read(&connection).is_err(),
        "an allocated but incorrect local ID is not the source word's target"
    );
    connection.execute_batch("UPDATE source_links SET byte_offset=0; UPDATE pack_plugins SET display_name='Other.esm'").unwrap();
    assert!(SourceArchive::read(&connection).is_err());
}

#[test]
fn native_archive_null_targets_and_repeat_sql_bytes_are_deterministic() {
    let root = tempfile::tempdir().unwrap();
    plugin(
        root.path(),
        "Null.esp",
        &[],
        false,
        &record(b"REFR", 0x811, &sub(b"NAME", &0u32.to_le_bytes())),
    );
    let paths = [root.path().join("Null.esp")];
    let mut outputs = Vec::new();
    for name in ["one.db", "two.db"] {
        let database = root.path().join(name);
        let mut connection = Connection::open(&database).unwrap();
        EsmParser::export_form_key_source_archive(&paths, &mut connection).unwrap();
        let archive = SourceArchive::read(&connection).unwrap();
        assert_eq!(archive.records[0].links.len(), 1);
        assert!(archive.records[0].links[0].key.is_none());
        assert!(archive.portable_json().unwrap()["records"][0]["links"][0]["target"].is_null());
        assert_eq!(
            connection
                .query_row::<String, _, _>(
                    "SELECT typeof(target_key) FROM source_links",
                    [],
                    |row| row.get(0)
                )
                .unwrap(),
            "null"
        );
        drop(connection);
        outputs.push(fs::read(database).unwrap());
    }
    assert_eq!(outputs[0], outputs[1]);
}

#[test]
fn native_sql_storage_has_no_full_or_light_slot_cap() {
    let root = tempfile::tempdir().unwrap();
    let mut paths = Vec::new();
    for (count, light, prefix, extension) in
        [(255, false, "Full", "esm"), (4097, true, "Light", "esl")]
    {
        for index in 0..count {
            let name = format!("{prefix}{index:04}.{extension}");
            plugin(
                root.path(),
                &name,
                &[],
                light,
                &record(b"STAT", 0xABC, &sub(b"EDID", b"kept\0")),
            );
            paths.push(root.path().join(name));
        }
    }
    let mut connection = Connection::open_in_memory().unwrap();
    EsmParser::export_form_key_source_archive(&paths, &mut connection).unwrap();
    let archive = SourceArchive::read(&connection).unwrap();
    assert_eq!(archive.records.len(), 4352);
    assert_eq!(
        archive.records.last().unwrap().key.to_sql_i64(),
        73_014_446_780
    );
    assert_eq!(
        archive.portable_json().unwrap()["records"][4351]["key"],
        "000ABC:Light4096.esl"
    );
}
