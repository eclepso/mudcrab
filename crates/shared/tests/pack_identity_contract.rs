//! Synthetic persistence fixtures; no installed game files or checked-in binary fixtures.
use rusqlite::Connection;
use shared::{
    form_key::{FormKey, NamedFormKey, PluginId},
    pack_identity::PackIdentity,
    plugin_table::{PluginFacts, PluginFlags, PluginTable},
};

/// Allocation order is alphabetical, supplier priority deliberately is not.
fn identity(proof: u8) -> PackIdentity {
    let table = PluginTable::fresh(vec![
        (
            "Zulu.esm".into(),
            PluginFacts {
                flags: PluginFlags {
                    master: true,
                    ..Default::default()
                },
                content_hash: Some([proof; 32]),
                header_version: Some(1.71),
                ..Default::default()
            },
        ),
        (
            "Alpha.esl".into(),
            PluginFacts {
                flags: PluginFlags {
                    light: true,
                    ..Default::default()
                },
                masters: vec!["Zulu.esm".into(), "malformed/source-name".into()],
                header_version: Some(f32::from_bits(0x7fc0_0001)),
                content_hash: Some([9; 32]),
            },
        ),
    ])
    .unwrap();
    let order = vec![
        table.id("Zulu.esm").unwrap(),
        table.id("Alpha.esl").unwrap(),
    ];
    PackIdentity::new(table, order).unwrap()
}

/// SQL restores literal owner IDs, exact native facts, tombstones and supplier order.
#[test]
fn sql_round_trip_retains_all_facts_and_validates_keys() {
    let expected = identity(7);
    let mut connection = Connection::open_in_memory().unwrap();
    let transaction = connection.transaction().unwrap();
    expected.write_sql(&transaction).unwrap();
    transaction.commit().unwrap();
    let actual = PackIdentity::read_sql(&connection).unwrap();
    assert_eq!(actual.digest(), expected.digest());
    // Independently encoded by the Python reference from literal fixture facts.
    assert_eq!(
        actual.digest(),
        [
            0xa7, 0x02, 0x83, 0x7a, 0x74, 0xd7, 0x49, 0x4b, 0x0b, 0x58, 0xfb, 0xc3, 0x81, 0xfc,
            0x3b, 0x52, 0x6a, 0x5f, 0x14, 0xbf, 0x49, 0xe2, 0xe3, 0x45, 0x1d, 0x6d, 0xa4, 0x84,
            0x50, 0xf9, 0xe2, 0xf1,
        ]
    );
    assert_eq!(
        actual.order().iter().map(|id| id.get()).collect::<Vec<_>>(),
        [2, 1]
    );
    let alpha = &actual.table().entries()[0];
    assert_eq!(alpha.display_name, "Alpha.esl");
    assert_eq!(alpha.facts.header_version.unwrap().to_bits(), 0x7fc0_0001);
    assert_eq!(alpha.facts.masters, ["Zulu.esm", "malformed/source-name"]);
    assert_eq!(actual.required_key(0x0100_1234).unwrap().local(), 0x1234);
    assert_eq!(
        actual
            .table()
            .named(actual.required_key(0x0200_1234).unwrap())
            .unwrap()
            .to_string(),
        "001234:Zulu.esm"
    );
    assert!(actual.required_key(0x0300_1234).is_err());
    assert!(actual.required_key(0x0100_0000_0000_0000).is_err());
    assert!(actual.required_key(-1).is_err());
    assert!(actual.optional_key(Some(0)).is_err());
    assert_eq!(actual.optional_key(None).unwrap(), None);
    assert_eq!(actual.required_key(0x0100_0000).unwrap().local(), 0);
}

/// Editing even a source proof or priority makes a persisted table unusable.
#[test]
fn sql_reader_refuses_tampered_or_old_tables() {
    let mut connection = Connection::open_in_memory().unwrap();
    let old = PackIdentity::read_sql(&connection).unwrap_err().to_string();
    assert!(old.contains("out of date") && old.contains("converted again"));
    let transaction = connection.transaction().unwrap();
    identity(7).write_sql(&transaction).unwrap();
    transaction.commit().unwrap();
    for sql in [
        "UPDATE pack_plugins SET content_hash=zeroblob(32) WHERE id=2",
        "UPDATE pack_plugins SET display_name='Changed.esm',normalized_name='changed.esm' WHERE id=2",
        "UPDATE pack_plugins SET header_version_bits=1 WHERE id=2",
        "UPDATE pack_plugins SET masters_json='[]' WHERE id=1",
        "UPDATE pack_plugins SET loaded=0 WHERE id=2",
        "UPDATE pack_plugins SET supplier_priority=7 WHERE id=2",
        "UPDATE pack_identity SET version=0",
    ] {
        connection.execute_batch("SAVEPOINT mutation").unwrap();
        connection.execute_batch(sql).unwrap();
        assert!(
            PackIdentity::read_sql(&connection).is_err(),
            "accepted {sql}"
        );
        connection
            .execute_batch("ROLLBACK TO mutation; RELEASE mutation")
            .unwrap();
        assert!(PackIdentity::read_sql(&connection).is_ok());
    }
}

/// A reused DB cannot be relabelled; caller transaction rollback preserves every table.
#[test]
fn write_refuses_relabel_and_snapshot_failure_rolls_back() {
    let mut connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "CREATE TABLE caller_owned(value TEXT); INSERT INTO caller_owned VALUES ('keep')",
        )
        .unwrap();
    {
        let transaction = connection.transaction().unwrap();
        identity(7).write_sql(&transaction).unwrap();
        // Simulated later snapshot error: dropping the transaction rolls back the registry too.
    }
    assert!(PackIdentity::read_sql(&connection).is_err());
    let transaction = connection.transaction().unwrap();
    identity(7).write_sql(&transaction).unwrap();
    transaction.commit().unwrap();
    let transaction = connection.transaction().unwrap();
    identity(7).write_sql(&transaction).unwrap();
    assert!(identity(8).write_sql(&transaction).is_err());
    transaction.commit().unwrap();
    assert_eq!(
        PackIdentity::read_sql(&connection).unwrap().digest(),
        identity(7).digest()
    );
    assert_eq!(
        connection
            .query_row("SELECT value FROM caller_owned", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "keep"
    );
}

/// Adding a table proof to already-populated legacy rows would falsely authenticate their IDs.
#[test]
fn old_rows_cannot_be_stamped_as_new_identities() {
    let mut connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("CREATE TABLE records(form_id INTEGER PRIMARY KEY, data BLOB); INSERT INTO records VALUES (4660,x'11223344')").unwrap();
    let transaction = connection.transaction().unwrap();
    let error = identity(7).write_sql(&transaction).unwrap_err().to_string();
    assert!(error.contains("out of date") && error.contains("converted again"));
    transaction.commit().unwrap();
    assert!(PackIdentity::read_sql(&connection).is_err());
    assert_eq!(
        connection
            .query_row(
                "SELECT hex(data) FROM records WHERE form_id=4660",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "11223344"
    );
}

/// A fixed envelope rejects old roots, wrong formats, cross-pack integers and altered bytes.
#[test]
fn envelope_round_trip_checks_discriminator_before_payload() {
    let scope = identity(7);
    let bytes = scope.envelope(2, 4, &[1, 2, 3, 4, 5]);
    assert_eq!(
        &bytes[..24],
        b"MCFKPACK\x02\0\0\0\x04\0\0\0\x01\0\0\0\0\0\0\0"
    );
    assert_eq!(&bytes[56..64], &[5, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(scope.payload(&bytes, 2, 4).unwrap(), [1, 2, 3, 4, 5]);
    assert!(scope.payload(&bytes, 1, 4).is_err());
    assert!(scope.payload(&bytes, 2, 3).is_err());
    assert!(identity(8).payload(&bytes, 2, 4).is_err());
    for length in 0..bytes.len() {
        assert!(scope.payload(&bytes[..length], 2, 4).is_err());
    }
    for offset in 0..bytes.len() {
        let mut altered = bytes.clone();
        altered[offset] ^= 1;
        assert!(
            scope.payload(&altered, 2, 4).is_err(),
            "accepted byte {offset}"
        );
    }
    let mut appended = bytes.clone();
    appended.push(0);
    assert!(scope.payload(&appended, 2, 4).is_err());
    let error = scope.payload(&[0; 128], 2, 4).unwrap_err().to_string();
    assert!(error.contains("out of date") && error.contains("converted again"));
}

/// Portable identities never encode pack integers, including values outside JSON precision.
#[test]
fn named_json_is_text_or_null_and_never_a_number() {
    let key = FormKey::new(PluginId::new(u32::MAX).unwrap(), 0x123456).unwrap();
    assert_eq!(key.to_packed(), 0x00ff_ffff_ff12_3456);
    let named = NamedFormKey::new("Zulu.esm", key.local()).unwrap();
    let json = serde_json::to_string(&named).unwrap();
    assert_eq!(json, "\"123456:Zulu.esm\"");
    assert_eq!(serde_json::from_str::<NamedFormKey>(&json).unwrap(), named);
    assert_eq!(serde_json::to_string(&NamedFormKey::Null).unwrap(), "null");
    assert_eq!(
        serde_json::from_str::<NamedFormKey>("null").unwrap(),
        NamedFormKey::Null
    );
    for rejected in [
        "123456",
        "72057594022302720",
        "0",
        "false",
        "{}",
        "\"123456\"",
    ] {
        assert!(serde_json::from_str::<NamedFormKey>(rejected).is_err());
    }
}

/// Loaded owners need byte proofs; retained tombstones remain addressable but have no priority.
#[test]
fn missing_proofs_and_incomplete_orders_are_rejected_without_allocation_changes() {
    let table = PluginTable::fresh(vec![("Zulu.esm".into(), PluginFacts::default())]).unwrap();
    assert!(PackIdentity::new(table.clone(), vec![table.id("Zulu.esm").unwrap()]).is_err());
    let scope = identity(7);
    assert!(PackIdentity::new(scope.table().clone(), vec![]).is_err());
    let retained = scope.table().retained(vec![]).unwrap();
    let tombstones = PackIdentity::new(retained, vec![]).unwrap();
    assert_eq!(
        tombstones.required_key(0x0200_1234).unwrap().local(),
        0x1234
    );
    let mut entries = scope.table().entries().to_vec();
    entries[0].id = PluginId::new(3).unwrap();
    assert!(PluginTable::from_entries(entries).is_err());
    let mut entries = scope.table().entries().to_vec();
    entries[0].normalized_name = "not-normalized".into();
    assert!(PluginTable::from_entries(entries).is_err());
}

/// The stored contract has no full/light slot ceiling and keeps keys wider than u32 intact.
#[test]
fn aggregate_past_both_legacy_limits_round_trips_without_runtime_slots() {
    let mut plugins = Vec::new();
    for (count, light, prefix, extension) in [(255, false, "F", "esm"), (4097, true, "L", "esl")] {
        for index in 0..count {
            plugins.push((
                format!("{prefix}{index:04}.{extension}"),
                PluginFacts {
                    flags: PluginFlags {
                        light,
                        ..Default::default()
                    },
                    header_version: Some(1.71),
                    content_hash: Some([index as u8; 32]),
                    ..Default::default()
                },
            ));
        }
    }
    let table = PluginTable::fresh(plugins).unwrap();
    let order = table.entries().iter().rev().map(|entry| entry.id).collect();
    let scope = PackIdentity::new(table, order).unwrap();
    let mut connection = Connection::open_in_memory().unwrap();
    let transaction = connection.transaction().unwrap();
    scope.write_sql(&transaction).unwrap();
    // The generation writes its registry before any key-bearing record rows.
    transaction.execute_batch("CREATE TABLE records(form_id INTEGER PRIMARY KEY); INSERT INTO records VALUES (73014446780)").unwrap();
    transaction.commit().unwrap();
    let restored = PackIdentity::read_sql(&connection).unwrap();
    assert_eq!(restored.table().entries().len(), 4352);
    let word: i64 = connection
        .query_row("SELECT form_id FROM records", [], |row| row.get(0))
        .unwrap();
    assert_eq!(word, 0x0011_0000_0abc);
    let key = restored.required_key(word).unwrap();
    assert_eq!(key.plugin().get(), 4352);
    assert_eq!(key.local(), 0xabc);
    assert_eq!(
        restored.table().named(key).unwrap().to_string(),
        "000ABC:L4096.esl"
    );
    assert_eq!(restored.order().first().unwrap().get(), 4352);
    assert_eq!(restored.order().last().unwrap().get(), 1);
    assert_eq!(restored.digest(), scope.digest());
}
