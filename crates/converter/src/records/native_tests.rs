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
            .any(|warning| warning.contains("broken.esp"))
    );
    assert!(result.diagnostics["broken.esp"].skipped_records >= 1);
    assert!(result.diagnostics["broken.esp"].invalid_links >= 1);
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
                .any(|warning| warning.contains("patch.esp"))
        );
        let failed = result
            .history
            .iter()
            .find(|entry| entry.supplier == 2 && entry.signature == *b"TEST")
            .unwrap();
        assert!(!failed.accepted);
        let reason = failed.reason.as_deref().unwrap();
        assert!(reason.contains("deferred"), "{reason}");
        assert!(reason.contains("form_id"), "{reason}");
    }
}

/// A failed root omits canonical fields while original bytes and neighboring sites stay exact.
#[test]
fn native_deferred_first_definition_preserves_safe_fields_source_and_live_sites() {
    let root = tempfile::tempdir().unwrap();
    let owner = plugin(
        root.path(),
        "Owner.esm",
        &[],
        0,
        &record(b"FACT", 0x880, 0, &sub(b"EDID", b"RankOwner\0")),
    );
    let gap = plugin(root.path(), "Gap.esm", &[], 0, &[]);
    let rank = [0x880u32.to_le_bytes(), 0x1379_2468u32.to_le_bytes()].concat();
    let payload = [
        sub(b"EDID", b"RetainedFirstDefinition\0"),
        sub(b"LINK", &0x880u32.to_le_bytes()),
        sub(b"COED", &rank),
        sub(b"ZRAW", &[0xA7, 0x31, 0xFF, 0x00, 0x58]),
        sub(b"LINK", &0xFF00_9999u32.to_le_bytes()),
    ]
    .concat();
    let broken = plugin(
        root.path(),
        "Broken.esp",
        &["Owner.esm"],
        0,
        &[
            record(b"TEST", 0x0100_0811, 0, &payload),
            record(b"STAT", 0x0100_0812, 0, &sub(b"EDID", b"Neighbor\0")),
        ]
        .concat(),
    );
    let paths = [owner, gap, broken];
    let order = FormKeyOrder::read(&paths).unwrap();
    let result = read_plugins_form_keys(&paths, &order, &schemas()).unwrap();
    let key = order
        .resolve(2, 0x0100_0811, ZeroPolicy::Null)
        .unwrap()
        .unwrap();
    let owner = order.resolve(2, 0x880, ZeroPolicy::Null).unwrap().unwrap();
    let neighbor = order
        .resolve(2, 0x0100_0812, ZeroPolicy::Null)
        .unwrap()
        .unwrap();
    let retained = &result.records[&key];
    assert!(result.records.contains_key(&neighbor));
    assert_eq!(retained.source.form_id, 0x0100_0811);
    assert_eq!(retained.source.load_order, 2);
    assert_eq!(
        retained.source.subrecords,
        decoder::native_source_fields(&payload)
    );
    assert_eq!(retained.decoded.raw_payload, payload);
    assert_eq!(retained.canonical_ordinals, [0, 1, 3, 4]);
    assert_eq!(
        retained
            .canonical
            .subrecords
            .iter()
            .map(|(tag, _)| tag.as_slice())
            .collect::<Vec<_>>(),
        [b"EDID".as_slice(), b"LINK", b"ZRAW", b"LINK"]
    );
    assert_eq!(
        retained.canonical.subrecords[2].1,
        [0xA7, 0x31, 0xFF, 0x00, 0x58]
    );
    assert_eq!(
        retained
            .links
            .iter()
            .map(|site| (site.subrecord, site.source_word, site.key, site.outcome))
            .collect::<Vec<_>>(),
        [
            (1, 0x880, Some(owner), LinkOutcome::Resolved),
            (4, 0xFF00_9999, None, LinkOutcome::Cleared)
        ]
    );
    assert!(!decoder::record_contains_deferred(&retained.decoded));
    assert_eq!(retained.decoded.rejected_fields, [*b"COED"]);
    let diagnostic = &result.diagnostics["broken.esp"];
    assert_eq!(
        (
            diagnostic.skipped_records,
            diagnostic.skipped_fields,
            diagnostic.invalid_links,
            diagnostic.compatibility_headers
        ),
        (0, 1, 1, 0)
    );
    let history = result
        .history
        .iter()
        .find(|entry| entry.supplier == 2 && entry.source_word == 0x0100_0811)
        .unwrap();
    assert!(history.accepted);
    let reason = history.reason.as_deref().unwrap();
    assert!(
        reason.contains("deferred recovery omitted COED[source:2]"),
        "{reason}"
    );
    assert!(reason.contains("i32"), "{reason}");
    let mut typed = Vec::new();
    retained
        .write_typed(&mut typed, key, order.table(), &BTreeMap::new())
        .unwrap();
    let typed = String::from_utf8(typed).unwrap();
    assert!(!typed.contains("Deferred"));
    assert!(typed.contains("RetainedFirstDefinition"));
}

/// Rejection of one repeated deferred root cannot erase an independently valid same-tag root.
#[test]
fn native_deferred_recovery_keeps_independent_same_tag_root_and_physical_sites() {
    let schemas = RuntimeSchemas::from_addon_bytes(&[br#"{
        "version":1,"name":"selective-deferred-probe","records":[
            {"signature":"PRUF","allow_unordered":true,"fields":[
                {"signature":"EDID","name":"editor_id","kind":"zstring"},
                {"signature":"COED","name":"ownership","kind":"struct","size":8,
                 "repeat":true,"reject_record_on_error":true,"members":[
                    {"name":"owner","kind":"form_id","offset":0,"targets":["NPC_","FACT"]},
                    {"name":"dependent","kind":"union","offset":4,"size":4,
                     "decider":"inventory_owner_condition","fields":[
                        {"name":"unused","kind":"bytes","size":4},
                        {"name":"global","kind":"form_id","targets":["GLOB"]}
                     ]}
                 ]}
            ]}
        ]
    }"#])
    .unwrap();
    for bad_first in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let owner = plugin(
            root.path(),
            "Owner.esm",
            &[],
            0,
            &[
                record(b"FACT", 0x880, 0, &sub(b"EDID", b"UnsupportedRankOwner\0")),
                record(b"NPC_", 0x881, 0, &sub(b"EDID", b"SupportedGlobalOwner\0")),
                record(b"GLOB", 0x882, 0, &sub(b"EDID", b"IndependentGlobal\0")),
            ]
            .concat(),
        );
        let gap = plugin(root.path(), "Gap.esm", &[], 0, &[]);
        let bad = [0x880u32.to_le_bytes(), 0x1379_2468u32.to_le_bytes()].concat();
        let good = [0x881u32.to_le_bytes(), 0x882u32.to_le_bytes()].concat();
        let opaque = [0x81, 0x88, 0x00, 0x00, 0x82, 0x88, 0x00, 0x00, 0xA7];
        let payload = [
            sub(b"EDID", b"IndependentDeferredRoots\0"),
            sub(b"COED", if bad_first { &bad } else { &good }),
            sub(b"ZRAW", &opaque),
            sub(b"COED", if bad_first { &good } else { &bad }),
        ]
        .concat();
        let path = plugin(
            root.path(),
            "Patch.esp",
            &["Owner.esm"],
            0,
            &record(b"PRUF", 0x0100_0811, 0, &payload),
        );
        let paths = [owner, gap, path];
        let order = FormKeyOrder::read(&paths).unwrap();
        let result = read_plugins_form_keys(&paths, &order, &schemas).unwrap();
        let key = order
            .resolve(2, 0x0100_0811, ZeroPolicy::Null)
            .unwrap()
            .unwrap();
        let owner = order.resolve(2, 0x881, ZeroPolicy::Null).unwrap().unwrap();
        let global = order.resolve(2, 0x882, ZeroPolicy::Null).unwrap().unwrap();
        let good_position = if bad_first { 3 } else { 1 };
        let bad_position = if bad_first { 1 } else { 3 };
        let retained = &result.records[&key];
        assert_eq!(
            retained.source.subrecords,
            decoder::native_source_fields(&payload)
        );
        assert_eq!(retained.decoded.raw_payload, payload);
        assert_eq!(
            retained.canonical_ordinals,
            if bad_first {
                vec![0, 2, 3]
            } else {
                vec![0, 1, 2]
            }
        );
        let good_index = retained
            .canonical_ordinals
            .iter()
            .position(|&position| position == good_position)
            .unwrap();
        assert_eq!(
            retained.canonical.subrecords[good_index],
            (b"COED".to_vec(), good)
        );
        assert_eq!(
            retained.decoded.fields[good_index].value,
            Value::Struct(vec![
                (
                    "owner".into(),
                    Value::NativeFormKey(order.table().named(owner).unwrap())
                ),
                (
                    "dependent".into(),
                    Value::NativeFormKey(order.table().named(global).unwrap())
                ),
            ])
        );
        let opaque_index = retained
            .canonical_ordinals
            .iter()
            .position(|&position| position == 2)
            .unwrap();
        assert_eq!(
            retained.canonical.subrecords[opaque_index],
            (b"ZRAW".to_vec(), opaque.to_vec())
        );
        assert_eq!(
            retained
                .links
                .iter()
                .map(|site| (
                    site.subrecord,
                    site.byte_offset,
                    site.source_word,
                    site.key,
                    site.outcome
                ))
                .collect::<Vec<_>>(),
            [
                (good_position, 0, 0x881, Some(owner), LinkOutcome::Resolved),
                (good_position, 4, 0x882, Some(global), LinkOutcome::Resolved),
            ]
        );
        assert!(!decoder::record_contains_deferred(&retained.decoded));
        let diagnostics = &result.diagnostics["patch.esp"];
        assert_eq!(
            (
                diagnostics.skipped_records,
                diagnostics.skipped_fields,
                diagnostics.invalid_links
            ),
            (0, 1, 0)
        );
        let history = result
            .history
            .iter()
            .find(|entry| entry.supplier == 2 && entry.source_word == 0x0100_0811)
            .unwrap();
        assert!(history.accepted);
        let reason = history.reason.as_deref().unwrap();
        assert!(
            reason.contains(&format!(
                "deferred recovery omitted COED[source:{bad_position}]"
            )),
            "{reason}"
        );
        assert!(
            !reason.contains(&format!("COED[source:{good_position}]")),
            "{reason}"
        );
        let mut typed = Vec::new();
        retained
            .write_typed(&mut typed, key, order.table(), &BTreeMap::new())
            .unwrap();
        let typed = String::from_utf8(typed).unwrap();
        assert!(!typed.contains("Deferred"));
        assert!(typed.contains(&order.table().named(owner).unwrap().to_string()));
        assert!(typed.contains(&order.table().named(global).unwrap().to_string()));
    }
}

/// GMST's EditorID convention and body-link clearing are separate from header compatibility.
#[test]
fn native_compatibility_counts_only_non_gmst_headers() {
    let root = tempfile::tempdir().unwrap();
    let body = [
        record(
            b"STAT",
            0x0700_0811,
            0,
            &sub(b"EDID", b"CompatibleHeader\0"),
        ),
        record(b"STAT", 0x812, 0, &sub(b"EDID", b"Target\0")),
        record(
            b"TEST",
            0x814,
            0,
            &sub(b"LINK", &0x0800_0812u32.to_le_bytes()),
        ),
        record(
            b"GMST",
            0x0600_0813,
            0,
            &[
                sub(b"EDID", b"fNativeConvention\0"),
                sub(b"DATA", &3.75f32.to_le_bytes()),
            ]
            .concat(),
        ),
    ]
    .concat();
    let paths = [plugin(root.path(), "Scope.esm", &[], 0, &body)];
    let order = FormKeyOrder::read(&paths).unwrap();
    let result = read_plugins_form_keys(&paths, &order, &schemas()).unwrap();
    assert_eq!(result.records.len(), 4);
    let diagnostic = &result.diagnostics["scope.esm"];
    assert_eq!(diagnostic.compatibility_headers, 1);
    assert!(diagnostic.first_by_category["compatibility"].contains("07000811"));
    assert_eq!(
        result
            .notices()
            .iter()
            .filter(|notice| notice.contains("supplier-self runtime compatibility"))
            .count(),
        1
    );
    let gmst = result
        .history
        .iter()
        .find(|entry| entry.signature == *b"GMST")
        .unwrap();
    assert!(gmst.accepted);
    assert_eq!(gmst.source_word, 0x0600_0813);
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

/// Lazy typed history remains available when its owner has no legacy full-plugin slot.
#[test]
fn native_named_history_decodes_beyond_full_slot_limit_and_guards_source_hash() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("Data");
    fs::create_dir(&data).unwrap();
    let mut names = String::new();
    let mut last = PathBuf::new();
    for index in 0..255 {
        let name = format!("Full{index:04}.esm");
        let body = if index == 254 {
            [
                record(b"KYWD", 0x831, 0, &sub(b"EDID", b"NativeTarget\0")),
                record(
                    b"FLST",
                    0x832,
                    0,
                    &[
                        sub(b"EDID", b"NativeHistory\0"),
                        sub(b"LNAM", &0x831u32.to_le_bytes()),
                    ]
                    .concat(),
                ),
            ]
            .concat()
        } else {
            Vec::new()
        };
        last = plugin(&data, &name, &[], 0, &body);
        names.push_str(&format!("*{name}\n"));
    }
    let list = root.path().join("plugins.txt");
    fs::write(&list, names).unwrap();
    let pack = root.path().join("pack");
    crate::esm::inhouse::export_record_bundle_typed(&data, &list, &pack, &[*b"FLST"]).unwrap();
    let named = "000832:Full0254.esm".parse().unwrap();
    let conn = rusqlite::Connection::open(pack.join("skyrim_world.db")).unwrap();
    let identity = shared::pack_identity::PackIdentity::read_sql(&conn).unwrap();
    let key = identity.table().intern(&named).unwrap().unwrap();
    assert!(
        identity.runtime_id(key).is_err(),
        "this proof requires an unrepresentable owner"
    );
    drop(conn);
    let report = crate::records::history::read_named_history(&pack, Some(&data), &named).unwrap();
    assert_eq!(report.form_id.to_string(), "000832:Full0254.esm");
    assert_eq!(report.versions.len(), 1);
    let version = &report.versions[0];
    assert_eq!(version["decode_status"], "decoded", "{report:?}");
    assert_eq!(version["record"]["form_id"], "000832:Full0254.esm");
    let link = version["record"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["signature"] == serde_json::json!([76, 78, 65, 77]))
        .unwrap();
    assert!(
        serde_json::to_string(&link["value"])
            .unwrap()
            .contains("000831:Full0254.esm")
    );
    assert_eq!(
        link["canonical_bytes"],
        serde_json::json!(0x831u32.to_le_bytes())
    );
    assert_eq!(version["diagnostics"]["invalid_links"], 0);
    let mut changed = fs::read(&last).unwrap();
    *changed.last_mut().unwrap() ^= 1;
    fs::write(&last, changed).unwrap();
    let refused = crate::records::history::read_named_history(&pack, Some(&data), &named).unwrap();
    assert_eq!(refused.versions.len(), 1);
    assert_eq!(refused.versions[0]["decode_status"], "undecodable");
    assert!(
        refused.versions[0]["decode_reason"]
            .as_str()
            .unwrap()
            .contains("source changed")
    );
    assert!(refused.versions[0]["record"].is_null());
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
