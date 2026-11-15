//! Runtime handles are transient checked views over one authenticated stored namespace.
use rusqlite::{Connection, params};
use shared::{
    form_key::FormKey,
    pack_identity::PackIdentity,
    plugin_table::{PluginFacts, PluginTable},
};

fn fixture() -> (Connection, PackIdentity) {
    let names: Vec<_> = (0..256)
        .rev()
        .map(|index| format!("Owner{index:03}.esm"))
        .collect();
    let table = PluginTable::fresh(
        names
            .iter()
            .map(|name| {
                (
                    name.clone(),
                    PluginFacts {
                        content_hash: Some([0x41; 32]),
                        ..Default::default()
                    },
                )
            })
            .collect(),
    )
    .unwrap();
    let order = names.iter().map(|name| table.id(name).unwrap()).collect();
    let identity = PackIdentity::new(table, order).unwrap();
    let mut connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch("CREATE TABLE schema_info(version INTEGER)")
        .unwrap();
    connection
        .execute(
            "INSERT INTO schema_info VALUES(?1)",
            [shared::WORLD_DATABASE_SCHEMA_VERSION],
        )
        .unwrap();
    let transaction = connection.transaction().unwrap();
    identity.write_sql(&transaction).unwrap();
    transaction.commit().unwrap();
    (connection, identity)
}

#[test]
fn sqlite_request_boundary_checks_scope_and_does_not_truncate_keys() {
    let (connection, identity) = fixture();
    shared::world_assets::install_runtime_identity_functions(&connection).unwrap();
    let stored = FormKey::new(identity.order()[0], 33).unwrap();
    assert!(stored.to_packed() > u64::from(u32::MAX));
    let unavailable = FormKey::new(identity.order()[254], 37).unwrap();
    let (encoded, decoded, declined, named): (i64, u32, Option<u32>, String) = connection
        .query_row(
            "SELECT stored_key(33),runtime_id(?1),runtime_id_or_null(?2),named_key(?2)",
            params![stored.to_sql_i64(), unavailable.to_sql_i64()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(encoded, stored.to_sql_i64());
    assert_eq!(decoded, 33);
    assert_eq!(declined, None);
    assert_eq!(named, "000025:Owner001.esm");
    assert!(
        connection
            .query_row("SELECT runtime_id(?1)", [unavailable.to_sql_i64()], |row| {
                row.get::<_, u32>(0)
            })
            .is_err()
    );
    let unknown = FormKey::new(shared::form_key::PluginId::new(999).unwrap(), 37).unwrap();
    assert!(
        connection
            .query_row(
                "SELECT runtime_id_or_null(?1)",
                [unknown.to_sql_i64()],
                |row| row.get::<_, Option<u32>>(0)
            )
            .is_err()
    );
    assert!(
        connection
            .query_row("SELECT runtime_id_or_null(0)", [], |row| row
                .get::<_, Option<u32>>(0))
            .is_err()
    );
}

#[test]
fn unavailable_runtime_owner_does_not_end_a_neighboring_catalog_scan() {
    let (connection, identity) = fixture();
    shared::world_assets::install_runtime_identity_functions(&connection).unwrap();
    connection
        .execute_batch("CREATE TABLE catalog(position INTEGER PRIMARY KEY,identity INTEGER)")
        .unwrap();
    for (position, owner) in [
        (0, identity.order()[0]),
        (1, identity.order()[254]),
        (2, identity.order()[1]),
    ] {
        connection
            .execute(
                "INSERT INTO catalog VALUES(?1,?2)",
                params![position, FormKey::new(owner, 33).unwrap().to_sql_i64()],
            )
            .unwrap();
    }
    let mut statement = connection
        .prepare("SELECT runtime_id_or_null(identity) FROM catalog ORDER BY position")
        .unwrap();
    let rows = statement
        .query_map([], |row| row.get::<_, Option<u32>>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(rows, vec![Some(33), None, Some(0x0100_0021)]);
}

#[test]
fn forged_pack_is_refused_before_registering_runtime_key_functions() {
    let (connection, _) = fixture();
    connection
        .execute("UPDATE pack_identity SET digest=zeroblob(32)", [])
        .unwrap();
    assert!(
        shared::world_assets::install_runtime_identity_functions(&connection)
            .unwrap_err()
            .to_string()
            .contains("convert again")
    );
    assert!(connection.prepare("SELECT runtime_id(1)").is_err());
}
