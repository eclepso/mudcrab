//! Database and LOD package startup contracts shared by the runtime and launcher.
use color_eyre::{Result, eyre::WrapErr};
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use std::path::Path;

/// Install checked transient-handle conversions for one already authenticated world connection.
/// SQL joins stay in FormKey space; only request binds and gameplay result columns use these.
pub fn install_runtime_identity_functions(
    connection: &Connection,
) -> Result<std::sync::Arc<crate::pack_identity::PackIdentity>> {
    use rusqlite::functions::FunctionFlags;
    let identity = std::sync::Arc::new(
        crate::pack_identity::PackIdentity::read_sql(connection)
            .wrap_err("world identity is unavailable; convert again")?,
    );
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    let decode = identity.clone();
    connection.create_scalar_function("runtime_id", 1, flags, move |context| {
        let value: Option<i64> = context.get(0)?;
        value
            .map(|value| {
                let key = decode.required_key(value)?;
                decode.runtime_id(key)
            })
            .transpose()
            .map_err(runtime_function_error)
    })?;
    let catalog = identity.clone();
    connection.create_scalar_function("runtime_id_or_null", 1, flags, move |context| {
        let value: Option<i64> = context.get(0)?;
        value
            .map(|value| {
                catalog
                    .required_key(value)
                    .and_then(|key| catalog.runtime_id_if_available(key))
            })
            .transpose()
            .map(Option::flatten)
            .map_err(runtime_function_error)
    })?;
    let encode = identity.clone();
    connection.create_scalar_function("stored_key", 1, flags, move |context| {
        let value: Option<u32> = context.get(0)?;
        value
            .map(|value| encode.runtime_key(value))
            .transpose()
            .map(|key| key.flatten().map(crate::form_key::FormKey::to_sql_i64))
            .map_err(runtime_function_error)
    })?;
    let names = identity.clone();
    connection.create_scalar_function("named_key", 1, flags, move |context| {
        let value: Option<i64> = context.get(0)?;
        value
            .map(|value| {
                let key = names.required_key(value)?;
                names
                    .table()
                    .named(key)
                    .map(|named| named.to_string())
                    .map_err(color_eyre::eyre::Error::new)
            })
            .transpose()
            .map_err(runtime_function_error)
    })?;
    Ok(identity)
}

/// Preserve the conversion's explanation in SQLite's scalar-function error boundary.
fn runtime_function_error(error: color_eyre::eyre::Report) -> rusqlite::Error {
    rusqlite::Error::UserFunctionError(Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        error.to_string(),
    )))
}

/// Preserve existing rusqlite reader APIs while requiring the same authenticated boundary.
pub fn install_runtime_identity_functions_sql(
    connection: &Connection,
) -> rusqlite::Result<std::sync::Arc<crate::pack_identity::PackIdentity>> {
    install_runtime_identity_functions(connection).map_err(runtime_function_error)
}

/// Authenticate a runtime cache's envelope and wire keys using its adjacent world allocation.
pub fn validate_runtime_cell_cache(assets_dir: &Path) -> Result<()> {
    let connection = Connection::open_with_flags(
        assets_dir.join("skyrim_world.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let identity = crate::pack_identity::PackIdentity::read_sql(&connection)
        .wrap_err("world identity is unavailable; convert again")?;
    let bytes = std::fs::read(assets_dir.join("cell_cache.rkyv"))
        .wrap_err("cell cache is unavailable; convert again")?;
    let mut aligned = rkyv::util::AlignedVec::<16>::with_capacity(bytes.len());
    aligned.extend_from_slice(&bytes);
    crate::cache_identity::cell_cache_payload(&identity, &aligned, false)
        .wrap_err("cell cache is incompatible; convert again")?;
    Ok(())
}

pub fn validate_world_database(path: &Path) -> Result<u32> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .wrap_err_with(|| format!("failed to open {}", path.display()))?;
    let version: u32 = connection
        .query_row("SELECT version FROM schema_info LIMIT 1", [], |row| {
            row.get(0)
        })
        .wrap_err("world database has no valid schema version; convert again")?;
    color_eyre::eyre::ensure!(
        crate::supports_runtime_world_database_schema(version),
        "world database schema {version} is unsupported; convert again with the current converter (supported schemas {} through {})",
        crate::MIN_RUNTIME_WORLD_DATABASE_SCHEMA_VERSION,
        crate::WORLD_DATABASE_SCHEMA_VERSION
    );
    crate::pack_identity::PackIdentity::read_sql(&connection)
        .wrap_err("world identity is unavailable or invalid; convert again")?;
    Ok(version)
}

pub fn validate_lod_build_contract(assets_dir: &Path, converter_schema: u32) -> Result<u32> {
    let database_path = assets_dir.join("skyrim_world.db");
    let version = validate_world_database(&database_path)?;
    let connection = Connection::open_with_flags(&database_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .wrap_err_with(|| format!("failed to open {}", database_path.display()))?;
    let identity = crate::pack_identity::PackIdentity::read_sql(&connection)
        .wrap_err("LOD world identity is unavailable; convert again")?;
    let manifest_path = assets_dir.join("lod-manifest.json");
    if !has_lod_table(&connection)? {
        let version: u32 =
            connection.query_row("SELECT version FROM schema_info", [], |row| row.get(0))?;
        color_eyre::eyre::ensure!(
            version < crate::WORLD_DATABASE_LOD_SCHEMA_VERSION && !manifest_path.exists(),
            "world database has no LOD chunk table"
        );
        return Ok(version);
    }
    let chunk_count: i64 = connection
        .query_row("SELECT count(*) FROM lod_chunks", [], |row| row.get(0))
        .wrap_err("world database has no LOD chunk table")?;
    if !manifest_path.is_file() {
        color_eyre::eyre::ensure!(
            chunk_count == 0,
            "world database contains {chunk_count} LOD chunks but {} is missing",
            manifest_path.display()
        );
        return Ok(version);
    }

    let conversion: LodProducerManifest = serde_json::from_slice(
        &std::fs::read(assets_dir.join("conversion-manifest.json"))
            .wrap_err("LOD package has no conversion producer manifest")?,
    )
    .wrap_err("invalid LOD conversion producer manifest")?;
    color_eyre::eyre::ensure!(
        conversion.schema_version == crate::LOD_CONVERTER_SCHEMA_VERSION,
        "LOD conversion producer is stale; reconvert assets with converter schema {}",
        crate::LOD_CONVERTER_SCHEMA_VERSION
    );

    let bytes = std::fs::read(&manifest_path)
        .wrap_err_with(|| format!("failed to read {}", manifest_path.display()))?;
    let manifest: LodBuildManifest =
        serde_json::from_slice(&bytes).wrap_err("invalid LOD build manifest; convert again")?;
    color_eyre::eyre::ensure!(
        manifest.plugin_table_digest == identity.digest_hex(),
        "LOD metadata belongs to another world allocation; convert again"
    );
    color_eyre::eyre::ensure!(
        manifest.land_texture_repeats_per_cell == crate::LAND_TEXTURE_REPEATS_PER_CELL,
        "LOD terrain texture scale is stale; rebuild LOD metadata for {} repeats per cell",
        crate::LAND_TEXTURE_REPEATS_PER_CELL
    );
    color_eyre::eyre::ensure!(
        converter_schema == crate::LOD_CONVERTER_SCHEMA_VERSION
            && manifest.converter_schema == converter_schema
            && manifest.world_database_schema == crate::WORLD_DATABASE_SCHEMA_VERSION
            && connection.query_row("SELECT version FROM schema_info", [], |row| row
                .get::<_, u32>(0))?
                == crate::WORLD_DATABASE_SCHEMA_VERSION,
        "LOD manifest schema is stale; reconvert assets with converter schema {converter_schema} and world database schema {}",
        crate::WORLD_DATABASE_SCHEMA_VERSION
    );
    color_eyre::eyre::ensure!(
        is_canonical_sha256(&manifest.build_identity),
        "LOD manifest build identity is not a lowercase SHA-256 digest"
    );
    let database_identity: String = connection
        .query_row(
            "SELECT build_identity FROM lod_build WHERE id=1",
            [],
            |row| row.get(0),
        )
        .wrap_err("world database has no LOD build identity")?;
    color_eyre::eyre::ensure!(
        database_identity == manifest.build_identity,
        "LOD database and manifest build identities do not match"
    );
    color_eyre::eyre::ensure!(
        u64::try_from(chunk_count).ok() == Some(manifest.chunks),
        "LOD manifest declares {} chunks but the world database contains {chunk_count}",
        manifest.chunks
    );
    Ok(version)
}

#[derive(Deserialize)]
struct LodProducerManifest {
    schema_version: u32,
}

#[derive(Deserialize)]
struct LodBuildManifest {
    plugin_table_digest: String,
    build_identity: String,
    converter_schema: u32,
    world_database_schema: u32,
    chunks: u64,
    #[serde(default)]
    land_texture_repeats_per_cell: f32,
}

fn has_lod_table(connection: &Connection) -> Result<bool> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='lod_chunks')",
        [],
        |row| row.get(0),
    )?)
}

fn is_canonical_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
