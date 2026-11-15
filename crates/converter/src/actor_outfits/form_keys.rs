//! Native, cap-free outfit projection from contextual canonical records.
use color_eyre::Result;
use rusqlite::Connection;
use shared::{
    actor_outfits::{ActorOutfitCatalogFile, ActorOutfitFile, OutfitPartFile, VERSION},
    form_key::FormKey,
    records::{ContextualRecord, RecordContextReader},
};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Count local omissions without turning malformed modded authoring into a failed conversion.
fn skip(counts: &mut BTreeMap<String, usize>, reason: &str) {
    *counts.entry(reason.into()).or_default() += 1;
}

/// Read all physical occurrences of a native FormID array through the stored site relation.
fn links(
    record: &ContextualRecord,
    tag: [u8; 4],
    counts: &mut BTreeMap<String, usize>,
) -> Vec<FormKey> {
    let mut result = Vec::new();
    for (ordinal, field) in record
        .subrecords
        .iter()
        .enumerate()
        .filter(|(_, field)| field.tag == tag)
    {
        if field.data.len() % 4 != 0 {
            skip(counts, "malformed_link_array");
            continue;
        }
        for offset in (0..field.data.len()).step_by(4) {
            match record.resolve_at(ordinal, offset) {
                Ok(Some(key)) => result.push(key),
                Ok(None) => skip(counts, "cleared_link"),
                Err(_) => skip(counts, "unusable_link_context"),
            }
        }
    }
    result
}

/// Optional absence is distinct from a present field with an unusable context/site.
fn first_link(record: &ContextualRecord, tag: [u8; 4]) -> Result<Option<FormKey>> {
    if record.bytes(tag).is_none() {
        return Ok(None);
    }
    record.first_link(tag, 0)
}

/// Decode the bounded native text prefix without interpreting any identity bytes as text.
fn text(record: &ContextualRecord, tag: [u8; 4]) -> Option<String> {
    record.text(tag).filter(|text| !text.is_empty())
}

/// Follow only admitted NPC templates, preserving the existing Use Inventory cycle policy.
fn effective_outfit(
    reader: &RecordContextReader<'_>,
    kinds: &HashMap<FormKey, String>,
    npc: FormKey,
    counts: &mut BTreeMap<String, usize>,
) -> Option<FormKey> {
    let mut current = npc;
    let mut visited = HashSet::from([npc]);
    loop {
        if kinds.get(&current).is_none_or(|kind| kind != "NPC_") {
            return None;
        }
        let record = match reader.record(current) {
            Ok(record) => record,
            Err(_) => {
                skip(counts, "npc_context_unavailable");
                return None;
            }
        };
        match first_link(&record, *b"DOFT") {
            Ok(Some(outfit)) if kinds.get(&outfit).is_some_and(|kind| kind == "OTFT") => {
                return Some(outfit);
            }
            Ok(_) => {}
            Err(_) => skip(counts, "unusable_outfit_link"),
        }
        let inventory = record
            .bytes(*b"ACBS")
            .filter(|bytes| bytes.len() >= 20)
            .is_some_and(|bytes| u16::from_le_bytes([bytes[18], bytes[19]]) & 0x0100 != 0);
        match first_link(&record, *b"TPLT") {
            Ok(Some(template)) if inventory && visited.insert(template) => current = template,
            Ok(_) => return None,
            Err(_) => {
                skip(counts, "unusable_template_link");
                return None;
            }
        }
    }
}

/// Derive portable outfits directly from the new SQL generation, including owners past slot caps.
/// Known bad fields/items are counted locally; schema/context failures reject the derived input.
pub fn resolve_form_key_actor_outfits(
    connection: &Connection,
    scene_exists: &dyn Fn(&str) -> bool,
) -> Result<ActorOutfitCatalogFile> {
    let reader = RecordContextReader::new(connection)?;
    let identity = reader.identity();
    let mut catalog = ActorOutfitCatalogFile {
        version: VERSION,
        pack_identity: identity.digest_hex(),
        outfits: Vec::new(),
        skipped: BTreeMap::new(),
    };
    let mut kinds = HashMap::new();
    let mut statement = connection.prepare("SELECT form_id,record_type FROM records WHERE record_type IN ('NPC_','RACE','OTFT','ARMO','ARMA','LVLI')")?;
    for row in statement.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })? {
        match row {
            Ok((key, kind)) => match identity.required_key(key) {
                Ok(key) => {
                    kinds.insert(key, kind);
                }
                Err(_) => skip(&mut catalog.skipped, "invalid_record_identity"),
            },
            Err(_) => skip(&mut catalog.skipped, "unusable_record_metadata"),
        }
    }
    let default_race = identity
        .table()
        .id("Skyrim.esm")
        .map(|owner| FormKey::new(owner, 25))
        .transpose()?;
    let mut models =
        connection.prepare("SELECT id,race_id,sex,resolution FROM actor_models ORDER BY id")?;
    for row in models.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Option<i64>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })? {
        let (npc, race, sex, resolution) = match row {
            Ok(row) => row,
            Err(_) => {
                skip(&mut catalog.skipped, "unusable_actor_model");
                continue;
            }
        };
        if resolution == "unresolved" {
            continue;
        }
        if !matches!(sex.as_str(), "male" | "female") {
            skip(&mut catalog.skipped, "invalid_actor_sex");
            continue;
        }
        let (npc, race) = match (identity.required_key(npc), identity.optional_key(race)) {
            (Ok(npc), Ok(Some(race))) => (npc, race),
            _ => {
                skip(&mut catalog.skipped, "invalid_actor_identity");
                continue;
            }
        };
        let Some(outfit) = effective_outfit(&reader, &kinds, npc, &mut catalog.skipped) else {
            continue;
        };
        let outfit_record = match reader.record(outfit) {
            Ok(record) => record,
            Err(_) => {
                skip(&mut catalog.skipped, "outfit_context_unavailable");
                continue;
            }
        };
        let mut parts = Vec::new();
        for item in links(&outfit_record, *b"INAM", &mut catalog.skipped) {
            match kinds.get(&item).map(String::as_str) {
                Some("ARMO") => {}
                Some("LVLI") => {
                    skip(&mut catalog.skipped, "leveled_list");
                    continue;
                }
                Some(_) => {
                    skip(&mut catalog.skipped, "not_armor");
                    continue;
                }
                None => {
                    skip(&mut catalog.skipped, "item_missing");
                    continue;
                }
            }
            let armor = match reader.record(item) {
                Ok(record) => record,
                Err(_) => {
                    skip(&mut catalog.skipped, "armor_context_unavailable");
                    continue;
                }
            };
            let mut compatible = Vec::new();
            let mut defaults = Vec::new();
            for armature in links(&armor, *b"MODL", &mut catalog.skipped) {
                if kinds.get(&armature).is_none_or(|kind| kind != "ARMA") {
                    continue;
                }
                let record = match reader.record(armature) {
                    Ok(record) => record,
                    Err(_) => {
                        skip(&mut catalog.skipped, "armature_context_unavailable");
                        continue;
                    }
                };
                let own_race = match first_link(&record, *b"RNAM") {
                    Ok(Some(race)) => race,
                    _ => {
                        skip(&mut catalog.skipped, "armature_without_race");
                        continue;
                    }
                };
                let additional = links(&record, *b"MODL", &mut catalog.skipped)
                    .into_iter()
                    .any(|key| key == race && kinds.get(&key).is_some_and(|kind| kind == "RACE"));
                if own_race == race || additional {
                    compatible.push((armature, own_race, record));
                } else if Some(own_race) == default_race {
                    defaults.push((armature, own_race, record));
                }
            }
            let found = !compatible.is_empty() || !defaults.is_empty();
            let mut covered = 0u32;
            for (armature, own_race, record) in compatible.into_iter().chain(defaults) {
                let mask = record
                    .bytes(*b"BOD2")
                    .or_else(|| record.bytes(*b"BODT"))
                    .filter(|bytes| bytes.len() >= 4)
                    .map(|bytes| u32::from_le_bytes(bytes[..4].try_into().expect("checked mask")))
                    .unwrap_or(0);
                if mask == 0 {
                    skip(&mut catalog.skipped, "armature_without_slots");
                    continue;
                }
                if covered & mask != 0 {
                    continue;
                }
                let path = if sex == "female" {
                    text(&record, *b"MOD3").or_else(|| text(&record, *b"MOD2"))
                } else {
                    text(&record, *b"MOD2")
                };
                let Some(path) = path else {
                    skip(&mut catalog.skipped, "armature_without_model");
                    continue;
                };
                let scene = match shared::actor_idle_catalog::canonical_body_path(&path, "glb") {
                    Ok(scene) => scene,
                    Err(_) => {
                        skip(&mut catalog.skipped, "invalid_model_path");
                        continue;
                    }
                };
                if !scene_exists(&scene) {
                    skip(&mut catalog.skipped, "converted_model_missing");
                    continue;
                }
                covered |= mask;
                parts.push(OutfitPartFile {
                    armo_id: identity.table().named(item)?,
                    armature_id: identity.table().named(armature)?,
                    armature_race_id: identity.table().named(own_race)?,
                    biped_mask: mask,
                    scene,
                });
            }
            if !found {
                skip(&mut catalog.skipped, "no_compatible_armature");
            }
        }
        if !parts.is_empty() {
            catalog.outfits.push(ActorOutfitFile {
                npc_id: identity.table().named(npc)?,
                outfit_id: identity.table().named(outfit)?,
                parts,
            });
        }
    }
    // Preserve old full-then-light owner order independently of numeric allocation and supplier.
    let ranks: HashMap<_, _> = identity
        .order()
        .iter()
        .enumerate()
        .map(|(priority, &owner)| {
            (
                owner,
                (u8::from(identity.table().is_light(owner)), priority),
            )
        })
        .collect();
    catalog.outfits.sort_by_cached_key(|row| {
        let key = identity
            .table()
            .intern(&row.npc_id)
            .expect("writer-created owner")
            .expect("required NPC");
        (ranks[&key.plugin()], key.local())
    });
    catalog.validate_for(identity)?;
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use shared::{
        form_key::PluginId,
        pack_identity::PackIdentity,
        plugin_table::{PluginFacts, PluginTable},
        records::{ArchivedRecordData, ArchivedSubRecord},
    };

    fn fixture() -> (Connection, PackIdentity) {
        let names: Vec<_> = (0..256)
            .map(|index| format!("Owner{:03}.esm", 255 - index))
            .collect();
        let table = PluginTable::fresh(
            names
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    (
                        name.clone(),
                        PluginFacts {
                            masters: if index == 0 {
                                vec![]
                            } else {
                                vec![names[0].clone()]
                            },
                            content_hash: Some([0x2A; 32]),
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
        connection.execute_batch("CREATE TABLE records(form_id INTEGER PRIMARY KEY,record_type TEXT,load_order INTEGER,data BLOB); CREATE TABLE actor_models(id INTEGER PRIMARY KEY,race_id INTEGER,sex TEXT,resolution TEXT); CREATE TABLE inhouse_localized_fields(form_id INTEGER,field_index INTEGER,resolved_text TEXT)").unwrap();
        let transaction = connection.transaction().unwrap();
        shared::records::create_record_context(&transaction, &identity).unwrap();
        transaction.commit().unwrap();
        (connection, identity)
    }
    fn key(owner: PluginId, local: u32) -> FormKey {
        FormKey::new(owner, local).unwrap()
    }
    fn field(tag: [u8; 4], bytes: Vec<u8>) -> ArchivedSubRecord {
        ArchivedSubRecord { tag, data: bytes }
    }
    fn write(
        connection: &Connection,
        identity: &PackIdentity,
        owner: PluginId,
        local: u32,
        kind: &str,
        fields: Vec<ArchivedSubRecord>,
        links: &[(usize, usize, bool)],
    ) {
        let record = key(owner, local);
        let data = ArchivedRecordData { subrecords: fields };
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&data).unwrap();
        let priority = identity
            .order()
            .iter()
            .position(|candidate| *candidate == owner)
            .unwrap();
        let selector = identity.table().entry(owner).unwrap().facts.masters.len() as u32;
        connection
            .execute(
                "INSERT INTO records VALUES(?1,?2,?3,?4)",
                params![record.to_sql_i64(), kind, priority, bytes.as_slice()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO record_context VALUES(?1,?2,?3,NULL,NULL,?4)",
                params![
                    record.to_sql_i64(),
                    owner.get(),
                    (selector << 24) | local,
                    data.subrecords.len()
                ],
            )
            .unwrap();
        for (ordinal, field) in data.subrecords.iter().enumerate() {
            connection
                .execute(
                    "INSERT INTO record_field_origins VALUES(?1,?2,?2,?3,1)",
                    params![record.to_sql_i64(), ordinal, field.tag.as_slice()],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO record_source_fields VALUES(?1,?2,?3,?4)",
                    params![
                        record.to_sql_i64(),
                        ordinal,
                        field.tag.as_slice(),
                        &field.data
                    ],
                )
                .unwrap();
        }
        for &(ordinal, offset, cleared) in links {
            let raw = u32::from_le_bytes(
                data.subrecords[ordinal].data[offset..offset + 4]
                    .try_into()
                    .unwrap(),
            );
            let target = if cleared {
                None
            } else {
                shared::records::source_key(identity, owner, raw).unwrap()
            };
            connection
                .execute(
                    "INSERT INTO resolved_links VALUES(?1,?2,?3,?4,?5,?6)",
                    params![
                        record.to_sql_i64(),
                        ordinal,
                        offset,
                        raw,
                        target.map(FormKey::to_sql_i64),
                        if cleared {
                            "cleared"
                        } else if raw == 0 {
                            "null"
                        } else {
                            "resolved"
                        }
                    ],
                )
                .unwrap();
        }
    }
    fn actor(connection: &Connection, identity: &PackIdentity, index: usize) {
        let owner = identity.order()[index];
        let source = |local: u32| {
            ((u32::from(index != 0) << 24) | local)
                .to_le_bytes()
                .to_vec()
        };
        write(connection, identity, owner, 10, "RACE", vec![], &[]);
        write(
            connection,
            identity,
            owner,
            20,
            "NPC_",
            vec![field(*b"ACBS", vec![0; 24]), field(*b"DOFT", source(30))],
            &[(1, 0, false)],
        );
        write(
            connection,
            identity,
            owner,
            30,
            "OTFT",
            vec![field(*b"INAM", [source(40), source(41)].concat())],
            &[(0, 0, false), (0, 4, true)],
        );
        write(
            connection,
            identity,
            owner,
            40,
            "ARMO",
            vec![field(*b"MODL", source(50))],
            &[(0, 0, false)],
        );
        write(
            connection,
            identity,
            owner,
            50,
            "ARMA",
            vec![
                field(*b"RNAM", source(10)),
                field(*b"BOD2", [4u32.to_le_bytes(), 2u32.to_le_bytes()].concat()),
                field(*b"MOD2", b"clothes/body.nif\0".to_vec()),
            ],
            &[(0, 0, false)],
        );
        connection
            .execute(
                "INSERT INTO actor_models VALUES(?1,?2,'male','body_head')",
                params![key(owner, 20).to_sql_i64(), key(owner, 10).to_sql_i64()],
            )
            .unwrap();
    }

    #[test]
    fn contextual_outfit_writer_is_cap_free_and_retains_neighbors_of_cleared_items() {
        let (connection, identity) = fixture();
        // Allocation integer order is not the native full/light supplying order.
        actor(&connection, &identity, 254);
        actor(&connection, &identity, 0);
        let file =
            resolve_form_key_actor_outfits(&connection, &|path| path == "meshes/clothes/body.glb")
                .unwrap();
        assert_eq!(file.outfits.len(), 2);
        assert_eq!(file.outfits[0].npc_id.to_string(), "000014:Owner255.esm");
        assert_eq!(file.outfits[1].npc_id.to_string(), "000014:Owner001.esm");
        assert_eq!(file.outfits[1].parts.len(), 1);
        assert_eq!(file.skipped["cleared_link"], 2);
        let json = serde_json::to_value(&file).unwrap();
        assert_eq!(
            json["outfits"][1]["parts"][0]["armature_id"],
            "000032:Owner001.esm"
        );
        let runtime = file.to_runtime(&identity).unwrap();
        assert_eq!(runtime.outfits.len(), 1);
        assert_eq!(runtime.outfits[0].npc_id, 20);
        let reader = RecordContextReader::new(&connection).unwrap();
        let native = reader.record(key(identity.order()[254], 30)).unwrap();
        assert_eq!(native.bytes(*b"INAM").unwrap().len(), 8);
        assert_eq!(native.resolve_at(0, 4).unwrap(), None);
    }
}
