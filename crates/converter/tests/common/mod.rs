//! Assertions shared by synthetic publication fixtures with reader warnings or notices.
use converter::PipelineReport;
use rusqlite::OptionalExtension;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Locate a fixture's expected gameplay handle in the authenticated native allocation.
/// This is only a query convenience: the database always stores the full FormKey.
#[allow(dead_code)]
pub fn stored_key(connection: &rusqlite::Connection, runtime: u32) -> i64 {
    if runtime == 0 {
        // Explicit negative lookup for null: a current world must never store SQL zero.
        return 0;
    }
    shared::pack_identity::PackIdentity::read_sql(connection)
        .unwrap()
        .runtime_key(runtime)
        .unwrap()
        .expect("fixture query requires a non-null identity")
        .to_sql_i64()
}

/// Compare representable fixture identities without depending on pack allocation order.
#[allow(dead_code)]
pub fn runtime_id(connection: &rusqlite::Connection, stored: i64) -> u32 {
    let identity = shared::pack_identity::PackIdentity::read_sql(connection).unwrap();
    identity
        .runtime_id(identity.required_key(stored).unwrap())
        .unwrap()
}

/// Read original canonical field bytes through the authenticated context contract.
#[allow(dead_code)]
pub fn native_fields(
    connection: &rusqlite::Connection,
    runtime: u32,
    signature: &[u8; 4],
) -> Vec<Vec<u8>> {
    let reader = shared::records::RecordContextReader::new(connection).unwrap();
    let key = reader.identity().runtime_key(runtime).unwrap().unwrap();
    reader
        .record(key)
        .unwrap()
        .subrecords
        .into_iter()
        .filter(|field| &field.tag == signature)
        .map(|field| field.data)
        .collect()
}

/// Interpret current native fields for the pre-existing runtime-link behavior oracle.
/// Original bytes are checked separately with `native_fields`; only certified semantic
/// sites are interpreted, so aliases, padding and unrelated integers stay untouched.
#[allow(dead_code)]
pub fn runtime_fields(
    connection: &rusqlite::Connection,
    runtime: u32,
    signature: &[u8; 4],
) -> Vec<Vec<u8>> {
    runtime_record(connection, runtime)
        .subrecords
        .into_iter()
        .filter(|field| &field.tag == signature)
        .map(|field| field.data)
        .collect()
}

/// Present only certified current-world fields to inherited runtime behavior assertions.
/// SQL canonical bytes are cross-checked against the independently loaded context before
/// this transient interpretation; the source archive and stored payload are never changed.
#[allow(dead_code)]
pub fn runtime_record(
    connection: &rusqlite::Connection,
    runtime: u32,
) -> shared::records::ArchivedRecordData {
    let reader = shared::records::RecordContextReader::new(connection).unwrap();
    let key = reader.identity().runtime_key(runtime).unwrap().unwrap();
    let record = reader.record(key).unwrap();
    let bytes: Vec<u8> = connection
        .query_row(
            "SELECT data FROM records WHERE form_id=?1",
            [key.to_sql_i64()],
            |row| row.get(0),
        )
        .unwrap();
    let mut aligned = rkyv::util::AlignedVec::<16>::new();
    aligned.extend_from_slice(&bytes);
    let canonical =
        rkyv::from_bytes::<shared::records::ArchivedRecordData, rkyv::rancor::Error>(&aligned)
            .unwrap();
    assert_eq!(canonical.subrecords, record.subrecords);
    let mut fields = Vec::new();
    for (ordinal, field) in record.subrecords.iter().enumerate() {
        if !record.is_runtime_usable(ordinal) {
            continue;
        }
        let text: Option<Option<String>> = connection
            .query_row(
                "SELECT text FROM record_text WHERE record_key=?1 AND source_ordinal=?2",
                rusqlite::params![key.to_sql_i64(), record.source_ordinals[ordinal]],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        let mut bytes = field.data.clone();
        let offsets = connection
            .prepare("SELECT byte_offset FROM resolved_links WHERE record_key=?1 AND source_ordinal=?2 ORDER BY byte_offset")
            .unwrap()
            .query_map(
                rusqlite::params![key.to_sql_i64(), record.source_ordinals[ordinal]],
                |row| row.get::<_, usize>(0),
            )
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        if let Some(text) = text {
            assert!(offsets.is_empty(), "text may not overlap a semantic link");
            // The inherited byte oracle represents an unavailable runtime string by
            // its empty wire leaf; SQL NULL and the original native ID are asserted
            // independently. Omission is controlled only by runtime_usable above.
            bytes = text.unwrap_or_default().into_bytes();
            bytes.push(0);
        }
        for offset in offsets {
            let runtime = record
                .resolve_at(ordinal, offset)
                .unwrap()
                .map(|target| reader.identity().runtime_id(target).unwrap())
                .unwrap_or(0);
            bytes[offset..offset + 4].copy_from_slice(&runtime.to_le_bytes());
        }
        fields.push(shared::records::ArchivedSubRecord {
            tag: field.tag,
            data: bytes,
        });
    }
    shared::records::ArchivedRecordData { subrecords: fields }
}

/// Build the current-generation portion of a synthetic interrupted-run fixture.
/// Existing database/manifest bytes must already authenticate as current; this helper
/// cannot upgrade an old world or relabel an old published producer.
#[allow(dead_code)]
pub fn current_staging_generation(staging: &Path) {
    let database = staging.join("skyrim_world.db");
    if database.exists() {
        shared::world_assets::validate_world_database(&database).unwrap();
        let connection = rusqlite::Connection::open(&database).unwrap();
        shared::records::RecordContextReader::new(&connection).unwrap();
    }
    let manifest = staging.join("conversion-manifest.json");
    if manifest.exists() {
        converter::cache::ConversionManifest::load(&manifest)
            .unwrap()
            .validate_world_generation(staging)
            .unwrap();
    }
    let proof = staging.join(converter::cache::STAGING_GENERATION_FILE);
    assert!(
        !proof.exists(),
        "fixture must establish one initial generation"
    );
    fs::write(
        proof,
        serde_json::to_vec(&serde_json::json!({
            "converter_schema": converter::cache::CONVERTER_SCHEMA_VERSION,
            "world_database_schema": shared::WORLD_DATABASE_SCHEMA_VERSION,
        }))
        .unwrap(),
    )
    .unwrap();
}

/// Build a genuine current world from original fixture plugins, then copy the whole
/// authenticated database into memory for scalar-table assertions.
#[allow(dead_code)]
pub fn native_database(paths: &[PathBuf]) -> rusqlite::Connection {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fixture.db");
    converter::esm::EsmParser::convert_plugins(paths, &path).unwrap();
    let source = rusqlite::Connection::open(path).unwrap();
    shared::records::RecordContextReader::new(&source).unwrap();
    let mut connection = rusqlite::Connection::open_in_memory().unwrap();
    {
        let backup = rusqlite::backup::Backup::new(&source, &mut connection).unwrap();
        backup
            .run_to_completion(64, std::time::Duration::from_millis(0), None)
            .unwrap();
    }
    shared::records::RecordContextReader::new(&connection).unwrap();
    connection
}

/// Read the current authenticated wire format, then expose checked gameplay handles solely
/// for existing geometry/recovery oracles. No old bytes are accepted by this fixture helper.
#[allow(dead_code)]
pub fn runtime_cache(output: &Path, name: &str) -> shared::CellCache {
    let database = rusqlite::Connection::open(output.join("skyrim_world.db")).unwrap();
    let identity = shared::pack_identity::PackIdentity::read_sql(&database).unwrap();
    let mut cache =
        converter::esm::cell_cache::read_form_key_cell_cache(&output.join(name)).unwrap();
    let runtime = |key| {
        u64::from(
            identity
                .runtime_id(shared::cache_identity::required(&identity, key).unwrap())
                .unwrap(),
        )
    };
    for cell in &mut cache.cells {
        cell.cell_id = runtime(cell.cell_id);
        cell.water_type_form_id = cell.water_type_form_id.map(runtime);
        for layer in &mut cell.layers {
            if layer.texture_form_id != 0 {
                layer.texture_form_id = runtime(layer.texture_form_id);
            }
        }
    }
    cache
}

/// Keep deterministic pipeline fixtures independent of GPU availability.
// Each test crate uses only some of these helpers.
#[allow(dead_code)]
pub fn cpu_lod_config(
    data_dir: impl Into<PathBuf>,
    output_dir: impl Into<PathBuf>,
) -> converter::PipelineConfig {
    let mut config = converter::PipelineConfig::new(data_dir, output_dir);
    config.lod_texture_encoder = converter::TextureEncoder::Cpu;
    config
}

/// Reader recovery publishes usable assets without certifying complete record data.
#[allow(dead_code)]
pub fn assert_reader_warning_publication(report: &PipelineReport, output: &Path) {
    assert!(
        !report.complete,
        "reader loss must remain incomplete: {report:?}"
    );
    assert_eq!(
        report.skipped, 0,
        "reader loss is not an asset failure: {report:?}"
    );
    assert!(
        !report.warnings.is_empty(),
        "missing reader recovery warning"
    );
    assert!(
        report
            .warnings
            .iter()
            .all(|warning| warning.contains("in-house reader")
                || warning.contains("inhouse adapter")
                || (warning.starts_with("warning: ")
                    && warning.contains(" cleared ")
                    && warning.contains(" invalid native links (first: native source "))
                || (warning.starts_with("warning: CELL ")
                    && warning.contains(" wins; dropped LAND "))
                || (warning.starts_with("warning: ")
                    && warning.contains(": omitted ")
                    && warning.contains(" unusable localized fields (first: "))
                || (warning.starts_with("warning: omitted ")
                    && warning.ends_with(" rows with a null required semantic key"))),
        "unexpected non-reader failure: {report:?}"
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("conversion-manifest.json")).unwrap())
            .unwrap();
    assert_eq!(manifest["complete"], false);
    assert_eq!(manifest["record_data_complete"], false);
    assert!(manifest["failures"].as_object().unwrap().is_empty());
    for artifact in ["skyrim_world.db", "cell_cache.rkyv", "record-reader.json"] {
        assert!(output.join(artifact).is_file(), "unpublished {artifact}");
    }
}

/// Faithful reader outcomes publish complete assets and retain bounded diagnostics.
#[allow(
    dead_code,
    reason = "Each integration target selects its expected reader severity."
)]
pub fn assert_reader_notice_publication(report: &PipelineReport, output: &Path) {
    assert!(report.complete, "faithful reader outcome: {report:?}");
    assert_eq!(report.skipped, 0, "reader notice is not an asset failure");
    assert!(report.warnings.is_empty(), "{report:?}");
    assert!(
        report.notices.iter().any(|notice| {
            notice.contains("in-house reader")
                || notice.contains("localized IDs missing from readable string tables")
                || notice.contains("localized IDs missing from readable tables")
                || notice
                    .contains("FormID occurrence(s) past the original master list resolved to self")
        }),
        "missing reader notice: {report:?}"
    );

    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("conversion-manifest.json")).unwrap())
            .unwrap();
    assert_eq!(manifest["complete"], true);
    assert_eq!(manifest["record_data_complete"], true);
    assert!(manifest["failures"].as_object().unwrap().is_empty());

    for artifact in ["skyrim_world.db", "cell_cache.rkyv", "record-reader.json"] {
        assert!(output.join(artifact).is_file(), "unpublished {artifact}");
    }
    for artifact in ["record-reader.json", "inhouse-reader-diagnostics.json"] {
        assert!(
            report.artifacts.contains(&PathBuf::from(artifact)),
            "reader report omitted {artifact}: {report:?}"
        );
    }

    let reader: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("record-reader.json")).unwrap()).unwrap();
    assert_eq!(reader["mode"], "inhouse");
    assert!(reader["schema_sha256"].as_str().is_some());

    let diagnostics: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("inhouse-reader-diagnostics.json")).unwrap())
            .unwrap();
    let database = rusqlite::Connection::open(output.join("skyrim_world.db")).unwrap();
    let identity = shared::pack_identity::PackIdentity::read_sql(&database).unwrap();
    assert_eq!(diagnostics["format_version"], 2);
    assert_eq!(diagnostics["plugin_table_digest"], identity.digest_hex());
    assert!(diagnostics["decoder"].is_object());
    assert_eq!(diagnostics["warnings"], serde_json::json!([]));
    for notice in diagnostics["notices"].as_array().unwrap() {
        let notice = notice.as_str().unwrap();
        assert!(report.notices.iter().any(|reported| reported == notice));
    }
}
