//! Independent permissive legacy packing versus the Mutagen-inspired FormKey contract.
//! Strict old record policies are tested with the retained real remapper in esm's unit tests.
//! Synthetic fixtures are generated in-process; the real-input audit is explicitly opt-in.
use converter::esm::binary::{parse_plugin_file, parse_plugin_metadata};
use sha2::{Digest, Sha256};
use shared::{
    form_key::FormKey,
    legacy_id_view::LegacyIdView,
    plugin_table::{PluginFacts, PluginFlags, PluginTable},
    source_form_id::{Resolution, SourceFormId, ZeroPolicy, resolve},
};
use std::collections::BTreeMap;

#[derive(Clone)]
struct Plugin {
    name: String,
    light: bool,
    masters: Vec<String>,
}

/// Allocate pack-local plugin IDs independently of the supplied legacy slot order.
fn table(plugins: &[Plugin]) -> PluginTable {
    PluginTable::fresh(
        plugins
            .iter()
            .map(|p| {
                (
                    p.name.clone(),
                    PluginFacts {
                        masters: p.masters.clone(),
                        flags: PluginFlags {
                            light: p.light,
                            ..Default::default()
                        },
                        header_version: Some(1.71),
                        ..Default::default()
                    },
                )
            })
            .collect(),
    )
    .unwrap()
}

/// Frozen old permissive header packer: source selectors address MAST, then self.
/// Scope: representable, valid-width identities only, not strict headers or link recovery.
/// This oracle never calls resolve, FormKey, LegacyIdView, or converter remapping.
fn old_pack(plugins: &[Plugin], supplier: usize, raw: u32) -> Option<u32> {
    if raw == 0 {
        return Some(0);
    }
    let p = &plugins[supplier];
    let name = p.masters.get((raw >> 24) as usize).unwrap_or(&p.name);
    let owner = plugins
        .iter()
        .position(|p| p.name.eq_ignore_ascii_case(name))?;
    let light = plugins[owner].light;
    let slot = plugins[..owner].iter().filter(|p| p.light == light).count();
    let local = raw & 0xffffff;
    if light {
        (slot < 4096 && local <= 0xfff).then_some(0xfe000000 | ((slot as u32) << 12) | local)
    } else {
        (slot < 254).then_some(((slot as u32) << 24) | local)
    }
}

/// Exercise the shared identity contract separately from the converter's resolver.
fn key(t: &PluginTable, p: &Plugin, raw: u32) -> Resolution {
    resolve(
        t,
        SourceFormId {
            raw,
            containing_plugin: t.id(&p.name).unwrap(),
        },
        ZeroPolicy::Null,
    )
}

/// Compare independent packed winner bytes against shared and production native identities.
fn compare(plugins: &[Plugin], records: &[(usize, u32, u32)]) -> Vec<u8> {
    let t = table(plugins);
    let converter_order = converter::esm::form_keys::FormKeyOrder::from_facts(
        plugins
            .iter()
            .map(|p| {
                (
                    p.name.clone(),
                    t.entry(t.id(&p.name).unwrap()).unwrap().facts.clone(),
                )
            })
            .collect(),
    )
    .unwrap();
    let order: Vec<_> = plugins.iter().map(|p| t.id(&p.name).unwrap()).collect();
    let view = LegacyIdView::from_load_order(&t, &order).unwrap();
    let mut old = BTreeMap::new();
    let mut new = BTreeMap::<FormKey, u32>::new();
    for &(supplier, raw, payload) in records {
        let expected = old_pack(plugins, supplier, raw).unwrap();
        let resolved = key(&t, &plugins[supplier], raw).key();
        let converter_key = converter_order
            .resolve(supplier, raw, ZeroPolicy::Null)
            .unwrap();
        assert_eq!(converter_key, resolved);
        let actual = converter_key.map_or(0, |k| {
            converter_order
                .legacy_view()
                .unwrap()
                .to_legacy_u32(k)
                .unwrap()
        });
        assert_eq!(actual, expected, "{}:{raw:08X}", plugins[supplier].name);
        assert_eq!(view.from_legacy_u32(actual).unwrap(), resolved);
        if expected != 0 {
            old.insert(expected, payload);
        }
        if let Some(k) = resolved {
            new.insert(k, payload);
        }
    }
    let new: BTreeMap<_, _> = new
        .into_iter()
        .map(|(k, v)| (view.to_legacy_u32(k).unwrap(), v))
        .collect();
    assert_eq!(
        new, old,
        "last supplier must win even when table-id order differs"
    );
    let encode = |rows: BTreeMap<u32, u32>| {
        rows.into_iter()
            .flat_map(|(id, v)| [id.to_le_bytes(), v.to_le_bytes()].concat())
            .collect::<Vec<_>>()
    };
    let bytes = encode(old);
    assert_eq!(encode(new), bytes);
    bytes
}

#[test]
/// Asymmetric master indices and interleaved owners retain overrides and null semantics.
fn synthetic_nonidentity_masters_lights_overrides_and_null() {
    let plugins = vec![
        Plugin {
            name: "Z.esm".into(),
            light: false,
            masters: vec![],
        },
        Plugin {
            name: "L.esp".into(),
            light: true,
            masters: vec!["Z.esm".into()],
        },
        Plugin {
            name: "A.esp".into(),
            light: false,
            masters: vec!["L.esp".into(), "Z.esm".into()],
        },
    ];
    let bytes = compare(
        &plugins,
        &[
            (0, 0x800, 11),
            (1, 0x01000fff, 12),
            (2, 0x01000800, 99),
            (2, 0x00000fff, 88),
            (2, 0x02000801, 77),
            (2, 0, 66),
            (0, 0x00ffffff, 55),
        ],
    );
    assert_eq!(&bytes[..8], &[0, 8, 0, 0, 99, 0, 0, 0]);
}

#[test]
/// Missing owners and wide light locals remain distinct from usable neighboring identities.
fn malformed_master_and_light_width_are_distinguished_from_valid_neighbors() {
    let plugins = vec![
        Plugin {
            name: "Base.esm".into(),
            light: false,
            masters: vec![],
        },
        Plugin {
            name: "Patch.esl".into(),
            light: true,
            masters: vec!["Absent.esm".into()],
        },
    ];
    let t = table(&plugins);
    assert!(matches!(
        key(&t, &plugins[1], 0x800),
        Resolution::UnknownOwner(_)
    ));
    assert!(matches!(
        key(&t, &plugins[1], 0x01001000),
        Resolution::InvalidWidth { .. }
    ));
    let native = converter::esm::form_keys::FormKeyOrder::from_facts(vec![
        ("Base.esm".into(), PluginFacts::default()),
        (
            "Patch.esl".into(),
            PluginFacts {
                masters: vec!["Base.esm".into()],
                ..Default::default()
            },
        ),
    ])
    .unwrap();
    assert!(
        native
            .resolve(1, 0x7f000801, ZeroPolicy::Null)
            .unwrap()
            .is_some()
    );
    // The converter and untouched step-1 contract both select self beyond MAST.
    assert!(matches!(
        key(&t, &plugins[1], 0x7f000801),
        Resolution::SelfProjected { .. }
    ));
    assert!(key(&t, &plugins[0], 0x801).key().is_some());
    assert!(key(&t, &plugins[1], 0x01000802).key().is_some());
}

#[test]
/// Every native owner remains addressable when only checked legacy packing exceeds capacity.
fn synthetic_exact_and_exceeded_full_and_light_limits() {
    for (full, light) in [(254, 4096), (255, 4097)] {
        let plugins: Vec<_> = (0..full + light)
            .map(|n| Plugin {
                name: format!("P{n:05}.{}", if n < full { "esm" } else { "esl" }),
                light: n >= full,
                masters: vec![],
            })
            .collect();
        let t = table(&plugins);
        let order: Vec<_> = plugins.iter().map(|p| t.id(&p.name).unwrap()).collect();
        let view = LegacyIdView::from_load_order(&t, &order).unwrap();
        let mut resolved = 0;
        let mut refused = 0;
        for (i, p) in plugins.iter().enumerate() {
            let k = key(&t, p, 0xabc).key().unwrap();
            resolved += 1;
            match old_pack(&plugins, i, 0xabc) {
                Some(expected) => {
                    assert_eq!(view.to_legacy_u32(k), Ok(expected));
                    assert_eq!(view.from_legacy_u32(expected), Ok(Some(k)));
                }
                None => {
                    assert!(view.to_legacy_u32(k).is_err());
                    refused += 1;
                }
            }
        }
        assert_eq!(resolved, full + light);
        assert_eq!(refused, usize::from(full > 254) + usize::from(light > 4096));
    }
}

/// Interleaved light owners and alphabetic table allocation must retain unsigned Skyrim order.
#[test]
fn explicit_owner_priority_order_matches_literal_legacy_ids() {
    use converter::esm::form_keys::FormKeyOrder;
    let order = FormKeyOrder::from_facts(
        ["Z.esm", "A.esl", "C.esp", "Y.esl", "B.esp"]
            .map(|name| (name.to_owned(), PluginFacts::default()))
            .to_vec(),
    )
    .unwrap();
    let mut keys = [4, 3, 2, 1, 0].map(|supplier| {
        order
            .resolve(supplier, 0x812, ZeroPolicy::Identity)
            .unwrap()
            .unwrap()
    });
    keys.sort_unstable_by_key(|key| order.identity_sort_tuple(*key).unwrap());
    assert_eq!(
        keys[0].plugin().get(),
        5,
        "alphabetical registry allocation is not owner priority"
    );
    let actual = keys.map(|key| order.legacy_view().unwrap().to_legacy_u32(key).unwrap());
    assert_eq!(
        actual,
        [0x812, 0x0100_0812, 0x0200_0812, 0xfe00_0812, 0xfe00_1812]
    );
}

/// Encode synthetic native bytes independently of converter identity resolution.
fn native_record(tag: &[u8; 4], id: u32, flags: u32, body: &[u8]) -> Vec<u8> {
    [
        tag.as_slice(),
        &(body.len() as u32).to_le_bytes(),
        &flags.to_le_bytes(),
        &id.to_le_bytes(),
        &[0; 8],
        body,
    ]
    .concat()
}

/// Frame an authored field; fixtures use asymmetric IDs and nonalphabetical priorities.
fn native_field(tag: &[u8; 4], bytes: &[u8]) -> Vec<u8> {
    [tag.as_slice(), &(bytes.len() as u16).to_le_bytes(), bytes].concat()
}

/// Frame a contextual CELL child group without using converter remapping.
fn cell_group(label: u32, records: &[u8]) -> Vec<u8> {
    [
        b"GRUP".as_slice(),
        &((records.len() + 24) as u32).to_le_bytes(),
        &label.to_le_bytes(),
        &6i32.to_le_bytes(),
        &[0; 8],
        records,
    ]
    .concat()
}

/// One format-2 VMAD object with distinct alias and padding sentinels.
fn script_object(raw: u32) -> Vec<u8> {
    let mut bytes = [5i16.to_le_bytes(), 2i16.to_le_bytes(), 1u16.to_le_bytes()].concat();
    bytes.extend(11u16.to_le_bytes());
    bytes.extend(b"Independent");
    bytes.push(0);
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(6u16.to_le_bytes());
    bytes.extend(b"Object");
    bytes.extend([1, 1, 0xa1, 0xb2, 0x56, 0x34]);
    bytes.extend(raw.to_le_bytes());
    bytes
}

/// Read every SQLite schema object and every table row, including virtual shadow tables.
fn logical_dump(conn: &rusqlite::Connection) -> Vec<String> {
    use rusqlite::types::Value;
    let schema: Vec<(String, String, Option<String>)> = conn
        .prepare("SELECT type,name,sql FROM sqlite_master ORDER BY type,name,sql")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let mut dump = Vec::new();
    for (kind, name, sql) in schema {
        dump.push(format!("schema {kind} {name} {sql:?}"));
        if kind != "table" {
            continue;
        }
        let mut query = conn
            .prepare(&format!("SELECT * FROM \"{}\"", name.replace('"', "\"\"")))
            .unwrap();
        let count = query.column_count();
        let mut rows: Vec<String> = query
            .query_map([], |row| {
                Ok(format!(
                    "{:?}",
                    (0..count)
                        .map(|i| row.get::<_, Value>(i))
                        .collect::<rusqlite::Result<Vec<_>>>()?
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        rows.sort();
        dump.extend(rows.into_iter().map(|row| format!("{name} {row}")));
    }
    dump
}

/// Independent inverse of fresh native storage: source owner names select the frozen packer.
/// Neither the production resolver nor its runtime-slot adapter supplies expected packed IDs.
fn old_key(identity: &shared::pack_identity::PackIdentity, plugins: &[Plugin], stored: i64) -> u32 {
    let key = identity.required_key(stored).unwrap();
    let owner = identity.table().entry(key.plugin()).unwrap();
    let supplier = plugins
        .iter()
        .position(|p| p.name.eq_ignore_ascii_case(&owner.display_name))
        .unwrap();
    old_pack(
        plugins,
        supplier,
        ((plugins[supplier].masters.len() as u32) << 24) | key.local(),
    )
    .unwrap()
}

/// Closed identity-column audit for every table emitted by the inherited scalar writer.
/// Supplier indices, row positions, coordinates and non-FormID scalar integers stay unchanged.
fn identity_columns(table: &str) -> &'static str {
    match table {
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
        "scripts" => "form_id",
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
        // RTree auxiliary columns live in its rowid shadow, not in node payloads.
        // Exterior rowids are reference FormKeys; LOD rowids are independent index IDs.
        "exterior_spatial_rowid" => "rowid a0 a1",
        "lod_chunks_spatial_rowid" => "a0",
        "schema_info"
        | "plugins"
        | "conversion_cache"
        | "room_graph_stats"
        | "projection_omission_stats"
        | "lod_build"
        | "exterior_spatial_node"
        | "exterior_spatial_parent"
        | "lod_chunks_spatial_node"
        | "lod_chunks_spatial_parent" => "",
        other => panic!("unreviewed scalar table {other}; extend the closed differential contract"),
    }
}

fn rows(conn: &rusqlite::Connection, table: &str) -> Vec<Vec<rusqlite::types::Value>> {
    let mut select = conn.prepare(&format!("SELECT * FROM \"{table}\"")).unwrap();
    let width = select.column_count();
    select
        .query_map([], |row| (0..width).map(|i| row.get(i)).collect())
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// Independently audit both inherited RTrees and their exact empty physical representation.
/// SQLite's rowid shadows store auxiliary values; node/parent numbers are tree-local IDs.
/// The fixture has only an interior CELL, so neither tree may contain an indexed row.
fn assert_empty_spatial_contract(conn: &rusqlite::Connection) {
    use rusqlite::types::Value;
    for (name, ddl) in [
        (
            "exterior_spatial",
            "CREATE VIRTUAL TABLE exterior_spatial USING rtree(id,minX,maxX,minY,maxY,minZ,maxZ,+cell_id,+worldspace_id)",
        ),
        (
            "lod_chunks_spatial",
            "CREATE VIRTUAL TABLE lod_chunks_spatial USING rtree(id,minX,maxX,minY,maxY,+worldspace_id,+tier,+anchor_x,+anchor_y)",
        ),
        (
            "exterior_spatial_node",
            "CREATE TABLE \"exterior_spatial_node\"(nodeno INTEGER PRIMARY KEY,data)",
        ),
        (
            "exterior_spatial_parent",
            "CREATE TABLE \"exterior_spatial_parent\"(nodeno INTEGER PRIMARY KEY,parentnode)",
        ),
        (
            "exterior_spatial_rowid",
            "CREATE TABLE \"exterior_spatial_rowid\"(rowid INTEGER PRIMARY KEY,nodeno,a0,a1)",
        ),
        (
            "lod_chunks_spatial_node",
            "CREATE TABLE \"lod_chunks_spatial_node\"(nodeno INTEGER PRIMARY KEY,data)",
        ),
        (
            "lod_chunks_spatial_parent",
            "CREATE TABLE \"lod_chunks_spatial_parent\"(nodeno INTEGER PRIMARY KEY,parentnode)",
        ),
        (
            "lod_chunks_spatial_rowid",
            "CREATE TABLE \"lod_chunks_spatial_rowid\"(rowid INTEGER PRIMARY KEY,nodeno,a0,a1,a2,a3)",
        ),
    ] {
        let actual: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name=?1",
                [name],
                |row| row.get(0),
            )
            .unwrap();
        let compact = |sql: &str| {
            sql.chars()
                .filter(|ch| !ch.is_ascii_whitespace())
                .collect::<String>()
        };
        assert_eq!(
            compact(&actual),
            compact(ddl),
            "independent spatial DDL {name}"
        );
    }
    for name in [
        "exterior_spatial",
        "exterior_spatial_rowid",
        "exterior_spatial_parent",
        "lod",
        "lod_chunks",
        "lod_chunks_spatial",
        "lod_chunks_spatial_rowid",
        "lod_chunks_spatial_parent",
        "lod_build",
    ] {
        assert!(
            rows(conn, name).is_empty(),
            "interior-only fixture must leave {name} empty"
        );
    }
    let page_size: usize = conn
        .pragma_query_value(None, "page_size", |row| row.get(0))
        .unwrap();
    // Bundled SQLite 3: four header bytes, an eight-byte row ID and two f32 bounds
    // per dimension per cell, at most 51 cells, capped by page size minus 64 bytes.
    for (name, dimensions) in [("exterior_spatial_node", 3), ("lod_chunks_spatial_node", 2)] {
        let bytes = (page_size - 64).min(4 + 51 * (8 + 8 * dimensions));
        assert_eq!(
            rows(conn, name),
            vec![vec![Value::Integer(1), Value::Blob(vec![0; bytes])]],
            "independent empty root {name}"
        );
    }
}

/// Compare all inherited schema/rows, while proving each intentional native-format addition.
fn native_logical_dump(
    old: &rusqlite::Connection,
    new: &rusqlite::Connection,
    plugins: &[Plugin],
    authored: &std::collections::HashMap<u32, converter::esm::records::RawRecord>,
) -> Vec<String> {
    use rusqlite::{params, types::Value};
    let context = shared::records::RecordContextReader::new(new).unwrap();
    let identity = context.identity();
    assert_eq!(identity.order().len(), 3);
    assert_eq!(
        old.query_row("SELECT version FROM schema_info", [], |row| row
            .get::<_, u32>(0))
            .unwrap(),
        shared::WORLD_DATABASE_SCHEMA_VERSION,
        "the memory-only scalar oracle and current native writer use the same explicit floor"
    );
    assert_eq!(
        new.query_row("SELECT version FROM schema_info", [], |row| row
            .get::<_, u32>(0))
            .unwrap(),
        shared::WORLD_DATABASE_SCHEMA_VERSION
    );
    let mut normalized = BTreeMap::new();
    let mut sites = Vec::new();
    let keys = new
        .prepare("SELECT form_id FROM records ORDER BY form_id")
        .unwrap()
        .query_map([], |row| row.get::<_, i64>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(keys.len(), authored.len());
    for stored in keys {
        let packed = old_key(identity, plugins, stored);
        let source = &authored[&packed];
        let record = context
            .record(identity.required_key(stored).unwrap())
            .unwrap();
        assert_eq!(record.source_header, source.form_id);
        assert_eq!(
            identity.order()[source.load_order as usize],
            record.supplier
        );
        assert_eq!(record.source_cell, source.cell_form_id);
        assert_eq!(record.source_world, source.worldspace_form_id);
        assert_eq!(
            new.query_row(
                "SELECT source_field_count FROM record_context WHERE record_key=?1",
                [stored],
                |row| row.get::<_, usize>(0)
            )
            .unwrap(),
            source.subrecords.len()
        );
        assert_eq!(
            record.source_ordinals,
            (0..source.subrecords.len()).collect::<Vec<_>>()
        );
        let mut fields = Vec::new();
        for (ordinal, field) in record.subrecords.iter().enumerate() {
            assert!(record.is_runtime_usable(ordinal));
            assert_eq!(
                (field.tag.as_slice(), field.data.as_slice()),
                (
                    source.subrecords[ordinal].0.as_slice(),
                    source.subrecords[ordinal].1.as_slice()
                ),
                "original canonical bytes"
            );
            let original:(Vec<u8>,Vec<u8>)=new.query_row("SELECT signature,data FROM record_source_fields WHERE record_key=?1 AND source_ordinal=?2",params![stored,ordinal],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
            assert_eq!(original, (field.tag.to_vec(), field.data.clone()));
            let offsets=new.prepare("SELECT byte_offset,source_word FROM resolved_links WHERE record_key=?1 AND source_ordinal=?2 ORDER BY byte_offset").unwrap().query_map(params![stored,ordinal],|row|Ok((row.get::<_,usize>(0)?,row.get::<_,u32>(1)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
            let mut bytes = field.data.clone();
            for (offset, raw) in offsets {
                assert_eq!(
                    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()),
                    raw
                );
                let expected = old_pack(plugins, source.load_order as usize, raw).unwrap();
                let actual = record
                    .resolve_at(ordinal, offset)
                    .unwrap()
                    .map(|key| old_key(identity, plugins, key.to_sql_i64()))
                    .unwrap_or(0);
                assert_eq!(actual, expected);
                sites.push((packed, ordinal, offset, raw, expected));
                bytes[offset..offset + 4].copy_from_slice(&expected.to_le_bytes());
            }
            fields.push(shared::records::ArchivedSubRecord {
                tag: field.tag,
                data: bytes,
            });
        }
        let archived:(u32,u32,String,u32,Vec<u8>)=new.query_row("SELECT source_form_id,load_order,record_type,flags,payload FROM inhouse_source_records WHERE form_id=?1",[stored],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).unwrap();
        assert_eq!(
            archived,
            (
                source.form_id,
                source.load_order,
                String::from_utf8(source.record_type.to_vec()).unwrap(),
                source.flags,
                source
                    .subrecords
                    .iter()
                    .flat_map(|(tag, bytes)| native_field(
                        tag.as_slice().try_into().unwrap(),
                        bytes
                    ))
                    .collect::<Vec<_>>()
            )
        );
        normalized.insert(
            stored,
            shared::records::ArchivedRecordData { subrecords: fields },
        );
    }
    sites.sort();
    assert_eq!(
        sites,
        [
            (0x811, 1, 36, 0x812, 0xfe00_0812),
            (0x0100_0914, 0, 0, 0x0100_0811, 0x811),
            (0x0100_0914, 1, 0, 0x812, 0xfe00_0812)
        ]
    );
    let additions = [
        ("pack_identity", 1),
        ("pack_plugins", 3),
        ("canonical_record_contract", 1),
        ("record_context", 4),
        ("record_source_fields", 8),
        ("record_field_origins", 8),
        ("resolved_links", 3),
        ("record_text", 3),
        ("land_context", 0),
        ("inhouse_source_records", 4),
        ("inhouse_localized_fields", 0),
        ("inhouse_terrain_source_order", 0),
        ("inhouse_terrain_layers", 0),
        ("inhouse_unresolved_terrain_textures", 0),
        ("script_properties", 1),
        ("script_property_values", 1),
    ];
    for (table, count) in additions {
        assert_eq!(
            new.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row
                .get::<_, usize>(0))
                .unwrap(),
            count,
            "{table}"
        );
    }
    // Every text sidecar matches the exact authored ASCII occurrence, without byte rewriting.
    for row in new
        .prepare("SELECT record_key,source_ordinal,text FROM record_text")
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, usize>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .unwrap()
    {
        let (key, ordinal, text) = row.unwrap();
        let source = &authored[&old_key(identity, plugins, key)];
        assert_eq!(source.subrecords[ordinal].0.as_slice(), b"EDID");
        assert_eq!(
            text,
            std::str::from_utf8(&source.subrecords[ordinal].1)
                .unwrap()
                .trim_end_matches('\0')
        );
    }
    let normalize_value = |value: Value| match value {
        Value::Null => Value::Null,
        Value::Integer(0) => Value::Integer(0),
        Value::Integer(value) => Value::Integer(i64::from(old_key(identity, plugins, value))),
        other => panic!("invalid identity storage {other:?}"),
    };
    let mut properties = rows(new, "script_properties");
    for row in &mut properties {
        row[0] = normalize_value(row[0].clone());
    }
    assert_eq!(
        properties,
        vec![vec![
            Value::Integer(0x811),
            Value::Text("Independent".into()),
            Value::Integer(0),
            Value::Text("Object".into()),
            Value::Integer(1),
            Value::Integer(1),
            Value::Integer(1)
        ]]
    );
    let mut values = rows(new, "script_property_values");
    for row in &mut values {
        row[0] = normalize_value(row[0].clone());
        row[4] = normalize_value(row[4].clone());
    }
    assert_eq!(
        values,
        vec![vec![
            Value::Integer(0x811),
            Value::Text("Independent".into()),
            Value::Integer(0),
            Value::Integer(0),
            Value::Integer(0xfe00_0812),
            Value::Integer(0x3456),
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null
        ]]
    );
    let properties_json = format!(
        r#"[{{"name":"Object","property_type":1,"status":1,"value":{{"Object":{{"form_id":{},"alias_id":13398}}}}}}]"#,
        0xfe00_0812u32
    );
    assert_eq!(
        old.query_row("SELECT properties_json FROM scripts", [], |row| row
            .get::<_, String>(0))
            .unwrap(),
        properties_json
    );
    let mut scripts = rows(new, "scripts");
    for row in &mut scripts {
        row[0] = normalize_value(row[0].clone());
    }
    assert_eq!(
        scripts,
        vec![vec![
            Value::Integer(0x811),
            Value::Text("Independent".into()),
            Value::Blob(script_object(0x812))
        ]]
    );
    let schema = |conn: &rusqlite::Connection| {
        conn.prepare("SELECT type,name,tbl_name,sql FROM sqlite_master ORDER BY type,name,sql")
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    let old_schema = schema(old);
    let new_schema = schema(new);
    for (kind, name, table, _) in &new_schema {
        if !old_schema
            .iter()
            .any(|(_, old_name, _, _)| old_name == name)
        {
            assert!(
                additions.iter().any(|(extra, _)| *extra == table.as_str()),
                "unreviewed new schema object {kind} {name} on {table}"
            );
        }
    }
    // Independently establish the rows before the complete old/new schema-and-row comparison.
    // In particular, no exterior node BLOB may hide an unnormalized reference FormKey.
    assert_empty_spatial_contract(old);
    assert_empty_spatial_contract(new);
    let mut dump = Vec::new();
    for (kind, name, _, sql) in old_schema {
        let actual = new_schema
            .iter()
            .find(|(_, new_name, _, _)| new_name == &name)
            .unwrap();
        if name != "scripts" {
            assert_eq!((&kind, &sql), (&actual.0, &actual.3), "schema {name}");
        }
        dump.push(format!("schema {kind} {name} {sql:?}"));
        if kind != "table" {
            continue;
        }
        let identity_fields = identity_columns(&name);
        let columns = old
            .prepare(&format!("SELECT * FROM \"{name}\""))
            .unwrap()
            .column_names()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let mut data = rows(new, &name);
        if name == "scripts" {
            data = vec![vec![
                Value::Integer(0x811),
                Value::Text("Independent".into()),
                Value::Blob(script_object(0xfe00_0812)),
                Value::Text(properties_json.clone()),
            ]];
        } else {
            let native_columns = new
                .prepare(&format!("SELECT * FROM \"{name}\""))
                .unwrap()
                .column_names()
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            assert_eq!(columns, native_columns, "all columns {name}");
            for row in &mut data {
                let owner = columns
                    .iter()
                    .position(|column| matches!(column.as_str(), "form_id" | "id" | "cell_id"))
                    .and_then(|index| match row[index] {
                        Value::Integer(value) => Some(value),
                        _ => None,
                    });
                for (index, column) in columns.iter().enumerate() {
                    if identity_fields
                        .split_ascii_whitespace()
                        .any(|key| key == column)
                    {
                        row[index] = normalize_value(row[index].clone());
                    }
                    if matches!(
                        (name.as_str(), column.as_str()),
                        ("records", "data")
                            | ("cells", "data")
                            | ("references", "data")
                            | ("waters", "data")
                    ) {
                        let record = &normalized[&owner.unwrap()];
                        row[index] = Value::Blob(if name == "records" {
                            rkyv::to_bytes::<rkyv::rancor::Error>(record)
                                .unwrap()
                                .to_vec()
                        } else {
                            converter::esm::extractors::serialize_subrecords(
                                &record
                                    .subrecords
                                    .iter()
                                    .map(|field| (field.tag.to_vec(), field.data.clone()))
                                    .collect::<Vec<_>>(),
                            )
                        });
                    }
                }
            }
        }
        let mut data = data
            .into_iter()
            .map(|row| format!("{row:?}"))
            .collect::<Vec<_>>();
        data.sort();
        dump.extend(data.into_iter().map(|row| format!("{name} {row}")));
    }
    dump
}

/// Compare actual conversion against frozen old packing, all-table SQL content and cache bytes.
#[test]
fn synthetic_publication_matches_independent_old_packer_and_cell_cache() {
    use converter::esm::{EsmParser, exporter, load_order::LoadOrder};
    use rusqlite::{Connection, params};
    let root = tempfile::tempdir().unwrap();
    let plugins = [
        Plugin {
            name: "Z.esm".into(),
            light: false,
            masters: vec![],
        },
        Plugin {
            name: "L.esl".into(),
            light: true,
            masters: vec!["Z.esm".into()],
        },
        Plugin {
            name: "A.esp".into(),
            light: false,
            masters: vec!["L.esl".into(), "Z.esm".into()],
        },
    ];
    let bodies = [
        native_record(b"STAT", 0x811, 0, &native_field(b"EDID", b"earlier\0")),
        native_record(b"STAT", 0x0100_0812, 0, &native_field(b"EDID", b"light\0")),
        [
            native_record(
                b"STAT",
                0x0100_0811,
                0,
                &[
                    native_field(b"EDID", b"winner\0"),
                    native_field(b"VMAD", &script_object(0x0000_0812)),
                ]
                .concat(),
            ),
            native_record(
                b"CELL",
                0x0200_0913,
                0,
                &[
                    native_field(b"EDID", b"InteriorFixture\0"),
                    native_field(b"DATA", &[1, 0]),
                ]
                .concat(),
            ),
            cell_group(
                0x0200_0913,
                &native_record(
                    b"REFR",
                    0x0200_0914,
                    0,
                    &[
                        native_field(b"NAME", &0x0100_0811u32.to_le_bytes()),
                        native_field(
                            b"XESP",
                            &[0x0000_0812u32.to_le_bytes(), [1, 0xa7, 0xb8, 0xc9]].concat(),
                        ),
                        native_field(b"DATA", &[0; 24]),
                    ]
                    .concat(),
                ),
            ),
        ]
        .concat(),
    ];
    let mut paths = Vec::new();
    for (p, body) in plugins.iter().zip(&bodies) {
        let mut header = native_field(
            b"HEDR",
            &[1.71f32.to_le_bytes(), [0; 4], 0x800u32.to_le_bytes()].concat(),
        );
        for master in &p.masters {
            header.extend(native_field(b"MAST", format!("{master}\0").as_bytes()));
            header.extend(native_field(b"DATA", &[0; 8]));
        }
        let path = root.path().join(&p.name);
        std::fs::write(
            &path,
            [
                native_record(b"TES4", 0, if p.light { 0x200 } else { 0 }, &header),
                body.clone(),
            ]
            .concat(),
        )
        .unwrap();
        paths.push(path);
    }
    let mut expected = std::collections::HashMap::new();
    let mut authored = std::collections::HashMap::new();
    for (supplier, path) in paths.iter().enumerate() {
        for mut record in parse_plugin_file(path).unwrap() {
            let mut original = record.clone();
            original.load_order = supplier as u32;
            authored.insert(
                old_pack(&plugins, supplier, record.form_id).unwrap(),
                original,
            );
            record.form_id = old_pack(&plugins, supplier, record.form_id).unwrap();
            if record.cell_form_id.is_some() {
                assert_eq!(record.cell_form_id, Some(0x0200_0913));
                record.cell_form_id = Some(0x0100_0913);
            }
            for (tag, data) in &mut record.subrecords {
                match tag.as_slice() {
                    b"VMAD" => *data = script_object(0xfe00_0812),
                    b"NAME" => *data = 0x811u32.to_le_bytes().to_vec(),
                    b"XESP" => data[..4].copy_from_slice(&0xfe00_0812u32.to_le_bytes()),
                    _ => {}
                }
            }
            record.load_order = supplier as u32;
            expected.insert(record.form_id, record);
        }
    }
    assert_eq!(expected[&0x811].load_order, 2);
    assert!(expected.contains_key(&0xfe00_0812));
    // The retained scalar oracle is explicitly unbound and MEMORY ONLY. It never
    // acquires a native world stamp or a disk-format compatibility adapter.
    let conn = Connection::open_in_memory().unwrap();
    exporter::create_tables(&conn).unwrap();
    for (priority, path) in paths.iter().enumerate() {
        conn.execute(
            "INSERT INTO plugins(id,name,priority,checksum) VALUES(?1,?2,?1,?3)",
            params![
                priority as u32,
                path.file_name().unwrap().to_string_lossy(),
                Sha256::digest(std::fs::read(path).unwrap()).as_slice()
            ],
        )
        .unwrap();
    }
    exporter::export_to_db_with_load_order(&conn, &expected, &LoadOrder::read(&paths).unwrap())
        .unwrap();
    let new_db = root.path().join("new.db");
    EsmParser::convert_plugins(&paths, &new_db).unwrap();
    let new = Connection::open(&new_db).unwrap();
    let before = Sha256::digest(std::fs::read(&new_db).unwrap());
    assert_eq!(
        logical_dump(&conn),
        native_logical_dump(&conn, &new, &plugins, &authored)
    );
    assert_eq!(
        Sha256::digest(std::fs::read(&new_db).unwrap()),
        before,
        "differential interpretation never changes native SQL"
    );
    let actual = EsmParser::merge_plugins(&paths).unwrap();
    let old_cache = root.path().join("old.rkyv");
    let new_cache = root.path().join("new.rkyv");
    // This interior-only fixture never had LAND, geometry, layers or water in its
    // cache. Establish that precondition independently instead of publishing an
    // unauthenticated scalar map through the current native cache API.
    for (records, path) in [(&expected, &old_cache), (&actual, &new_cache)] {
        assert!(
            records
                .values()
                .all(|record| record.record_type != *b"LAND")
        );
        let error = converter::esm::cell_cache::write_cell_cache(records, path).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("native FormKey context is required")
        );
        assert!(error.to_string().contains("convert again"));
        assert!(
            !path.exists(),
            "refused unbound publication must not create a cache"
        );
    }
    let expected_cache = shared::CellCache {
        version: 4,
        cells: Vec::new(),
    };
    let expected_payload = rkyv::to_bytes::<rkyv::rancor::Error>(&expected_cache).unwrap();
    // The genuine native writer adds an authenticated envelope and native identities.
    // After that explicit representation transition, every payload byte must still agree.
    let native = EsmParser::merge_plugins_form_keys(&paths).unwrap();
    assert!(
        native
            .records
            .values()
            .all(|record| record.source.record_type != *b"LAND")
    );
    let native_cache = root.path().join("native.rkyv");
    let (count, warnings) = converter::esm::cell_cache::write_form_key_cell_cache(
        &native,
        &Default::default(),
        &native_cache,
        false,
    )
    .unwrap();
    assert_eq!(count, 0);
    assert!(warnings.is_empty(), "{warnings:?}");
    let identity = shared::pack_identity::PackIdentity::read_sql(&new).unwrap();
    assert_eq!(
        native.order.pack_identity().unwrap().digest(),
        identity.digest()
    );
    let file_bytes = std::fs::read(native_cache).unwrap();
    // Independent envelope oracle: do not use the production encoder to construct
    // its expected magic/kind/format/identity/reserved/length/hash/payload bytes.
    let mut expected_file = b"MCFKPACK".to_vec();
    expected_file.extend_from_slice(&0x4345_4c52u32.to_le_bytes());
    expected_file.extend_from_slice(&4u32.to_le_bytes());
    expected_file.extend_from_slice(&1u32.to_le_bytes());
    expected_file.extend_from_slice(&[0; 4]);
    expected_file.extend_from_slice(&identity.digest());
    expected_file.extend_from_slice(&(expected_payload.len() as u64).to_le_bytes());
    expected_file.extend_from_slice(&Sha256::digest(expected_payload.as_slice()));
    expected_file.extend_from_slice(&expected_payload);
    assert_eq!(
        file_bytes, expected_file,
        "entire native cache envelope and payload"
    );
    let mut aligned = rkyv::util::AlignedVec::<16>::new();
    aligned.extend_from_slice(&file_bytes);
    let payload = shared::cache_identity::cell_cache_payload(&identity, &aligned, false).unwrap();
    let mut cache = rkyv::from_bytes::<shared::CellCache, rkyv::rancor::Error>(payload).unwrap();
    assert_eq!(cache, expected_cache);
    for cell in &mut cache.cells {
        cell.cell_id = u64::from(old_key(&identity, &plugins, cell.cell_id as i64));
        cell.water_type_form_id = cell
            .water_type_form_id
            .map(|key| u64::from(old_key(&identity, &plugins, key as i64)));
        for layer in &mut cell.layers {
            if layer.texture_form_id != 0 {
                layer.texture_form_id =
                    u64::from(old_key(&identity, &plugins, layer.texture_form_id as i64));
            }
        }
    }
    assert_eq!(
        rkyv::to_bytes::<rkyv::rancor::Error>(&cache)
            .unwrap()
            .as_slice(),
        expected_payload.as_slice(),
        "entire normalized native cache payload, no skipped fields or float tolerances"
    );
}

#[test]
#[ignore = "read-only 80-plugin audit; set FORMKEY_REAL80_AUDIT=1 explicitly"]
/// Optionally compare an installed source identity stream with a separately pinned receipt.
fn real80_record_identity_baseline_audit() {
    assert_eq!(std::env::var("FORMKEY_REAL80_AUDIT").as_deref(), Ok("1"));
    let input = std::path::PathBuf::from(
        std::env::var_os("FORMKEY_REAL80_INPUT").expect("read-only Data input"),
    );
    let list =
        std::path::PathBuf::from(std::env::var_os("FORMKEY_REAL80_PLUGINS").expect("plugins.txt"));
    let output = std::path::PathBuf::from(
        std::env::var_os("FORMKEY_REAL80_OUTPUT").expect("retained audit output"),
    );
    let paths = converter::esm::read_plugins_txt_with_options(
        &list,
        &input,
        Some(&input.join("Skyrim.ccc")),
        None,
    )
    .unwrap();
    assert_eq!(paths.len(), 80, "audit requires the pinned 80-plugin input");
    let plugins: Vec<_> = paths
        .iter()
        .map(|path| {
            let m = parse_plugin_metadata(path).unwrap();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            Plugin {
                light: name.to_ascii_lowercase().ends_with(".esl") || m.flags & 0x200 != 0,
                name,
                masters: m.masters,
            }
        })
        .collect();
    let mut records = Vec::new();
    let mut source_hashes = String::new();
    for (supplier, path) in paths.iter().enumerate() {
        let bytes = std::fs::read(path).unwrap();
        source_hashes.push_str(&format!(
            "{} {:x}\n",
            plugins[supplier].name,
            Sha256::digest(&bytes)
        ));
        for record in parse_plugin_file(path).unwrap() {
            if (record.form_id >> 24) as usize > plugins[supplier].masters.len() {
                assert_eq!(
                    record.record_type, *b"GMST",
                    "unexpected shipped invalid header"
                );
            }
            records.push((supplier, record.form_id, supplier as u32));
        }
    }
    let bytes = compare(&plugins, &records);
    let receipt = format!(
        "identity-winner-stream-sha256 {:x}\nsource-records {}\n{}",
        Sha256::digest(&bytes),
        records.len(),
        source_hashes
    );
    std::fs::create_dir_all(&output).unwrap();
    // Baselines are never silently overwritten. To pin a baseline, independently
    // review the candidate and copy it to real80-identity-baseline.txt in this folder.
    let baseline = output.join("real80-identity-baseline.txt");
    std::fs::write(output.join("real80-identity-candidate.txt"), &receipt).unwrap();
    assert_eq!(
        std::fs::read_to_string(baseline).expect("review and pin baseline before acceptance"),
        receipt
    );
}
