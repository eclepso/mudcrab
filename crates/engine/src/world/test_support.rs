//! Fresh FormKey test-pack writers; these never accept or adapt a persisted legacy world.
use rusqlite::{Connection, params};
use shared::{
    pack_identity::PackIdentity,
    plugin_table::{PluginFacts, PluginTable},
    records::{ArchivedRecordData, ArchivedSubRecord},
};

/// Supply eight authenticated plugin priorities for asymmetric runtime-boundary fixtures.
pub(crate) fn install(connection: &mut Connection) -> PackIdentity {
    let names = [
        "Base.esm",
        "Weather.esm",
        "Middle2.esm",
        "Middle3.esm",
        "Middle4.esm",
        "Middle5.esm",
        "Middle6.esm",
        "Winning.esm",
    ];
    let table = PluginTable::fresh(
        names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                (
                    name.to_string(),
                    PluginFacts {
                        masters: if index == 0 {
                            vec![]
                        } else {
                            vec!["Base.esm".into()]
                        },
                        content_hash: Some([index as u8 + 1; 32]),
                        ..Default::default()
                    },
                )
            })
            .collect(),
    )
    .unwrap();
    let order = names.iter().map(|name| table.id(name).unwrap()).collect();
    let identity = PackIdentity::new(table, order).unwrap();
    let transaction = connection.transaction().unwrap();
    shared::records::create_record_context(&transaction, &identity).unwrap();
    transaction.execute_batch("CREATE TABLE inhouse_localized_fields(form_id INTEGER,field_index INTEGER,resolved_text TEXT)").unwrap();
    transaction.commit().unwrap();
    identity
}

/// Encode one explicitly written synthetic runtime request as this fresh pack's stored key.
pub(crate) fn key(identity: &PackIdentity, runtime: u32) -> i64 {
    identity.runtime_key(runtime).unwrap().unwrap().to_sql_i64()
}

/// Publish canonical native fields and physical site outcomes atomically with their test record.
pub(crate) fn insert(
    connection: &Connection,
    identity: &PackIdentity,
    runtime: u32,
    kind: &str,
    priority: u32,
    fields: Vec<ArchivedSubRecord>,
) {
    let stored = key(identity, runtime);
    let supplier = identity.order()[priority as usize];
    let record_key = identity.required_key(stored).unwrap();
    let facts = &identity.table().entry(supplier).unwrap().facts;
    let selector = if record_key.plugin() == supplier {
        facts.masters.len()
    } else {
        facts
            .masters
            .iter()
            .position(|name| identity.table().id(name) == Some(record_key.plugin()))
            .expect("fixture supplier includes record owner")
    };
    let header = ((selector as u32) << 24) | record_key.local();
    let data = ArchivedRecordData { subrecords: fields };
    let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&data).unwrap();
    connection.execute("INSERT OR REPLACE INTO records(form_id,record_type,load_order,data) VALUES(?1,?2,?3,?4)",params![stored,kind,priority,bytes.as_slice()]).unwrap();
    // These are fixture replacement writes for the same selected record, not migration.
    connection
        .execute("DELETE FROM resolved_links WHERE record_key=?1", [stored])
        .unwrap();
    connection
        .execute(
            "DELETE FROM record_field_origins WHERE record_key=?1",
            [stored],
        )
        .unwrap();
    connection
        .execute(
            "DELETE FROM record_source_fields WHERE record_key=?1",
            [stored],
        )
        .unwrap();
    connection
        .execute(
            "INSERT OR REPLACE INTO record_context VALUES(?1,?2,?3,NULL,NULL,?4)",
            params![stored, supplier.get(), header, data.subrecords.len()],
        )
        .unwrap();
    for (ordinal, field) in data.subrecords.iter().enumerate() {
        connection
            .execute(
                "INSERT INTO record_field_origins VALUES(?1,?2,?2,?3,1)",
                params![stored, ordinal, field.tag.as_slice()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO record_source_fields VALUES(?1,?2,?3,?4)",
                params![stored, ordinal, field.tag.as_slice(), field.data],
            )
            .unwrap();
        if field.tag == *b"IMSP" {
            for (word_index, word) in field.data.chunks_exact(4).enumerate() {
                let raw = u32::from_le_bytes(word.try_into().unwrap());
                let target = shared::records::source_key(identity, supplier, raw).unwrap();
                connection
                    .execute(
                        "INSERT INTO resolved_links VALUES(?1,?2,?3,?4,?5,?6)",
                        params![
                            stored,
                            ordinal,
                            word_index * 4,
                            raw,
                            target.map(shared::form_key::FormKey::to_sql_i64),
                            if raw == 0 { "null" } else { "resolved" }
                        ],
                    )
                    .unwrap();
            }
        }
    }
}
