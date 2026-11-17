//! Fresh FormKey test-pack writers; these never accept or adapt a persisted legacy world.
use rusqlite::{Connection, params};
use shared::{
    pack_identity::PackIdentity,
    plugin_table::{PluginFacts, PluginTable},
    records::{ArchivedRecordData, ArchivedSubRecord},
};

/// Supply eight authenticated plugin priorities for asymmetric runtime-boundary fixtures.
pub(crate) fn install_projection(connection: &Connection) -> PackIdentity {
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
    connection
        .execute_batch("CREATE TABLE IF NOT EXISTS schema_info(version INTEGER)")
        .unwrap();
    connection.execute("INSERT INTO schema_info(version) SELECT ?1 WHERE NOT EXISTS(SELECT 1 FROM schema_info)",[shared::WORLD_DATABASE_SCHEMA_VERSION]).unwrap();
    let version: u32 = connection
        .query_row("SELECT version FROM schema_info", [], |row| row.get(0))
        .unwrap();
    if version == 0 {
        // create_tables supplies an unsealed, empty projection schema, never an old world.
        let records: i64 = connection
            .query_row("SELECT count(*) FROM records", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            records, 0,
            "native fixtures must allocate before writing records"
        );
        connection
            .execute(
                "UPDATE schema_info SET version=?1",
                [shared::WORLD_DATABASE_SCHEMA_VERSION],
            )
            .unwrap();
    }
    let transaction = connection.unchecked_transaction().unwrap();
    identity.write_sql(&transaction).unwrap();
    transaction.commit().unwrap();
    shared::world_assets::install_runtime_identity_functions(connection).unwrap();
    identity
}

/// Build a fresh canonical native fixture, including the contextual record contract.
pub(crate) fn install(connection: &mut Connection) -> PackIdentity {
    let identity = install_projection(connection);
    connection.execute_batch("CREATE TABLE IF NOT EXISTS records(form_id INTEGER PRIMARY KEY,record_type TEXT,flags INTEGER NOT NULL DEFAULT 0,cell_id INTEGER,worldspace_id INTEGER,load_order INTEGER,data BLOB)").unwrap();
    let transaction = connection.unchecked_transaction().unwrap();
    shared::records::create_record_context(&transaction, &identity).unwrap();
    transaction.execute_batch("CREATE TABLE inhouse_localized_fields(form_id INTEGER,field_index INTEGER,resolved_text TEXT)").unwrap();
    transaction.commit().unwrap();
    identity
}

/// Write a current runtime cache bound to its already authenticated synthetic world.
pub(crate) fn write_cache(
    path: &std::path::Path,
    identity: &PackIdentity,
    cache: &shared::CellCache,
) {
    std::fs::write(
        path,
        shared::cache_identity::encode_cell_cache(identity, cache, false).unwrap(),
    )
    .unwrap();
}

/// Convert newly generated single-owner source records through the production native writer.
/// The destination is an in-memory fixture, never an existing converted world to migrate.
pub(crate) fn publish_records(
    connection: &mut Connection,
    records: &std::collections::HashMap<u32, converter::esm::records::RawRecord>,
) -> color_eyre::Result<()> {
    let destination: String = connection.query_row(
        "SELECT file FROM pragma_database_list WHERE name='main'",
        [],
        |row| row.get(0),
    )?;
    assert!(
        destination.is_empty(),
        "synthetic source publication requires an in-memory destination"
    );
    let directory = tempfile::tempdir()?;
    let plugin = directory.path().join("Skyrim.esm");
    let database = directory.path().join("native.db");
    let mut bytes = Vec::new();
    let mut hedr = 1.7f32.to_le_bytes().to_vec();
    hedr.extend((records.len() as u32).to_le_bytes());
    hedr.extend(0x10000u32.to_le_bytes());
    let mut header = b"HEDR".to_vec();
    header.extend(12u16.to_le_bytes());
    header.extend(hedr);
    let append = |bytes: &mut Vec<u8>, kind: [u8; 4], id: u32, flags: u32, data: Vec<u8>| {
        bytes.extend(kind);
        bytes.extend((data.len() as u32).to_le_bytes());
        bytes.extend(flags.to_le_bytes());
        bytes.extend(id.to_le_bytes());
        bytes.extend([0; 4]);
        bytes.extend(44u16.to_le_bytes());
        bytes.extend([0; 2]);
        bytes.extend(data);
    };
    append(&mut bytes, *b"TES4", 0, 1, header);
    let mut records: Vec<_> = records.values().collect();
    records.sort_by_key(|record| record.form_id);
    for record in records {
        assert!(
            record.cell_form_id.is_none() && record.worldspace_form_id.is_none(),
            "grouped source fixtures require explicit GRUP serialization"
        );
        assert_eq!(record.load_order, 0);
        let mut fields = Vec::new();
        for (tag, data) in &record.subrecords {
            fields.extend(tag);
            fields.extend(u16::try_from(data.len())?.to_le_bytes());
            fields.extend(data);
        }
        append(
            &mut bytes,
            record.record_type,
            record.form_id,
            record.flags,
            fields,
        );
    }
    std::fs::write(&plugin, bytes)?;
    converter::esm::EsmParser::convert_form_key_plugins(&[plugin], &database)?;
    connection.restore("main", database, rusqlite::backup::NO_PROGRESS)?;
    Ok(())
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
            for (word_index, word) in field.data.as_chunks::<4>().0.iter().enumerate() {
                let raw = u32::from_le_bytes(*word);
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
