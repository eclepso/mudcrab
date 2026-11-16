//! Generated native typed fixtures distinguish source ownership, byte sites and runtime slots.
use super::*;
use std::{fs, path::Path};

/// Frame a physical source field independently of typed decoding.
fn sub(tag: &[u8; 4], bytes: &[u8]) -> Vec<u8> {
    [tag.as_slice(), &(bytes.len() as u16).to_le_bytes(), bytes].concat()
}

/// Keep header metadata sentinels and source words separate from semantic keys.
fn record(tag: &[u8; 4], word: u32, flags: u32, bytes: &[u8]) -> Vec<u8> {
    [
        tag.as_slice(),
        &(bytes.len() as u32).to_le_bytes(),
        &flags.to_le_bytes(),
        &word.to_le_bytes(),
        &0x1357_2468u32.to_le_bytes(),
        &44u16.to_le_bytes(),
        &0xA571u16.to_le_bytes(),
        bytes,
    ]
    .concat()
}

/// Write only generated synthetic source data while each test is running.
fn plugin(root: &Path, name: &str, masters: &[&str], flags: u32, body: &[u8]) -> PathBuf {
    let mut payload = sub(
        b"HEDR",
        &[1.70f32.to_le_bytes(), [0; 4], 0x800u32.to_le_bytes()].concat(),
    );
    for master in masters {
        payload.extend(sub(b"MAST", format!("{master}\0").as_bytes()));
        payload.extend(sub(b"DATA", &[0; 8]));
    }
    let path = root.join(name);
    fs::write(
        &path,
        [record(b"TES4", 0, flags, &payload), body.to_vec()].concat(),
    )
    .unwrap();
    path
}

/// Independently authored layouts exercise generic members, arrays and deferred selectors.
fn schemas() -> RuntimeSchemas {
    RuntimeSchemas::from_addon_bytes(&[br#"{"version":1,"name":"native-probe","records":[
        {"signature":"TEST","allow_unordered":true,"fields":[
            {"signature":"EDID","name":"editor_id","kind":"zstring"},
            {"signature":"LINK","name":"link","kind":"form_id","repeat":true,"targets":["*"]},
            {"signature":"STRC","name":"members","kind":"struct","size":12,"members":[
                {"name":"padding","kind":"u32","offset":0},
                {"name":"first","kind":"form_id","offset":4,"targets":["*"]},
                {"name":"second","kind":"form_id","offset":8,"targets":["*"]}]},
            {"signature":"ARRY","name":"array","kind":"array","count":{"kind":"prefix_u32","stride":4},"members":[{"kind":"form_id","targets":["*"]}]},
            {"signature":"VMAD","name":"scripts","definition":"vmad"},
            {"signature":"REQD","name":"required","kind":"form_id","reject_record_on_error":true,"targets":["*"]},
            {"signature":"COED","name":"ownership","kind":"struct","size":8,"reject_record_on_error":true,"members":[
                {"name":"owner","kind":"form_id","offset":0,"targets":["NPC_","FACT"]},
                {"name":"dependent","kind":"union","offset":4,"size":4,"decider":"inventory_owner_condition","fields":[{"name":"unused","kind":"bytes","size":4}]}]}
        ]}
    ]}"#]).unwrap()
}

/// A valid scalar object preserves nonidentity alias and padding words.
fn vmad(word: u32) -> Vec<u8> {
    [
        5u16.to_le_bytes().as_slice(),
        &2u16.to_le_bytes(),
        &1u16.to_le_bytes(),
        &1u16.to_le_bytes(),
        b"S",
        &[0],
        &1u16.to_le_bytes(),
        &1u16.to_le_bytes(),
        b"P",
        &[1, 1],
        &[0xA1, 0xB2, 7, 0],
        &word.to_le_bytes(),
    ]
    .concat()
}

/// Allocation, supplier priority and original MAST selectors are three different orders.
#[test]
fn native_sites_name_owner_keys_and_retain_original_physical_offsets() {
    let root = tempfile::tempdir().unwrap();
    let zulu = plugin(
        root.path(),
        "Zulu.esm",
        &[],
        0,
        &record(b"TEST", 0x822, 0, &[]),
    );
    let alpha = plugin(
        root.path(),
        "Alpha.esm",
        &[],
        0,
        &record(b"STAT", 0x823, 0, &[]),
    );
    let word = 0x0000_0823u32;
    let script = vmad(word);
    let payload = [
        sub(b"EDID", b"Physical\0"),
        sub(b"LINK", &word.to_le_bytes()),
        sub(b"ZZZZ", &word.to_le_bytes()),
        sub(b"LINK", &[1, 2, 3]),
        sub(b"LINK", &word.to_le_bytes()),
        sub(
            b"STRC",
            &[word.to_le_bytes(), word.to_le_bytes(), word.to_le_bytes()].concat(),
        ),
        sub(
            b"ARRY",
            &[2u32.to_le_bytes(), word.to_le_bytes(), word.to_le_bytes()].concat(),
        ),
        sub(b"VMAD", &script),
    ]
    .concat();
    let patch = plugin(
        root.path(),
        "Patch.esp",
        &["Alpha.esm", "Zulu.esm"],
        0,
        &record(b"TEST", 0x0100_0822, 0, &payload),
    );
    let paths = [zulu, alpha, patch];
    let order = FormKeyOrder::read(&paths).unwrap();
    let result = read_plugins_form_keys(&paths, &order, &schemas()).unwrap();
    let key = FormKey::from_packed(0x0300_0822).unwrap();
    let record = &result.records[&key];
    assert_eq!(record.source.form_id, 0x0100_0822);
    assert_eq!(record.source.load_order, 2);
    assert_eq!(record.canonical_ordinals, [0, 1, 2, 4, 5, 6, 7]);
    assert_eq!(
        record
            .links
            .iter()
            .map(|site| (
                site.subrecord,
                site.byte_offset,
                site.key.unwrap().to_packed()
            ))
            .collect::<Vec<_>>(),
        [
            (1, 0, 0x0100_0823),
            (4, 0, 0x0100_0823),
            (5, 4, 0x0100_0823),
            (5, 8, 0x0100_0823),
            (6, 4, 0x0100_0823),
            (6, 8, 0x0100_0823),
            (7, 21, 0x0100_0823)
        ]
    );
    assert_eq!(record.source.subrecords[3].1, [1, 2, 3]);
    assert_eq!(record.canonical.subrecords[6].1, script);
    assert_eq!(record.decoded.version_control, 0x1357_2468);
    let named = serde_json::to_string(&record.decoded.fields).unwrap();
    assert!(named.contains("000823:Alpha.esm"), "{named}");
    assert!(!record.links.iter().any(|site| site.subrecord == 2));
}

/// One broken required candidate and one malformed optional field preserve usable neighbors.
#[test]
fn native_broken_items_recover_locally_and_persist_loss_diagnostics() {
    let root = tempfile::tempdir().unwrap();
    let body = [
        record(b"STAT", 0x820, 0, &[]),
        record(
            b"TEST",
            0x821,
            0,
            &[
                sub(b"LINK", &[1, 2, 3]),
                sub(b"LINK", &0xFF00_0820u32.to_le_bytes()),
            ]
            .concat(),
        ),
        record(b"TEST", 0x822, 0, &sub(b"REQD", &[9, 8, 7])),
        record(
            b"TEST",
            0x823,
            0,
            &sub(b"LINK", &0xFF00_9999u32.to_le_bytes()),
        ),
        record(b"STAT", 0x824, 0, &[]),
    ]
    .concat();
    let paths = [plugin(root.path(), "Broken.esp", &[], 0, &body)];
    let order = FormKeyOrder::read(&paths).unwrap();
    let result = read_plugins_form_keys(&paths, &order, &schemas()).unwrap();
    let get = |local: u64| FormKey::from_packed(0x0100_0000 | local).unwrap();
    assert!(result.records.contains_key(&get(0x820)));
    assert!(result.records.contains_key(&get(0x824)));
    assert!(!result.records.contains_key(&get(0x822)));
    let healthy = &result.records[&get(0x821)];
    assert_eq!(healthy.canonical_ordinals, [1]);
    assert_eq!(healthy.links[0].key, Some(get(0x820)));
    let missing = &result.records[&get(0x823)];
    assert_eq!(missing.links[0].outcome, LinkOutcome::Cleared);
    assert_eq!(
        missing.canonical.subrecords[0].1,
        0xFF00_9999u32.to_le_bytes()
    );
    assert!(
        result
            .warnings()
            .iter()
            .any(|warning| warning.contains("Broken.esp"))
    );
    assert!(result.diagnostics["Broken.esp"].skipped_records >= 1);
    assert!(result.diagnostics["Broken.esp"].invalid_links >= 1);
}

/// A catalogue-rejected deferred override exposes its predecessor unless a tombstone intervened.
#[test]
fn native_deferred_replay_retains_predecessors_and_respects_deletion_barriers() {
    for deletion in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let base = plugin(
            root.path(),
            "Base.esm",
            &[],
            0,
            &record(b"TEST", 0x811, 0, &sub(b"EDID", b"Earlier\0")),
        );
        let deleted = if deletion {
            record(b"TEST", 0x0000_0811, 0x20, &[])
        } else {
            Vec::new()
        };
        let middle = plugin(root.path(), "Middle.esp", &["Base.esm"], 0, &deleted);
        let body = [
            record(b"NPC_", 0x0100_0900, 0, &sub(b"EDID", b"Forward\0")),
            record(
                b"TEST",
                0x0000_0811,
                0,
                &sub(
                    b"COED",
                    &[0x0100_0900u32.to_le_bytes(), 7u32.to_le_bytes()].concat(),
                ),
            ),
        ]
        .concat();
        let patch = plugin(root.path(), "Patch.esp", &["Base.esm"], 0, &body);
        let paths = [base, middle, patch];
        let order = FormKeyOrder::read(&paths).unwrap();
        let result = read_plugins_form_keys(&paths, &order, &schemas()).unwrap();
        let key = FormKey::from_packed(0x0100_0811).unwrap();
        assert_eq!(result.records.contains_key(&key), !deletion);
        if !deletion {
            assert_eq!(result.records[&key].source.load_order, 0);
        }
        assert!(
            result
                .warnings()
                .iter()
                .any(|warning| warning.contains("Patch.esp"))
        );
    }
}

/// Native low and wide light locals never pass through a legacy width/header check.
#[test]
fn native_light_locals_preserve_low_old_header_and_wide_values() {
    let root = tempfile::tempdir().unwrap();
    let body = [
        record(b"STAT", 0x123, 0, &[]),
        record(b"STAT", 0x1234, 0, &[]),
        record(
            b"TEST",
            0x811,
            0,
            &[
                sub(b"LINK", &0x123u32.to_le_bytes()),
                sub(b"LINK", &0x1234u32.to_le_bytes()),
            ]
            .concat(),
        ),
    ]
    .concat();
    let paths = [plugin(root.path(), "Light.esl", &[], 0x200, &body)];
    let order = FormKeyOrder::read(&paths).unwrap();
    let result = read_plugins_form_keys(&paths, &order, &schemas()).unwrap();
    let row = &result.records[&FormKey::from_packed(0x0100_0811).unwrap()];
    assert_eq!(
        row.links
            .iter()
            .map(|site| site.key.unwrap().to_packed())
            .collect::<Vec<_>>(),
        [0x0100_0123, 0x0100_1234]
    );
}

/// GMST aliases retain the first identity through deletions and a zero-header restoration.
#[test]
fn native_gmst_editor_identity_aliases_and_zero_header_restore_are_stable() {
    let root = tempfile::tempdir().unwrap();
    let setting = |word: u32, number: f32| {
        record(
            b"GMST",
            word,
            0,
            &[
                sub(b"EDID", b"fNativeProbe\0"),
                sub(b"DATA", &number.to_le_bytes()),
            ]
            .concat(),
        )
    };
    let base = plugin(root.path(), "Base.esm", &[], 0, &setting(0x811, 1.25));
    let patch = plugin(
        root.path(),
        "Patch.esp",
        &["Base.esm"],
        0,
        &setting(0x0100_0821, 2.5),
    );
    let deleted = plugin(
        root.path(),
        "Removed.esp",
        &["Base.esm", "Patch.esp"],
        0,
        &record(b"GMST", 0x0100_0821, 0x20, &[]),
    );
    let restored = plugin(
        root.path(),
        "Restored.esp",
        &["Base.esm"],
        0,
        &setting(0, 3.75),
    );
    let paths = [base, patch, deleted, restored];
    let order = FormKeyOrder::read(&paths).unwrap();
    let result = read_plugins_form_keys(
        &paths,
        &order,
        &RuntimeSchemas::from_addon_bytes(&[]).unwrap(),
    )
    .unwrap();
    let key = FormKey::from_packed(0x0100_0811).unwrap();
    assert_eq!(result.records.len(), 1);
    assert_eq!(result.records[&key].source.form_id, 0);
    assert_eq!(result.records[&key].source.load_order, 3);
    assert_eq!(
        result.records[&key]
            .decoded
            .fields
            .iter()
            .find(|field| field.signature == *b"DATA")
            .unwrap()
            .value,
        Value::Float(3.75)
    );
    let accepted: Vec<_> = result
        .history
        .iter()
        .filter(|entry| entry.signature == *b"GMST")
        .map(|entry| (entry.key, entry.accepted))
        .collect();
    assert_eq!(accepted, [(Some(key), true); 4]);
}

/// A projector rejects one candidate locally before it claims the earlier usable key.
#[test]
fn native_admission_callback_retains_predecessor_and_valid_neighbor() {
    let root = tempfile::tempdir().unwrap();
    let base = plugin(
        root.path(),
        "Base.esm",
        &[],
        0,
        &record(b"TEST", 0x811, 0, &sub(b"EDID", b"Healthy\0")),
    );
    let body = [
        record(b"TEST", 0x0000_0811, 0, &sub(b"EDID", b"RejectMe\0")),
        record(b"STAT", 0x0100_0824, 0, &[]),
    ]
    .concat();
    let patch = plugin(root.path(), "Patch.esp", &["Base.esm"], 0, &body);
    let paths = [base, patch];
    let order = FormKeyOrder::read(&paths).unwrap();
    let result = read_plugins_form_keys_with_validation(&paths, &order, &schemas(), |record| {
        if record
            .decoded
            .fields
            .iter()
            .any(|field| field.value == Value::String("RejectMe".into()))
        {
            Err("unusable scalar projection".into())
        } else {
            Ok(())
        }
    })
    .unwrap();
    let key = FormKey::from_packed(0x0100_0811).unwrap();
    assert_eq!(result.records[&key].source.load_order, 0);
    assert!(
        result
            .records
            .contains_key(&FormKey::from_packed(0x0200_0824).unwrap())
    );
    assert!(result.history.iter().any(|entry| entry.source_word == 0x811
        && !entry.accepted
        && entry.reason.as_deref() == Some("unusable scalar projection")));
}

/// Native history/diagnostics retain scalar source facts and exact high semantic keys together.
#[test]
fn native_provenance_sql_round_trips_physical_metadata_and_named_diagnostic_sources() {
    let root = tempfile::tempdir().unwrap();
    let mut paths = Vec::new();
    for index in 0..256 {
        let body = if index == 255 {
            record(
                b"TEST",
                0x811,
                0,
                &sub(b"LINK", &0xFF00_9999u32.to_le_bytes()),
            )
        } else {
            Vec::new()
        };
        paths.push(plugin(
            root.path(),
            &format!("Owner{index:03}.esm"),
            &[],
            0,
            &body,
        ));
    }
    let order = FormKeyOrder::read(&paths).unwrap();
    let schemas = schemas();
    let result = read_plugins_form_keys(&paths, &order, &schemas).unwrap();
    let identity = order.pack_identity().unwrap();
    let database = root.path().join("native.db");
    let mut connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch("CREATE TABLE schema_info(version INTEGER NOT NULL)")
        .unwrap();
    connection
        .execute(
            "INSERT INTO schema_info VALUES(?1)",
            [shared::WORLD_DATABASE_SCHEMA_VERSION],
        )
        .unwrap();
    let tx = connection.transaction().unwrap();
    shared::records::create_record_context(&tx, &identity).unwrap();
    super::super::history::write_native_tables(&tx, &result, &identity, &paths, &schemas).unwrap();
    super::super::diagnostics::write_native_tables(&tx, &result, &identity).unwrap();
    tx.commit().unwrap();
    let key: i64 = connection
        .query_row(
            "SELECT form_id FROM record_overrides WHERE record_type='TEST'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(key, 0x0000_0001_0000_0811);
    let header: (u32,u32,bool,Vec<u8>) = connection.query_row("SELECT source_form_id,flags,frame_complete,record_signature FROM record_overrides WHERE record_type='TEST'",[],|row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).unwrap();
    assert_eq!(header, (0x811, 0, true, b"TEST".to_vec()));
    let diagnostic: (i64,u32,String,String,String) = connection.query_row("SELECT form_id,link_value,link_value_kind,source_link_identity,link_outcome FROM record_diagnostics WHERE category='link'",[],|row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).unwrap();
    assert_eq!(
        diagnostic,
        (
            0x0000_0001_0000_0811,
            0xFF00_9999,
            "source_form_id".into(),
            "009999:Owner255.esm".into(),
            "cleared".into()
        )
    );
    connection.execute_batch("CREATE TABLE inhouse_source_records(form_id INTEGER,load_order INTEGER,source_form_id INTEGER,record_type TEXT,flags INTEGER); CREATE TABLE records(form_id INTEGER);
        INSERT INTO inhouse_source_records VALUES(4294969361,255,2065,'TEST',0); INSERT INTO records VALUES(4294969361);").unwrap();
    drop(connection);
    let summary = super::super::history_source::visit_native_source_history(
        &database,
        None,
        super::super::history_source::SourceQuery::Source {
            original_owner: "Owner255.esm".into(),
            full_local: 0x811,
        },
        |version| {
            assert_eq!(
                version.form_id.as_ref().map(ToString::to_string).as_deref(),
                Some("000811:Owner255.esm")
            );
            assert_eq!(version.runtime_form_id, None);
            assert!(version.accepted_source_winner && version.runtime_record_present);
            let wire = serde_json::to_value(&version)?;
            assert_eq!(wire["form_id"], "000811:Owner255.esm");
            assert!(wire.get("runtime_form_id").is_none());
            Ok(())
        },
    )
    .unwrap();
    assert!(summary.fully_raw_available);
    let named = NamedFormKey::new("Owner255.esm", 0x811).unwrap();
    let chain = super::super::history::read_native_chain(&database, &named).unwrap();
    assert_eq!(chain.form_id.to_string(), "000811:Owner255.esm");
    assert_eq!(chain.versions.len(), 1);
    assert!(chain.versions[0].winner);
    let events = super::super::diagnostics::read_native_for_form(&database, &named).unwrap();
    assert_eq!(events.form_id, named);
    assert!(events.events.iter().any(|event| {
        event.link_outcome.as_deref() == Some("cleared")
            && event
                .source_link_identity
                .as_ref()
                .map(ToString::to_string)
                .as_deref()
                == Some("009999:Owner255.esm")
    }));
}

/// Generated files exceed both legacy slot ceilings in one admitted typed source set.
#[test]
fn native_typed_storage_retains_255_full_and_4097_light_owners() {
    let root = tempfile::tempdir().unwrap();
    let mut paths = Vec::new();
    for (count, flags, prefix, extension) in
        [(255, 0, "Full", "esm"), (4097, 0x200, "Light", "esl")]
    {
        for index in 0..count {
            paths.push(plugin(
                root.path(),
                &format!("{prefix}{index:04}.{extension}"),
                &[],
                flags,
                &record(b"STAT", 0xABC, 0, &[]),
            ));
        }
    }
    let order = FormKeyOrder::read(&paths).unwrap();
    let result = read_plugins_form_keys(
        &paths,
        &order,
        &RuntimeSchemas::from_addon_bytes(&[]).unwrap(),
    )
    .unwrap();
    assert_eq!(result.records.len(), 4352);
    let key = FormKey::from_packed(73_014_446_780).unwrap();
    assert!(result.records.contains_key(&key));
    assert_eq!(
        order.table().named(key).unwrap().to_string(),
        "000ABC:Light4096.esl"
    );
    let mut output = Vec::new();
    result.records[&key]
        .write_typed(&mut output, key, order.table(), &BTreeMap::new())
        .unwrap();
    let portable: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(portable["form_id"], "000ABC:Light4096.esl");
}

#[test]
fn native_notices_do_not_filter_on_plugin_filename_text() {
    let mut result = NativeReadResult::default();
    let mut diagnostic = PluginDiagnostics::default();
    diagnostic.compatibility_headers = 1;
    diagnostic
        .first_by_category
        .insert("compatibility".into(), "source header".into());
    result
        .diagnostics
        .insert("cleared owner.esp".into(), diagnostic);
    assert_eq!(result.notices().len(), 1);
}

/// Retain the scoped NAVM publication policy while native bytes and other self links stay exact.
#[test]
fn native_navm_onam_keeps_scoped_null_projection_and_valid_neighbor() {
    let root = tempfile::tempdir().unwrap();
    let words = [0x3f00_003fu32.to_le_bytes(), 0x3f00_0810u32.to_le_bytes()].concat();
    let body = [
        record(
            b"NAVM",
            0x800,
            0,
            &[sub(b"EDID", b"Mesh\0"), sub(b"ONAM", &words)].concat(),
        ),
        record(b"TEST", 0x810, 0, &sub(b"EDID", b"Target\0")),
        record(
            b"TEST",
            0x811,
            0,
            &[
                sub(b"EDID", b"Neighbor\0"),
                sub(b"LINK", &0x3f00_0810u32.to_le_bytes()),
            ]
            .concat(),
        ),
    ]
    .concat();
    let paths = vec![plugin(root.path(), "NavmSource.esm", &[], 0, &body)];
    let order = crate::esm::form_keys::FormKeyOrder::read(&paths).unwrap();
    let result = read_plugins_form_keys(&paths, &order, &schemas()).unwrap();
    let mesh = result
        .records
        .values()
        .find(|record| record.source.record_type == *b"NAVM")
        .unwrap();
    assert_eq!(mesh.links.len(), 2);
    assert!(mesh.links.iter().all(|link| link.key.is_none()
        && link.outcome == crate::esm::form_key_merge::LinkOutcome::Cleared));
    assert_eq!(
        mesh.canonical
            .subrecords
            .iter()
            .find(|(tag, _)| tag == b"ONAM")
            .unwrap()
            .1,
        words
    );
    let neighbor = result
        .records
        .values()
        .find(|record| record.source.form_id == 0x811)
        .unwrap();
    assert_eq!(neighbor.links.len(), 1);
    assert_eq!(neighbor.links[0].key.unwrap().local(), 0x810);
    assert!(
        result
            .warnings()
            .iter()
            .any(|line| line.to_ascii_lowercase().contains("navmsource.esm"))
    );
}
