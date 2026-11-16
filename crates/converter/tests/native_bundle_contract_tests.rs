//! Native bundle scope, ordering and byte contracts from independently framed plugin inputs.
use converter::{
    esm::inhouse::{RecordBundleMode, export_record_bundle_typed_with_options},
    records::{diagnostics::read_native_for_form, history::read_native_chain},
};
use rusqlite::Connection;
use serde_json::{Value, json};
use shared::{form_key::NamedFormKey, pack_identity::PackIdentity, records::RecordContextReader};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Frame original bytes without using the production decoder or key packer.
fn sub(tag: &[u8; 4], body: &[u8]) -> Vec<u8> {
    [tag.as_slice(), &(body.len() as u16).to_le_bytes(), body].concat()
}

fn record(tag: &[u8; 4], source_word: u32, flags: u32, body: &[u8]) -> Vec<u8> {
    [
        tag.as_slice(),
        &(body.len() as u32).to_le_bytes(),
        &flags.to_le_bytes(),
        &source_word.to_le_bytes(),
        &0x1357_2468u32.to_le_bytes(),
        &44u16.to_le_bytes(),
        &0xA571u16.to_le_bytes(),
        body,
    ]
    .concat()
}

fn plugin(data: &Path, name: &str, masters: &[&str], flags: u32, body: &[u8]) {
    let mut header = sub(
        b"HEDR",
        &[1.70f32.to_le_bytes(), [0; 4], 0x800u32.to_le_bytes()].concat(),
    );
    for master in masters {
        header.extend(sub(b"MAST", format!("{master}\0").as_bytes()));
        header.extend(sub(b"DATA", &[0; 8]));
    }
    fs::write(
        data.join(name),
        [record(b"TES4", 0, flags, &header), body.to_vec()].concat(),
    )
    .unwrap();
}

fn text(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend((value.len() as u16).to_le_bytes());
    bytes.extend(value);
}

/// Format-two object padding and alias are asymmetric and remain nonidentity scalars.
fn object(bytes: &mut Vec<u8>, word: u32, alias: i16, padding: u16) -> usize {
    bytes.extend(padding.to_le_bytes());
    bytes.extend(alias.to_le_bytes());
    let offset = bytes.len();
    bytes.extend(word.to_le_bytes());
    offset
}

/// The physical sites are authored independently, not recovered by searching equal words.
fn vmad() -> (Vec<u8>, Vec<(usize, u32, Option<&'static str>)>) {
    let mut bytes = [5u16.to_le_bytes(), 2u16.to_le_bytes(), 1u16.to_le_bytes()].concat();
    text(&mut bytes, b"Probe");
    bytes.push(0xA5);
    bytes.extend(2u16.to_le_bytes());
    text(&mut bytes, b"one");
    bytes.extend([1, 0x6D]);
    let first = object(&mut bytes, 0x0000_0810, -7, 0xB2A1);
    text(&mut bytes, b"many");
    bytes.extend([11, 0x3C]);
    bytes.extend(3u32.to_le_bytes());
    let second = object(&mut bytes, 0x0100_0820, 12, 0x9988);
    let third = object(&mut bytes, 0, -2, 0x4433);
    let fourth = object(&mut bytes, 0x0200_9999, 7, 0xCCDD);
    (
        bytes,
        vec![
            (first, 0x0000_0810, Some("000810:Alpha.esm")),
            (second, 0x0100_0820, Some("000820:Zulu.esm")),
            (third, 0, None),
            (fourth, 0x0200_9999, None),
        ],
    )
}

struct Fixture {
    _root: tempfile::TempDir,
    output: PathBuf,
    vmad: Vec<u8>,
    sites: Vec<(usize, u32, Option<&'static str>)>,
}

/// Explicit supplier order Zulu/Alpha/Patch/Light differs from alphabetic owner allocation.
fn fixture(mode: RecordBundleMode) -> Fixture {
    fixture_with_vmad_owner(mode, false)
}

/// Keep the shared scope/order fixtures unchanged; the byte witness uses authored ACTI VMAD.
fn fixture_with_vmad_owner(mode: RecordBundleMode, script_on_acti: bool) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("Data");
    fs::create_dir(&data).unwrap();
    plugin(
        &data,
        "Zulu.esm",
        &[],
        0,
        &[
            record(b"STAT", 0x820, 0, &sub(b"EDID", b"Zulu\0")),
            record(b"KYWD", 0x830, 0, &sub(b"EDID", b"ZuluKeyword\0")),
        ]
        .concat(),
    );
    plugin(
        &data,
        "Alpha.esm",
        &[],
        0,
        &[
            record(b"STAT", 0x810, 0, &sub(b"EDID", b"Alpha\0")),
            record(b"KYWD", 0x830, 0, &sub(b"EDID", b"AlphaKeyword\0")),
        ]
        .concat(),
    );
    let (script, sites) = vmad();
    let mut list_fields = [
        sub(b"EDID", b"Caf\xE9\x80\0"),
        sub(b"LNAM", &0x0000_0810u32.to_le_bytes()),
    ]
    .concat();
    if !script_on_acti {
        list_fields.extend(sub(b"VMAD", &script));
    }
    let mut patch_records = [
        record(b"STAT", 0x0100_0820, 0, &sub(b"EDID", b"ZuluOverride\0")),
        record(b"STAT", 0x0200_0900, 0, &sub(b"EDID", b"Patch\0")),
        record(b"FLST", 0x0200_0901, 0, &list_fields),
    ]
    .concat();
    if script_on_acti {
        patch_records.extend(record(
            b"ACTI",
            0x0200_0902,
            0,
            &[sub(b"EDID", b"Caf\xE9\x80\0"), sub(b"VMAD", &script)].concat(),
        ));
    }
    plugin(
        &data,
        "Patch.esp",
        &["Alpha.esm", "Zulu.esm"],
        0,
        &patch_records,
    );
    plugin(
        &data,
        "Light.esl",
        &[],
        0x200,
        &record(b"STAT", 0x815, 0, &sub(b"EDID", b"Light\0")),
    );
    let list = data.join("plugins.txt");
    fs::write(&list, "*Zulu.esm\n*Alpha.esm\n*Patch.esp\n*Light.esl\n").unwrap();
    let output = root.path().join("bundle");
    let mut signatures = vec![*b"TES4", *b"FLST", *b"KYWD", *b"STAT"];
    if script_on_acti {
        signatures.push(*b"ACTI");
    }
    export_record_bundle_typed_with_options(&data, &list, &output, &signatures, None, mode)
        .unwrap();
    Fixture {
        _root: root,
        output,
        vmad: script,
        sites,
    }
}

fn rows(output: &Path) -> Vec<Value> {
    fs::read_to_string(output.join("typed-records.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn field<'a>(row: &'a Value, tag: &[u8; 4]) -> &'a Value {
    row["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["signature"] == json!(tag))
        .unwrap()
}

fn form_ids(value: &Value, output: &mut Vec<Value>) {
    match value {
        Value::Object(object) => {
            if let Some(form) = object.get("FormId") {
                assert!(
                    form.is_string() || form.is_null(),
                    "semantic numeric FormId: {form}"
                );
                output.push(form.clone());
            }
            for child in object.values() {
                form_ids(child, output);
            }
        }
        Value::Array(array) => {
            for child in array {
                form_ids(child, output);
            }
        }
        _ => {}
    }
}

fn member<'a>(value: &'a Value, name: &str) -> &'a Value {
    &value["Struct"]
        .as_array()
        .unwrap()
        .iter()
        .find(|member| member[0] == name)
        .unwrap()[1]
}

fn read_scope(output: &Path) -> Value {
    let path = output.join("record-bundle-scope.json");
    assert!(
        path.is_file(),
        "typed bundle must publish its bound scope metadata"
    );
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn assert_scope(output: &Path, mode: &str, runtime_views: bool) {
    let db = Connection::open(output.join("skyrim_world.db")).unwrap();
    let identity = PackIdentity::read_sql(&db).unwrap();
    let scope = read_scope(output);
    assert_eq!(scope["schema"], "inhouse-diagnostic-record-bundle-scope-v2");
    assert_eq!(scope["mode"], mode);
    assert_eq!(scope["runtime_ready"], false);
    assert_eq!(scope["runtime_views_materialized"], runtime_views);
    assert_eq!(scope["plugin_table_digest"], identity.digest_hex());
    let sql_version: u32 = db
        .query_row(
            "SELECT version FROM canonical_record_contract WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(scope["canonical_context_version"], sql_version);
    assert_eq!(
        scope["native_source_word_namespace"],
        "supplier-mast-relative-u32"
    );
    assert_eq!(scope["source_context_database"], "skyrim_world.db");
    assert_eq!(
        scope["source_context_tables"],
        json!([
            "pack_identity",
            "pack_plugins",
            "canonical_record_contract",
            "record_context",
            "record_source_fields",
            "record_field_origins",
            "resolved_links",
        ])
    );
    assert_eq!(
        scope["byte_contract"],
        json!({
            "sql_records_data": "rkyv-canonical-subrecords-with-source-relative-links",
            "typed_field_canonical_bytes": "bounded-decoder-canonical-fields-with-source-relative-links",
            "original_payload": "inhouse_source_records.payload",
            "text_values": "decoded-unicode-not-native-byte-arrays",
        })
    );
}

#[test]
fn typed_bundle_orders_headers_then_types_then_original_full_light_owners() {
    let fixture = fixture(RecordBundleMode::Full);
    let rows = rows(&fixture.output);
    assert_eq!(rows.len(), 11);
    assert!(
        rows[..4]
            .iter()
            .all(|row| row["record_type"] == json!(b"TES4"))
    );
    assert_eq!(
        rows[..4]
            .iter()
            .map(|row| row["load_order"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [0, 1, 2, 3]
    );
    assert!(rows[..4].iter().all(|row| row["form_id"].is_null()));
    let actual = rows[4..]
        .iter()
        .map(|row| row["form_id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        actual,
        [
            "000901:Patch.esp",
            "000830:Zulu.esm",
            "000830:Alpha.esm",
            "000820:Zulu.esm",
            "000810:Alpha.esm",
            "000900:Patch.esp",
            "000815:Light.esl",
        ]
    );
}

#[test]
fn nested_typed_ids_are_named_while_native_bytes_aliases_and_text_stay_explicit() {
    let fixture = fixture_with_vmad_owner(RecordBundleMode::RecordArchiveOnly, true);
    let rows = rows(&fixture.output);
    let row = rows
        .iter()
        .find(|row| row["form_id"] == "000902:Patch.esp")
        .unwrap();
    let list_row = rows
        .iter()
        .find(|row| row["form_id"] == "000901:Patch.esp")
        .unwrap();
    let script = field(row, b"VMAD");
    let mut identities = Vec::new();
    form_ids(&script["value"], &mut identities);
    assert_eq!(
        identities,
        vec![
            json!("000810:Alpha.esm"),
            json!("000820:Zulu.esm"),
            Value::Null,
            Value::Null
        ]
    );
    let scripts = member(&script["value"], "scripts")["Array"]
        .as_array()
        .unwrap();
    assert_eq!(member(&scripts[0], "flags")["Unsigned"], 0xA5);
    let properties = member(&scripts[0], "properties")["Array"]
        .as_array()
        .unwrap();
    assert_eq!(member(&properties[0], "flags")["Unsigned"], 0x6D);
    assert_eq!(
        member(member(&properties[0], "value"), "alias")["Signed"],
        -7
    );
    assert_eq!(
        member(member(&properties[0], "value"), "unused")["Unsigned"],
        0xB2A1
    );
    assert_eq!(member(&properties[1], "flags")["Unsigned"], 0x3C);
    let objects = member(&properties[1], "value")["Array"].as_array().unwrap();
    assert_eq!(objects.len(), 3);
    assert_eq!(member(&objects[0], "alias")["Signed"], 12);
    assert_eq!(member(&objects[1], "alias")["Signed"], -2);
    assert_eq!(member(&objects[2], "alias")["Signed"], 7);
    assert_eq!(script["canonical_bytes"], json!(fixture.vmad));
    assert_eq!(
        field(list_row, b"LNAM")["canonical_bytes"],
        json!(0x0000_0810u32.to_le_bytes())
    );
    assert_eq!(field(row, b"EDID")["value"]["String"], "Caf\u{e9}\u{20ac}");
    assert_eq!(
        field(row, b"EDID")["canonical_bytes"],
        json!(b"Caf\xE9\x80\0")
    );
    let db = Connection::open(fixture.output.join("skyrim_world.db")).unwrap();
    let context = RecordContextReader::new(&db).unwrap();
    let key = context
        .identity()
        .table()
        .intern(&"000902:Patch.esp".parse().unwrap())
        .unwrap()
        .unwrap();
    let record = context.record(key).unwrap();
    assert_eq!(record.source_header, 0x0200_0902);
    assert_eq!(
        context
            .identity()
            .table()
            .entry(record.supplier)
            .unwrap()
            .display_name,
        "Patch.esp"
    );
    assert_eq!(record.bytes(*b"EDID"), Some(b"Caf\xE9\x80\0".as_slice()));
    assert_eq!(record.text(*b"EDID").as_deref(), Some("Caf\u{e9}\u{20ac}"));
    assert_eq!(record.bytes(*b"VMAD"), Some(fixture.vmad.as_slice()));
    for &(offset, word, expected) in &fixture.sites {
        let resolved = record.resolve_at(1, offset).unwrap();
        let actual = resolved.map(|key| context.identity().table().named(key).unwrap().to_string());
        assert_eq!(actual.as_deref(), expected);
        let (source, outcome): (u32, String) = db.query_row(
            "SELECT source_word,outcome FROM resolved_links WHERE record_key=?1 AND source_ordinal=1 AND byte_offset=?2",
            (key.to_sql_i64(), offset as u64), |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(source, word);
        assert_eq!(
            outcome,
            if expected.is_some() {
                "resolved"
            } else if word == 0 {
                "null"
            } else {
                "cleared"
            }
        );
    }
    assert_scope(&fixture.output, "record_archive_only", false);
}

#[test]
fn full_typed_bundle_declares_the_same_pack_and_byte_context_as_adjacent_sql() {
    let fixture = fixture(RecordBundleMode::Full);
    assert_scope(&fixture.output, "full", true);
}

fn assert_readers_refuse(output: &Path) {
    let path = output.join("skyrim_world.db");
    let name: NamedFormKey = "000901:Patch.esp".parse().unwrap();
    let history = read_native_chain(&path, &name);
    let diagnostics = read_native_for_form(&path, &name);
    let source = converter::records::history_source::visit_native_source_history(
        output,
        None,
        converter::records::history_source::SourceQuery::Source {
            original_owner: "Patch.esp".into(),
            full_local: 0x901,
        },
        |_| Ok(()),
    );
    assert!(
        history.is_err(),
        "native history accepted an unbound/old bundle scope"
    );
    assert!(
        diagnostics.is_err(),
        "native diagnostics accepted an unbound/old bundle scope"
    );
    assert!(
        source.is_err(),
        "native source history accepted an unbound/old bundle scope"
    );
    assert!(format!("{:#}", history.unwrap_err()).contains("convert again"));
    assert!(format!("{:#}", diagnostics.unwrap_err()).contains("convert again"));
    assert!(format!("{:#}", source.unwrap_err()).contains("convert again"));
}

#[test]
fn native_bundle_readers_refuse_a_foreign_scope_digest() {
    let fixture = fixture(RecordBundleMode::RecordArchiveOnly);
    let path = fixture.output.join("skyrim_world.db");
    let name = "000901:Patch.esp".parse().unwrap();
    assert!(read_native_chain(&path, &name).is_ok());
    assert!(read_native_for_form(&path, &name).is_ok());
    let mut scope = read_scope(&fixture.output);
    scope["plugin_table_digest"] = json!("00".repeat(32));
    fs::write(
        fixture.output.join("record-bundle-scope.json"),
        serde_json::to_vec(&scope).unwrap(),
    )
    .unwrap();
    assert_readers_refuse(&fixture.output);
}

#[test]
fn native_bundle_readers_refuse_old_scope_without_restamping() {
    let fixture = fixture(RecordBundleMode::RecordArchiveOnly);
    let mut scope = read_scope(&fixture.output);
    scope["schema"] = json!("inhouse-diagnostic-record-bundle-scope-v1");
    let bytes = serde_json::to_vec(&scope).unwrap();
    fs::write(fixture.output.join("record-bundle-scope.json"), &bytes).unwrap();
    assert_readers_refuse(&fixture.output);
    assert_eq!(
        fs::read(fixture.output.join("record-bundle-scope.json")).unwrap(),
        bytes
    );
}

#[test]
fn native_bundle_readers_refuse_mismatched_context_version_or_byte_namespace() {
    let fixture = fixture(RecordBundleMode::RecordArchiveOnly);
    let original = read_scope(&fixture.output);
    for (field, value) in [
        ("canonical_context_version", json!(999)),
        (
            "native_source_word_namespace",
            json!("global-runtime-form-id"),
        ),
        (
            "byte_contract",
            json!({"sql_records_data": "original-raw-plugin-payload"}),
        ),
    ] {
        let mut scope = original.clone();
        scope[field] = value;
        fs::write(
            fixture.output.join("record-bundle-scope.json"),
            serde_json::to_vec(&scope).unwrap(),
        )
        .unwrap();
        assert_readers_refuse(&fixture.output);
    }
}

#[test]
fn typed_bundle_cannot_bypass_scope_binding_with_an_empty_metadata_member() {
    let fixture = fixture(RecordBundleMode::Full);
    // The marker is invalid rather than deleted: this preserves all generated evidence.
    fs::write(fixture.output.join("record-bundle-scope.json"), b"{}").unwrap();
    assert_readers_refuse(&fixture.output);
}

#[test]
fn typed_bundle_readers_refuse_missing_scope_while_preserving_original_member() {
    let fixture = fixture(RecordBundleMode::RecordArchiveOnly);
    fs::rename(
        fixture.output.join("record-bundle-scope.json"),
        fixture
            .output
            .join("retained-original-record-bundle-scope.json"),
    )
    .unwrap();
    assert_readers_refuse(&fixture.output);
}
