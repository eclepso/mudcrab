//! Production FormKey world generation. Native evidence and scalar projection are separate.
use super::{
    form_key_merge::{FormKeyConversion, FormKeyRecord},
    form_key_projection::ProjectionStage,
    form_keys::FormKeyOrder,
    inhouse::{StringSources, read_plugin_strings},
};
use crate::{
    config::RecordReader,
    records::{self, Value, addons::RuntimeSchemas},
};
use color_eyre::{
    Result,
    eyre::{ensure, eyre},
};
use rusqlite::{Connection, Transaction, params, types::Value as SqlValue};
use shared::{form_key::FormKey, pack_identity::PackIdentity};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    time::Instant,
};

/// Native text metadata distinguishes inline strings from localized table entries.
type TextFact = (String, Option<String>, Option<u32>);

/// Provenance stays native and independent of the scalar projection's compact CPU indices.
#[derive(Default)]
struct SourceFact {
    offset: u64,
    payload: Vec<u8>,
    text_fields: BTreeMap<usize, TextFact>,
}

/// Build and publish a complete SQL generation with the selected independent field decoder.
pub(crate) fn convert(
    paths: &[PathBuf],
    database: &Path,
    reader: RecordReader,
    strings: StringSources<'_>,
    schemas: Option<&RuntimeSchemas>,
) -> Result<super::RecordConversion> {
    convert_with_dump(
        paths,
        database,
        reader,
        strings,
        schemas,
        None,
        super::inhouse::RecordBundleMode::Full,
    )
}

/// Typed exports and runtime publication share the same native admission and contextual writer.
pub(crate) fn convert_with_dump(
    paths: &[PathBuf],
    database: &Path,
    reader: RecordReader,
    strings: StringSources<'_>,
    schemas: Option<&RuntimeSchemas>,
    dump: Option<(&Path, &[[u8; 4]])>,
    mode: super::inhouse::RecordBundleMode,
) -> Result<super::RecordConversion> {
    let started = Instant::now();
    let (paths, name_warnings) = super::form_keys::recover_plugin_paths(paths)?;
    let default_schemas;
    let schemas = match schemas {
        Some(schemas) => schemas,
        None => {
            default_schemas =
                RuntimeSchemas::from_addon_bytes(&[]).map_err(color_eyre::eyre::Error::msg)?;
            &default_schemas
        }
    };
    let mut typed = None;
    let mut facts = HashMap::new();
    let mut conversion = if reader == RecordReader::Inhouse {
        let order = FormKeyOrder::read(&paths)?;
        let mut read =
            records::read_plugins_form_keys_with_validation(&paths, &order, schemas, |native| {
                let scalar = &native.canonical;
                if matches!(&scalar.record_type, b"GMST" | b"RACE") {
                    super::exporter::validate_inhouse_record(scalar)
                        .map_err(|error| error.to_string())?;
                }
                if scalar.record_type == *b"MOVT" {
                    let key = order
                        .resolve(
                            scalar.load_order as usize,
                            native.source.form_id,
                            shared::source_form_id::ZeroPolicy::Null,
                        )
                        .map_err(|error| error.to_string())?;
                    if key.is_some_and(|key| {
                        key.local() == 0x3580d
                            && order
                                .table()
                                .entry(key.plugin())
                                .is_some_and(|entry| entry.normalized_name == "skyrim.esm")
                    }) {
                        let mut selected = scalar.clone();
                        selected.form_id = 0x3580d;
                        super::exporter::validate_inhouse_record(&selected)
                            .map_err(|error| error.to_string())?;
                    }
                }
                if scalar.record_type == *b"LAND" {
                    super::cell_cache::validate_inhouse_land(scalar)
                        .map_err(|error| error.to_string())?;
                }
                Ok(())
            })?;
        let warnings = read.warnings();
        let notices = read.notices();
        let mut records = HashMap::with_capacity(read.records.len());
        for (key, mut native) in std::mem::take(&mut read.records) {
            let mut text = BTreeMap::new();
            let mut omitted = BTreeSet::new();
            for (index, field) in native.decoded.fields.iter().enumerate() {
                if let Some(&origin) = native.canonical_ordinals.get(index) {
                    if field.opaque {
                        omitted.insert(origin);
                    }
                    if let Value::String(value) = &field.value {
                        text.insert(origin, Some(value.clone()));
                    }
                }
            }
            let text_fields = native
                .decoded
                .fields
                .iter()
                .enumerate()
                .filter_map(|(index, field)| {
                    let &origin = native.canonical_ordinals.get(index)?;
                    match &field.value {
                        Value::LocalizedString(id) => Some((
                            origin,
                            (
                                field.name.clone(),
                                Some(
                                    field
                                        .localized_bank
                                        .clone()
                                        .unwrap_or_else(|| "strings".into()),
                                ),
                                Some(*id),
                            ),
                        )),
                        Value::String(_) => Some((origin, (field.name.clone(), None, None))),
                        _ => None,
                    }
                })
                .collect();
            if dump.is_some() {
                facts.insert(
                    key,
                    SourceFact {
                        offset: native.decoded.source_record_offset,
                        payload: native.decoded.raw_payload.clone(),
                        text_fields,
                    },
                );
                records.insert(
                    key,
                    FormKeyRecord {
                        source: native.source.clone(),
                        canonical: Some(native.canonical.clone()),
                        canonical_ordinals: native.canonical_ordinals.clone(),
                        localized_text: text,
                        runtime_omitted: omitted,
                        cell: native.cell,
                        worldspace: native.worldspace,
                        links: native.links.clone(),
                    },
                );
                read.records.insert(key, native);
            } else {
                facts.insert(
                    key,
                    SourceFact {
                        offset: native.decoded.source_record_offset,
                        payload: std::mem::take(&mut native.decoded.raw_payload),
                        text_fields,
                    },
                );
                records.insert(
                    key,
                    FormKeyRecord {
                        source: native.source,
                        canonical: Some(native.canonical),
                        canonical_ordinals: native.canonical_ordinals,
                        localized_text: text,
                        runtime_omitted: omitted,
                        cell: native.cell,
                        worldspace: native.worldspace,
                        links: native.links,
                    },
                );
            }
        }
        typed = Some(read);
        FormKeyConversion {
            order,
            records,
            warnings,
            notices,
        }
    } else {
        let conversion = super::EsmParser::merge_plugins_form_keys(&paths)?;
        collect_source_facts(&paths, &conversion, &mut facts)?;
        conversion
    };
    conversion.warnings.extend(name_warnings);
    localize(&mut conversion, &paths, strings, &facts)?;
    if let Some((path, signatures)) = dump {
        use std::io::Write;
        let read = typed
            .as_mut()
            .ok_or_else(|| eyre!("typed dump requires the typed native reader"))?;
        let mut writer =
            std::io::BufWriter::new(std::fs::File::create(path.join("typed-records.jsonl"))?);
        let wanted = |tag: &[u8; 4]| signatures.is_empty() || signatures.contains(tag);
        for header in &read.headers {
            if wanted(&header.source.record_type) {
                header.write_typed_header(
                    &mut writer,
                    conversion.order.table(),
                    &BTreeMap::new(),
                )?;
                writer.write_all(b"\n")?;
            }
        }
        // TES4 headers remain first in supplier order. Winner ordering retains the
        // original record-type/full/light owner order, independent of pack allocation.
        let mut ordered = read
            .records
            .iter()
            .map(|(key, native)| {
                Ok((
                    native.source.record_type,
                    conversion.order.identity_sort_tuple(*key)?,
                    *key,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        ordered
            .sort_unstable_by_key(|(signature, original_order, _)| (*signature, *original_order));
        for (_, _, key) in ordered {
            let native = &read.records[&key];
            if !wanted(&native.source.record_type) {
                continue;
            }
            let localization=conversion.records[&key].localized_text.iter().filter_map(|(&ordinal,text)|{
                let (field_name,bank,id)=facts[&key].text_fields.get(&ordinal)?;let id=(*id)?;
                Some((ordinal,serde_json::json!({"field_name":field_name,"string_table":bank,"string_id":id,"resolved_text":text,"status":if text.is_some(){"resolved"}else if id==0{"null_id"}else{"missing"}})))
            }).collect();
            native.write_typed(&mut writer, key, conversion.order.table(), &localization)?;
            writer.write_all(b"\n")?;
        }
        writer.flush()?;
        read.records.clear();
    }
    let offsets = facts
        .iter()
        .map(|(key, fact)| (*key, fact.offset))
        .collect::<HashMap<_, _>>();
    let (land_winners, land_warnings) =
        super::cell_cache::native_land_winners(&conversion, &offsets)?;
    conversion.warnings.extend(land_warnings);
    let mut stage = ProjectionStage::build(&conversion)?;
    let selected_lands = land_winners
        .iter()
        .map(|(_, land)| stage.handle(*land))
        .collect::<Result<BTreeSet<_>>>()?;
    stage
        .master
        .retain(|handle, record| record.record_type != *b"LAND" || selected_lands.contains(handle));
    let projected = stage.project_with_source_offsets(&offsets)?;
    let mut output = Connection::open(database)?;
    output.pragma_update(None, "foreign_keys", true)?;
    output.pragma_update(None, "cache_size", -65_536)?;
    let projection_warnings = write_world(
        &mut output,
        WorldSnapshot {
            projected: &projected,
            stage: &stage,
            conversion: &conversion,
            facts: &facts,
            paths: &paths,
            schemas,
            typed: typed.as_ref(),
            mode,
        },
    )?;
    drop(projected);
    drop(stage);
    conversion.warnings.extend(projection_warnings);
    if let Some(read) = typed.as_ref() {
        if let Some(parent) = database.parent() {
            std::fs::write(
                parent.join("inhouse-reader-diagnostics.json"),
                serde_json::to_vec_pretty(
                    &serde_json::json!({"format_version":2,"plugin_table_digest":conversion.order.pack_identity()?.digest_hex(),"decoder":read.diagnostics,"warnings":conversion.warnings,"notices":conversion.notices}),
                )?,
            )?;
        }
    }
    let warnings = conversion.warnings.clone();
    let notices = conversion.notices.clone();
    let mut phases = BTreeMap::new();
    phases.insert(
        "native_form_key_world".into(),
        started.elapsed().as_secs_f64(),
    );
    Ok(super::RecordConversion {
        records: HashMap::new(),
        warnings,
        notices,
        phases,
        native: Some(conversion),
        source_offsets: offsets,
    })
}

/// A second framing-only pass retains exact source payloads/positions for the handwritten decoder.
fn collect_source_facts(
    paths: &[PathBuf],
    conversion: &FormKeyConversion,
    facts: &mut HashMap<FormKey, SourceFact>,
) -> Result<()> {
    let mut candidates: HashMap<(usize, u32), Vec<FormKey>> = HashMap::new();
    for (&key, record) in &conversion.records {
        candidates
            .entry((record.source.load_order as usize, record.source.form_id))
            .or_default()
            .push(key);
    }
    for (priority, path) in paths.iter().enumerate() {
        let mut diagnostics = records::PluginDiagnostics::default();
        records::scanner::scan_plugin(
            path,
            &mut diagnostics,
            |_| {},
            |source, _| {
                let Some(keys) = candidates.get(&(priority, source.source_form_id)) else {
                    return;
                };
                let Ok(fields) = super::extractors::extract_subrecords(&source.payload) else {
                    return;
                };
                for key in keys {
                    let original = &conversion.records[key].source;
                    if original.record_type == source.record_type
                        && original.flags == source.flags
                        && original.cell_form_id == source.cell_form_id
                        && original.worldspace_form_id == source.worldspace_form_id
                        && original.subrecords == fields
                    {
                        facts.insert(
                            *key,
                            SourceFact {
                                offset: source.source_record_offset,
                                payload: source.payload.clone(),
                                ..Default::default()
                            },
                        );
                    }
                }
            },
        )?;
    }
    ensure!(
        facts.len() == conversion.records.len(),
        "native source provenance is incomplete; output retained"
    );
    Ok(())
}

/// Local field failures remain bounded diagnostics; original canonical bytes stay unchanged.
fn localize(
    conversion: &mut FormKeyConversion,
    paths: &[PathBuf],
    sources: StringSources<'_>,
    facts: &HashMap<FormKey, SourceFact>,
) -> Result<()> {
    let flags = paths
        .iter()
        .map(|path| super::binary::parse_plugin_metadata(path).map(|metadata| metadata.flags))
        .collect::<Result<Vec<_>>>()?;
    let mut tables = HashMap::new();
    let mut losses: BTreeMap<usize, (usize, String)> = BTreeMap::new();
    let mut gaps: BTreeMap<usize, (usize, String)> = BTreeMap::new();
    for (key, record) in &mut conversion.records {
        let priority = record.source.load_order as usize;
        for (ordinal, (tag, bytes)) in record.source.subrecords.iter().enumerate() {
            let bank = facts
                .get(key)
                .and_then(|fact| fact.text_fields.get(&ordinal))
                .and_then(|(_, bank, _)| bank.as_deref())
                .or_else(|| records::localized_field_bank(&record.source.record_type, tag));
            let Some(bank) = bank else { continue };
            if record.runtime_omitted.contains(&ordinal) {
                continue;
            }
            if flags[priority] & 0x80 == 0 {
                let end = bytes
                    .iter()
                    .position(|byte| *byte == 0)
                    .unwrap_or(bytes.len());
                record
                    .localized_text
                    .insert(ordinal, Some(records::decode_text(&bytes[..end])));
                continue;
            }
            let table = tables
                .entry((priority, bank.to_owned()))
                .or_insert_with(|| read_plugin_strings(&paths[priority], sources, bank));
            let id = <&[u8; 4]>::try_from(bytes.as_slice())
                .ok()
                .map(|value| u32::from_le_bytes(*value));
            let text = id
                .filter(|id| *id != 0)
                .and_then(|id| table.as_ref().ok()?.as_ref()?.values.get(&id))
                .cloned();
            record.localized_text.insert(ordinal, text.clone());
            if text.is_none() && id != Some(0) {
                let readable = table.as_ref().ok().and_then(|table| table.as_ref());
                let malformed = id.is_some_and(|id| {
                    readable.is_some_and(|table| table.malformed_ids.contains(&id))
                });
                let summaries = if id.is_some() && readable.is_some() && !malformed {
                    &mut gaps
                } else {
                    &mut losses
                };
                let entry = summaries.entry(priority).or_insert_with(|| {
                    (
                        0,
                        format!(
                            "{} source {:08X} {} localized ID {:?}",
                            String::from_utf8_lossy(&record.source.record_type),
                            record.source.form_id,
                            String::from_utf8_lossy(tag),
                            id
                        ),
                    )
                });
                entry.0 += 1;
            }
        }
    }
    for (priority, (count, first)) in losses {
        conversion.warnings.push(format!(
            "warning: {}: omitted {count} unusable localized fields (first: {first})",
            paths[priority]
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        ));
    }
    for (priority, (count, first)) in gaps {
        conversion.notices.push(format!(
            "notice: {}: {count} localized IDs missing from readable tables (first: {first})",
            paths[priority]
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        ));
    }
    Ok(())
}

/// Semantic identity columns; supplier indices, row ordinals, coordinates and scalar IDs are absent.
pub fn identity_column(table: &str, column: &str) -> bool {
    let columns = match table {
        "records" => "form_id cell_id worldspace_id",
        "formid_map" => "form_id",
        "worldspaces" => "id parent_world",
        "cells" => "id worldspace_id",
        "references" => "id cell_id worldspace_id base_form_id enable_parent_id emittance_id",
        "exterior_spatial" => "id cell_id worldspace_id",
        "land" => "cell_id",
        "statics" | "texture_sets" | "grass_types" | "movement_types" | "waters"
        | "image_spaces" | "lighting_templates" | "climates" => "id",
        "lights" => "id sound_id lens_id",
        "emittance" => "id weather_id",
        "npcs" => "id race_id class_id",
        "landscape_textures" => "id texture_set_id material_type",
        "landscape_texture_grasses" => "ltex_id gras_id",
        "actor_models" => "id race_id skin_armo_id",
        "actor_model_parts" => "npc_id armature_race_id skin_texture_set_id",
        "race_movement_links" => "id walk_movt_id run_movt_id",
        "race_animation_sources" => "race_id",
        "movement_game_settings" => "id",
        "base_texture_swaps" => "base_id texture_set_id",
        "scripts" | "script_properties" => "form_id",
        "script_property_values" => "form_id object_form_id",
        "cell_environment" => {
            "cell_id image_space_id acoustic_space_id music_id sky_region_id location_id encounter_zone_id owner_id water_id"
        }
        "cell_lighting" => "cell_id lighting_template_id image_space_id",
        "cell_regions" => "cell_id region_id",
        "worldspace_environment" => {
            "worldspace_id climate_id water_id lod_water_id lighting_template_id music_id location_id encounter_zone_id"
        }
        "weathers" => {
            "id precipitation_id visual_effect_id sky_static_id sun_glare_lens_id image_space_sunrise_id image_space_day_id image_space_sunset_id image_space_night_id volumetric_lighting_sunrise_id volumetric_lighting_day_id volumetric_lighting_sunset_id volumetric_lighting_night_id"
        }
        "weather_colors" | "weather_directional_ambient" => "weather_id",
        "climate_weathers" => "climate_id weather_id chance_global_id",
        "region_weathers" => "region_id weather_id chance_global_id",
        "door_links" => "ref_id destination_ref_id destination_cell_id destination_worldspace_id",
        "reference_linked_refs" => "reference_id keyword_id linked_ref_id",
        "reference_activate_parents" => "reference_id parent_ref_id",
        "reference_details" => {
            "reference_id owner_id lock_key_id location_id location_ref_type_id encounter_zone_id attached_ref_id"
        }
        "room_bounds" => "ref_id cell_id lighting_template_id image_space_id",
        "room_portals" => "ref_id cell_id room_a room_b",
        "room_links" => "room_ref linked_ref",
        "reference_rooms" => "reference_id room_ref",
        "projection_omission_samples" => "form_id",
        "lod" => "cell_id",
        "lod_chunks" | "lod_chunks_spatial" => "worldspace_id",
        _ => "",
    };
    columns.split_ascii_whitespace().any(|name| name == column)
}

/// All inputs belong to the same frozen source and identity generation.
struct WorldSnapshot<'a, 'b> {
    projected: &'b Connection,
    stage: &'b ProjectionStage<'a>,
    conversion: &'a FormKeyConversion,
    facts: &'b HashMap<FormKey, SourceFact>,
    paths: &'b [PathBuf],
    schemas: &'b RuntimeSchemas,
    typed: Option<&'b records::native::NativeReadResult>,
    mode: super::inhouse::RecordBundleMode,
}

/// Publish actual keys and evidence once, with all owned tables in one rollback boundary.
fn write_world(output: &mut Connection, snapshot: WorldSnapshot<'_, '_>) -> Result<Vec<String>> {
    let WorldSnapshot {
        projected,
        stage,
        conversion,
        facts,
        paths,
        schemas,
        typed,
        mode,
    } = snapshot;
    let identity = conversion.order.pack_identity()?;
    let definitions=projected.prepare("SELECT name,sql FROM sqlite_schema WHERE sql IS NOT NULL AND type='table' AND name NOT LIKE 'sqlite_%' AND name NOT IN (SELECT name FROM pragma_table_list WHERE type='shadow') ORDER BY name")?.query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let tx = output.transaction()?;
    for table in [
        "land_context",
        "record_text",
        "resolved_links",
        "record_field_origins",
        "record_source_fields",
        "record_context",
        "canonical_record_contract",
        "pack_identity",
        "pack_plugins",
    ] {
        tx.execute_batch(&format!("DROP TABLE IF EXISTS \"{table}\""))?;
    }
    super::exporter::replace_complete_snapshot(&tx)?;
    for (name, sql) in &definitions {
        if name != "schema_info" {
            tx.execute_batch(&format!("DROP TABLE IF EXISTS \"{name}\"; {sql}"))?;
        } else {
            tx.execute("DELETE FROM schema_info", [])?;
        }
    }
    let indexes = projected
        .prepare(
            "SELECT sql FROM sqlite_schema WHERE type='index' AND sql IS NOT NULL ORDER BY name",
        )?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for sql in indexes {
        tx.execute_batch(&sql)?;
    }
    shared::records::create_record_context(&tx, &identity)?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS inhouse_source_records(form_id INTEGER PRIMARY KEY,source_form_id INTEGER NOT NULL,load_order INTEGER NOT NULL,record_type TEXT NOT NULL,flags INTEGER NOT NULL,payload BLOB NOT NULL); DELETE FROM inhouse_source_records;
       CREATE TABLE IF NOT EXISTS inhouse_localized_fields(form_id INTEGER NOT NULL,field_index INTEGER NOT NULL,signature TEXT NOT NULL,field_name TEXT NOT NULL,string_table TEXT NOT NULL,string_id INTEGER NOT NULL,load_order INTEGER NOT NULL,resolved_text TEXT,status TEXT NOT NULL,PRIMARY KEY(form_id,field_index)); DELETE FROM inhouse_localized_fields;
       CREATE TABLE IF NOT EXISTS inhouse_terrain_source_order(form_id INTEGER PRIMARY KEY,source_record_offset INTEGER NOT NULL); DELETE FROM inhouse_terrain_source_order;")?;
    let mut projection_warnings = Vec::new();
    for (table, _) in &definitions {
        if matches!(table.as_str(), "records" | "schema_info" | "plugins") {
            continue;
        }
        if mode == super::inhouse::RecordBundleMode::Full || table == "formid_map" {
            let skipped = copy_projection(&tx, projected, table, stage, &identity)?;
            if skipped != 0 {
                projection_warnings.push(format!(
                    "warning: omitted {skipped} {table} rows with a null required semantic key"
                ));
            }
        }
    }
    let mut ordered = conversion
        .records
        .iter()
        .map(|(key, record)| Ok((conversion.order.identity_sort_tuple(*key)?, *key, record)))
        .collect::<Result<Vec<_>>>()?;
    ordered.sort_unstable_by_key(|entry| entry.0);
    for (_, key, record) in ordered {
        let canonical = record.canonical_record();
        let blob = super::extractors::serialize_subrecords(&canonical.subrecords);
        tx.execute(
            "INSERT INTO records VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                key.to_sql_i64(),
                String::from_utf8_lossy(&record.source.record_type),
                record.cell.map(FormKey::to_sql_i64),
                record.worldspace.map(FormKey::to_sql_i64),
                record.source.load_order,
                blob
            ],
        )?;
        #[cfg(test)]
        super::publication_provenance_fault_point()?;
        let supplier = identity.order()[record.source.load_order as usize];
        tx.execute(
            "INSERT INTO record_context VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                key.to_sql_i64(),
                supplier.get(),
                record.source.form_id,
                record.source.cell_form_id,
                record.source.worldspace_form_id,
                record.source.subrecords.len() as i64
            ],
        )?;
        for (ordinal, (signature, bytes)) in record.source.subrecords.iter().enumerate() {
            tx.execute(
                "INSERT INTO record_source_fields VALUES(?1,?2,?3,?4)",
                params![key.to_sql_i64(), ordinal as i64, signature, bytes],
            )?;
        }
        for (ordinal, (signature, _)) in canonical.subrecords.iter().enumerate() {
            let source = record.source_ordinal(ordinal)?;
            tx.execute(
                "INSERT INTO record_field_origins VALUES(?1,?2,?3,?4,?5)",
                params![
                    key.to_sql_i64(),
                    ordinal as i64,
                    source as i64,
                    signature,
                    !record.runtime_omitted.contains(&source)
                ],
            )?;
        }
        for link in &record.links {
            if (0..canonical.subrecords.len())
                .any(|ordinal| record.source_ordinal(ordinal).ok() == Some(link.subrecord))
            {
                tx.execute(
                    "INSERT INTO resolved_links VALUES(?1,?2,?3,?4,?5,?6)",
                    params![
                        key.to_sql_i64(),
                        link.subrecord as i64,
                        link.byte_offset as i64,
                        link.source_word,
                        link.key.map(FormKey::to_sql_i64),
                        link.outcome.as_str()
                    ],
                )?;
            }
        }
        for (&ordinal, text) in &record.localized_text {
            tx.execute(
                "INSERT INTO record_text VALUES(?1,?2,?3)",
                params![key.to_sql_i64(), ordinal as i64, text],
            )?;
            if let Some((name, Some(bank), Some(id))) = facts[&key].text_fields.get(&ordinal) {
                tx.execute(
                    "INSERT INTO inhouse_localized_fields VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                    params![
                        key.to_sql_i64(),
                        ordinal as i64,
                        String::from_utf8_lossy(&record.source.subrecords[ordinal].0),
                        name,
                        bank,
                        id,
                        record.source.load_order,
                        text,
                        if text.is_some() {
                            "resolved"
                        } else if *id == 0 {
                            "null_id"
                        } else {
                            "missing"
                        }
                    ],
                )?;
            }
        }
        let fact = facts
            .get(&key)
            .ok_or_else(|| eyre!("missing source provenance"))?;
        tx.execute(
            "INSERT INTO inhouse_source_records VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                key.to_sql_i64(),
                record.source.form_id,
                record.source.load_order,
                String::from_utf8_lossy(&record.source.record_type),
                record.source.flags,
                fact.payload
            ],
        )?;
        if record.source.record_type == *b"LAND" {
            tx.execute(
                "INSERT INTO inhouse_terrain_source_order VALUES(?1,?2)",
                params![key.to_sql_i64(), fact.offset],
            )?;
        }
    }
    super::inhouse_terrain_diagnostics::replace_snapshot_tables(&tx)?;
    for (&key, record) in &conversion.records {
        if record.source.record_type != *b"LAND" || record.source.is_deleted() {
            continue;
        }
        let layers = super::cell_cache::form_key_source_texture_layers(record, &conversion.order)?;
        tx.execute(
            "INSERT INTO inhouse_terrain_layers VALUES(?1,?2,?3,?4)",
            params![
                key.to_sql_i64(),
                record.cell.unwrap_or(key).to_sql_i64(),
                record.source.load_order,
                rkyv::to_bytes::<rkyv::rancor::Error>(&layers)?.as_slice()
            ],
        )?;
        for texture in layers
            .iter()
            .map(|layer| layer.texture_form_id)
            .filter(|id| *id != 0)
            .collect::<BTreeSet<_>>()
        {
            let texture_key = shared::cache_identity::required(&identity, texture)?;
            if !conversion.records.get(&texture_key).is_some_and(|target| {
                target.source.record_type == *b"LTEX" && !target.source.is_deleted()
            }) {
                tx.execute(
                    "INSERT OR IGNORE INTO inhouse_unresolved_terrain_textures VALUES(?1,?2)",
                    params![key.to_sql_i64(), texture_key.to_sql_i64()],
                )?;
            }
        }
    }
    let source_offsets = facts
        .iter()
        .map(|(key, fact)| (*key, fact.offset))
        .collect();
    let (lands, _) = super::cell_cache::native_land_winners(conversion, &source_offsets)?;
    for (cell, land) in lands {
        tx.execute(
            "INSERT INTO land_context VALUES(?1,?2)",
            params![cell.to_sql_i64(), land.to_sql_i64()],
        )?;
    }
    super::insert_plugins(&tx, paths)?;
    if let Some(read) = typed {
        records::history::write_native_tables(&tx, read, &identity, paths, schemas)?;
        records::diagnostics::write_native_tables(&tx, read, &identity)?;
    }
    tx.execute("DELETE FROM schema_info", [])?;
    tx.execute(
        "INSERT INTO schema_info VALUES(?1)",
        [shared::WORLD_DATABASE_SCHEMA_VERSION],
    )?;
    tx.commit()?;
    Ok(projection_warnings)
}

/// These diagnostic projections deliberately use zero when their target is unavailable.
/// Their row owner is still required; the reason/resolution makes the absent target explicit.
fn preserves_zero_target(table: &str, column: &str) -> bool {
    matches!(
        (table, column),
        ("base_texture_swaps", "texture_set_id") | ("actor_models", "race_id")
    )
}

/// Copy scalar results with checked identity bindings; compact indices never enter the pack.
fn copy_projection(
    tx: &Transaction<'_>,
    source: &Connection,
    table: &str,
    stage: &ProjectionStage<'_>,
    identity: &PackIdentity,
) -> Result<usize> {
    let columns = source
        .prepare(&format!("PRAGMA table_info(\"{table}\")"))?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, bool>(3)? || row.get::<_, i64>(5)? != 0,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let names = columns
        .iter()
        .map(|(name, _)| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(",");
    let placeholders = (1..=columns.len())
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(",");
    let mut select = source.prepare(&format!("SELECT {names} FROM \"{table}\""))?;
    let mut rows = select.query([])?;
    let mut insert = tx.prepare(&format!(
        "INSERT INTO \"{table}\"({names}) VALUES({placeholders})"
    ))?;
    let mut skipped = 0;
    while let Some(row) = rows.next()? {
        let mut values = (0..columns.len())
            .map(|index| row.get::<_, SqlValue>(index))
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let owner_index = columns.iter().position(|(name, _)| {
            matches!(name.as_str(), "id" | "form_id" | "reference_id" | "cell_id")
        });
        let owner = owner_index.and_then(|index| match values[index] {
            SqlValue::Integer(value) => u32::try_from(value).ok(),
            _ => None,
        });
        let mut unusable = false;
        for (index, (column, required)) in columns.iter().enumerate() {
            if identity_column(table, column) {
                match values[index] {
                    SqlValue::Null => {}
                    SqlValue::Integer(value) => {
                        let handle = u32::try_from(value)
                            .map_err(|_| eyre!("invalid CPU projection index"))?;
                        let key = stage.key(handle)?;
                        if key.is_none() && *required {
                            if preserves_zero_target(table, column) {
                                // Zero is an explicit diagnostic sentinel, never an owner key.
                                values[index] = SqlValue::Integer(0);
                                continue;
                            }
                            unusable = true;
                            break;
                        }
                        values[index] =
                            key.map_or(SqlValue::Null, |key| SqlValue::Integer(key.to_sql_i64()));
                    }
                    _ => return Err(eyre!("invalid identity storage class in {table}.{column}")),
                }
            }
            if matches!(
                (table, column.as_str()),
                ("cells", "data") | ("references", "data") | ("waters", "data")
            ) {
                if let Some(handle) = owner {
                    values[index] = SqlValue::Blob(super::extractors::serialize_subrecords(
                        &stage.original(handle)?.canonical_record().subrecords,
                    ));
                }
            }
            if table == "scripts" && column == "vmad" {
                if let Some(handle) = owner {
                    values[index] = SqlValue::Blob(
                        stage
                            .original(handle)?
                            .canonical_record()
                            .subrecords
                            .iter()
                            .find(|(tag, _)| tag == b"VMAD")
                            .map(|(_, bytes)| bytes.clone())
                            .unwrap_or_default(),
                    );
                }
            }
            if table == "land" && column == "vtex" {
                if let Some(handle) = owner {
                    let record = stage.master.values().find(|record| {
                        record.record_type == *b"LAND" && record.cell_form_id == Some(handle)
                    });
                    if let Some(record) = record {
                        values[index] = stage
                            .original(record.form_id)?
                            .canonical_record()
                            .subrecords
                            .iter()
                            .find(|(tag, _)| tag == b"VTEX")
                            .map_or(SqlValue::Null, |(_, bytes)| SqlValue::Blob(bytes.clone()));
                    }
                }
            }
        }
        if unusable {
            skipped += 1;
            continue;
        }
        if table == "projection_omission_samples" {
            for (index, (name, _)) in columns.iter().enumerate() {
                if name == "form_id" {
                    if let SqlValue::Integer(value) = values[index] {
                        identity.required_key(value)?;
                    }
                }
            }
        }
        insert.execute(rusqlite::params_from_iter(values))?;
    }
    Ok(skipped)
}

#[cfg(test)]
mod projection_copy_tests {
    use super::*;
    use crate::esm::records::RawRecord;
    use shared::plugin_table::PluginFacts;

    /// One authenticated source record gives the SQL copier a nonidentity CPU index.
    fn fixture() -> (FormKeyConversion, FormKey) {
        let order = FormKeyOrder::from_facts(vec![(
            "Fixture.esm".into(),
            PluginFacts {
                content_hash: Some([0x5A; 32]),
                ..Default::default()
            },
        )])
        .unwrap();
        let key = FormKey::new(order.table().id("Fixture.esm").unwrap(), 0x1234).unwrap();
        let record = FormKeyRecord {
            source: RawRecord {
                form_id: 0x1234,
                record_type: *b"STAT",
                flags: 0,
                subrecords: Vec::new(),
                cell_form_id: None,
                worldspace_form_id: None,
                load_order: 0,
            },
            canonical: None,
            canonical_ordinals: Vec::new(),
            localized_text: Default::default(),
            runtime_omitted: Default::default(),
            cell: None,
            worldspace: None,
            links: Vec::new(),
        };
        (
            FormKeyConversion {
                order,
                records: [(key, record)].into(),
                warnings: Vec::new(),
                notices: Vec::new(),
            },
            key,
        )
    }

    /// Missing materials and unresolved actor races retain their owner and diagnostic state.
    #[test]
    fn diagnostic_zero_targets_preserve_material_and_unresolved_actor_rows() {
        let (conversion, key) = fixture();
        let stage = ProjectionStage::build(&conversion).unwrap();
        let identity = conversion.order.pack_identity().unwrap();
        let handle = stage.handle(key).unwrap();
        assert_ne!(u64::from(handle), key.to_packed());
        let source = Connection::open_in_memory().unwrap();
        let mut output = Connection::open_in_memory().unwrap();
        let schema = "CREATE TABLE base_texture_swaps(base_id INTEGER NOT NULL,position INTEGER NOT NULL,shape_name TEXT NOT NULL,shape_index INTEGER NOT NULL,texture_set_id INTEGER NOT NULL,disposition TEXT NOT NULL,PRIMARY KEY(base_id,position));
            CREATE TABLE actor_models(id INTEGER PRIMARY KEY,race_id INTEGER NOT NULL,resolution TEXT NOT NULL);";
        source.execute_batch(schema).unwrap();
        output.execute_batch(schema).unwrap();
        source.execute("INSERT INTO base_texture_swaps VALUES(?1,0,'ArchiveShape',0,0,'missing_texture_set')",[handle]).unwrap();
        source
            .execute(
                "INSERT INTO actor_models VALUES(?1,0,'unresolved')",
                [handle],
            )
            .unwrap();
        let tx = output.transaction().unwrap();
        for table in ["base_texture_swaps", "actor_models"] {
            assert_eq!(
                copy_projection(&tx, &source, table, &stage, &identity).unwrap(),
                0
            );
        }
        let material: (i64, u32, String, i32, u32, String) = tx
            .query_row("SELECT * FROM base_texture_swaps", [], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })
            .unwrap();
        assert_eq!(
            material,
            (
                key.to_sql_i64(),
                0,
                "ArchiveShape".into(),
                0,
                0,
                "missing_texture_set".into()
            )
        );
        let actor: (i64, u32, String) = tx
            .query_row("SELECT * FROM actor_models", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .unwrap();
        assert_eq!(actor, (key.to_sql_i64(), 0, "unresolved".into()));
    }

    /// Diagnostic exceptions never turn owner/edge zero into a key or change nullable targets.
    #[test]
    fn zero_owners_and_required_edges_still_omit_while_nullable_targets_become_null() {
        let (conversion, key) = fixture();
        let stage = ProjectionStage::build(&conversion).unwrap();
        let identity = conversion.order.pack_identity().unwrap();
        let handle = stage.handle(key).unwrap();
        let source = Connection::open_in_memory().unwrap();
        let mut output = Connection::open_in_memory().unwrap();
        let schema = "CREATE TABLE base_texture_swaps(base_id INTEGER NOT NULL,texture_set_id INTEGER NOT NULL);
            CREATE TABLE door_links(ref_id INTEGER PRIMARY KEY,destination_ref_id INTEGER NOT NULL);
            CREATE TABLE reference_details(reference_id INTEGER PRIMARY KEY,owner_id INTEGER);";
        source.execute_batch(schema).unwrap();
        output.execute_batch(schema).unwrap();
        source
            .execute("INSERT INTO base_texture_swaps VALUES(0,0)", [])
            .unwrap();
        source
            .execute("INSERT INTO door_links VALUES(?1,0)", [handle])
            .unwrap();
        source
            .execute("INSERT INTO reference_details VALUES(?1,0)", [handle])
            .unwrap();
        let tx = output.transaction().unwrap();
        for table in ["base_texture_swaps", "door_links"] {
            assert_eq!(
                copy_projection(&tx, &source, table, &stage, &identity).unwrap(),
                1
            );
            assert_eq!(
                tx.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row
                    .get::<_, u32>(0))
                    .unwrap(),
                0
            );
        }
        assert_eq!(
            copy_projection(&tx, &source, "reference_details", &stage, &identity).unwrap(),
            0
        );
        let row: (i64, Option<i64>) = tx
            .query_row(
                "SELECT reference_id,owner_id FROM reference_details",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(row, (key.to_sql_i64(), None));
    }
}
