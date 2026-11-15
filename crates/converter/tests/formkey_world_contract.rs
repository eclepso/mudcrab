//! Generated production world contracts; every native fixture is created inside cargo test.
use converter::esm::EsmParser;
use rusqlite::Connection;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Frame a native subrecord with an explicit bounded payload.
fn sub(tag: &[u8; 4], data: &[u8]) -> Vec<u8> {
    [tag.as_slice(), &(data.len() as u16).to_le_bytes(), data].concat()
}
/// Frame a native record, keeping its supplier-relative header untouched.
fn record(tag: &[u8; 4], id: u32, data: &[u8]) -> Vec<u8> {
    [
        tag.as_slice(),
        &(data.len() as u32).to_le_bytes(),
        &[0; 4],
        &id.to_le_bytes(),
        &[0; 8],
        data,
    ]
    .concat()
}
/// Write one generated plugin with its original MAST sequence.
fn plugin(root: &Path, name: &str, masters: &[&str], body: &[u8]) {
    let mut header = sub(
        b"HEDR",
        &[1.71f32.to_le_bytes(), [0; 4], 0x800u32.to_le_bytes()].concat(),
    );
    for master in masters {
        header.extend(sub(b"MAST", format!("{master}\0").as_bytes()));
        header.extend(sub(b"DATA", &[0; 8]));
    }
    fs::write(
        root.join(name),
        [record(b"TES4", 0, &header), body.to_vec()].concat(),
    )
    .unwrap();
}
/// Allocation Alpha/Patch/Zulu differs from priority Zulu/Alpha/Patch and MAST Alpha/Zulu.
fn world(root: &Path) -> (PathBuf, Connection) {
    plugin(
        root,
        "Zulu.esm",
        &[],
        &record(b"STAT", 0x811, &sub(b"EDID", b"Before\0")),
    );
    plugin(
        root,
        "Alpha.esm",
        &[],
        &record(b"TXST", 0x822, &sub(b"EDID", b"Texture\0")),
    );
    let mods = [
        1u32.to_le_bytes(),
        4u32.to_le_bytes(),
        *b"Rock",
        0x822u32.to_le_bytes(),
        3u32.to_le_bytes(),
    ]
    .concat();
    plugin(
        root,
        "Patch.esp",
        &["Alpha.esm", "Zulu.esm"],
        &record(
            b"STAT",
            0x0100_0811,
            &[sub(b"EDID", b"PatchWon\0"), sub(b"MODS", &mods)].concat(),
        ),
    );
    let db = root.join("skyrim_world.db");
    EsmParser::convert_plugins(
        &["Zulu.esm", "Alpha.esm", "Patch.esp"].map(|name| root.join(name)),
        &db,
    )
    .unwrap();
    let conn = Connection::open(&db).unwrap();
    (db, conn)
}
/// A production world authenticates one allocation in the same published database.
#[test]
fn production_world_has_authenticated_plugin_allocation() {
    let root = tempfile::tempdir().unwrap();
    let (_, conn) = world(root.path());
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name='pack_identity'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 1,
        "production world must contain its authenticated FormKey allocation"
    );
}
/// Winning priority does not replace the original owning-plugin identity.
#[test]
fn production_world_keys_use_owner_allocation_not_legacy_slots() {
    let root = tempfile::tempdir().unwrap();
    let (_, conn) = world(root.path());
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM records WHERE form_id=50333713",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 1,
        "Zulu-owned winner must use literal FormKey 03000811"
    );
}
/// Canonical words remain native; semantic links are a separate checked relation.
#[test]
fn production_canonical_fields_keep_source_words() {
    let root = tempfile::tempdir().unwrap();
    let (_, conn) = world(root.path());
    let blob: Vec<u8> = conn
        .query_row(
            "SELECT data FROM records WHERE record_type='STAT'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let record =
        rkyv::from_bytes::<shared::records::ArchivedRecordData, rkyv::rancor::Error>(&blob)
            .unwrap();
    let mods = &record
        .subrecords
        .iter()
        .find(|field| field.tag == *b"MODS")
        .unwrap()
        .data;
    assert_eq!(
        &mods[12..16],
        &0x822u32.to_le_bytes(),
        "canonical field must preserve original Alpha MAST[0] word"
    );
}

/// Source metadata and stored root/group keys must agree before any canonical field is used.
#[test]
fn production_context_refuses_wrong_owner_and_group_metadata() {
    for tamper in [
        "UPDATE record_context SET source_header=2065 WHERE record_key=50333713",
        "UPDATE records SET cell_id=50333713 WHERE form_id=50333713",
        "UPDATE records SET worldspace_id=50333713 WHERE form_id=50333713",
    ] {
        let root = tempfile::tempdir().unwrap();
        let (_, connection) = world(root.path());
        let reader = shared::records::RecordContextReader::new(&connection).unwrap();
        let key = reader.identity().required_key(50333713).unwrap();
        connection.execute(tamper, []).unwrap();
        let error = reader.record(key).unwrap_err();
        assert!(error.to_string().contains("convert again"), "{error:#}");
    }
}
