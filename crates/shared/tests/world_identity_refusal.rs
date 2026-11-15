//! Startup identity checks run before content SQL or cache payload access.
use rusqlite::Connection;
use shared::{
    pack_identity::PackIdentity,
    plugin_table::{PluginFacts, PluginTable},
    world_assets::validate_world_database,
};

/// Keep generated refusal fixtures under the test environment's task-owned TEMP directory.
fn fixture_root() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "formkey-startup-{}-{stamp}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir(&path).unwrap();
    path
}

/// Write only a schema stamp: reaching any content query would be a separate failure.
fn schema_only(path: &std::path::Path, version: u32) -> Connection {
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch("CREATE TABLE schema_info(version INTEGER NOT NULL)")
        .unwrap();
    connection
        .execute("INSERT INTO schema_info VALUES(?1)", [version])
        .unwrap();
    connection
}

/// A contiguous allocation authenticates its exact source proof independently of content.
fn identity() -> PackIdentity {
    let table = PluginTable::fresh(vec![(
        "Fixture.esm".into(),
        PluginFacts {
            content_hash: Some([0x6B; 32]),
            ..Default::default()
        },
    )])
    .unwrap();
    let owner = table.id("Fixture.esm").unwrap();
    PackIdentity::new(table, vec![owner]).unwrap()
}

/// Schema-9 converted worlds need a fresh conversion before content is consulted.
#[test]
fn legacy_world_refusal_precedes_content_queries() {
    let path = fixture_root().join("legacy.db");
    drop(schema_only(&path, 9));
    let error = validate_world_database(&path).unwrap_err().to_string();
    assert!(error.contains("convert again"), "{error}");
}

/// Merely changing a stamp cannot certify old or incomplete payloads as FormKey output.
#[test]
fn current_stamp_without_identity_is_refused_before_content_queries() {
    let path = fixture_root().join("stamp-only.db");
    drop(schema_only(&path, shared::WORLD_DATABASE_SCHEMA_VERSION));
    let error = validate_world_database(&path).unwrap_err().to_string();
    assert!(error.contains("convert again"), "{error}");
}

/// A forged allocation digest is rejected even when no payload table exists to decode.
#[test]
fn allocation_proof_failure_precedes_content_queries() {
    let path = fixture_root().join("tampered.db");
    let mut connection = schema_only(&path, shared::WORLD_DATABASE_SCHEMA_VERSION);
    let transaction = connection.transaction().unwrap();
    identity().write_sql(&transaction).unwrap();
    transaction.commit().unwrap();
    connection
        .execute("UPDATE pack_identity SET digest=zeroblob(32)", [])
        .unwrap();
    drop(connection);
    let error = validate_world_database(&path).unwrap_err().to_string();
    assert!(error.contains("convert again"), "{error}");
}

/// The authenticated startup contract does not require or inspect record payloads.
#[test]
fn current_identity_authenticates_without_content_queries() {
    let path = fixture_root().join("current.db");
    let mut connection = schema_only(&path, shared::WORLD_DATABASE_SCHEMA_VERSION);
    let transaction = connection.transaction().unwrap();
    identity().write_sql(&transaction).unwrap();
    transaction.commit().unwrap();
    drop(connection);
    assert_eq!(
        validate_world_database(&path).unwrap(),
        shared::WORLD_DATABASE_SCHEMA_VERSION
    );
}

/// Same schemas and build text cannot authenticate LOD metadata from another allocation.
#[test]
fn lod_manifest_allocation_mismatch_is_refused_before_content_keys() {
    let directory = fixture_root();
    let path = directory.join("skyrim_world.db");
    let mut connection = schema_only(&path, shared::WORLD_DATABASE_SCHEMA_VERSION);
    let scope = identity();
    let transaction = connection.transaction().unwrap();
    scope.write_sql(&transaction).unwrap();
    transaction.commit().unwrap();
    connection.execute_batch("CREATE TABLE lod_chunks(id INTEGER); CREATE TABLE lod_build(id INTEGER PRIMARY KEY,build_identity TEXT)").unwrap();
    connection
        .execute("INSERT INTO lod_build VALUES(1,?1)", ["a".repeat(64)])
        .unwrap();
    std::fs::write(
        directory.join("conversion-manifest.json"),
        serde_json::to_vec(
            &serde_json::json!({"schema_version":shared::LOD_CONVERTER_SCHEMA_VERSION}),
        )
        .unwrap(),
    )
    .unwrap();
    let mut manifest = serde_json::json!({"plugin_table_digest":"0".repeat(64),"build_identity":"a".repeat(64),"converter_schema":shared::LOD_CONVERTER_SCHEMA_VERSION,"world_database_schema":shared::WORLD_DATABASE_SCHEMA_VERSION,"chunks":0,"land_texture_repeats_per_cell":shared::LAND_TEXTURE_REPEATS_PER_CELL});
    let manifest_path = directory.join("lod-manifest.json");
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let error = shared::world_assets::validate_lod_build_contract(
        &directory,
        shared::LOD_CONVERTER_SCHEMA_VERSION,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("another world allocation"), "{error}");
    manifest["plugin_table_digest"] = serde_json::json!(scope.digest_hex());
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert_eq!(
        shared::world_assets::validate_lod_build_contract(
            &directory,
            shared::LOD_CONVERTER_SCHEMA_VERSION
        )
        .unwrap(),
        shared::WORLD_DATABASE_SCHEMA_VERSION
    );
}
