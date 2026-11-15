//! Private CPU scalar-projection indices; these never become runtime IDs or stored identities.
//!
//! A clone of sanitized canonical fields is adapted for existing pure scalar extractors. Native
//! source/canonical evidence and final database BLOBs remain untouched. The final writer translates
//! every identity column directly through `keys`, before any output database is published.
use super::{
    form_key_merge::{FormKeyConversion, FormKeyRecord, LinkOutcome},
    load_order::StableId,
    records::RawRecord,
};
use color_eyre::{
    Result,
    eyre::{ensure, eyre},
};
use rusqlite::Connection;
use shared::{form_key::FormKey, source_form_id::ZeroPolicy};
use std::collections::{BTreeSet, HashMap};

/// Compact CPU indices adapt scalar extractors only, with no plugin-slot allocation.
pub struct ProjectionStage<'a> {
    pub master: HashMap<u32, RawRecord>,
    pub keys: HashMap<u32, FormKey>,
    pub reverse: HashMap<FormKey, u32>,
    pub plugin_ids: HashMap<u32, StableId>,
    originals: HashMap<u32, &'a FormKeyRecord>,
}

impl<'a> ProjectionStage<'a> {
    /// Build one deterministic owner/local index, including referenced owners without rows.
    /// Skyrim's native local indices preserve explicit built-in scalar-extractor predicates.
    pub fn build(conversion: &'a FormKeyConversion) -> Result<Self> {
        let mut all = BTreeSet::new();
        for (&key, record) in &conversion.records {
            all.insert(key);
            all.extend(record.cell);
            all.extend(record.worldspace);
            let supplier = record.source.load_order as usize;
            all.extend(conversion.order.resolve(
                supplier,
                record.source.form_id,
                ZeroPolicy::Null,
            )?);
            for site in &record.links {
                all.extend(site.key);
                // Even a cleared absent target retains its source identity in this private index.
                // The cloned scalar field still receives zero for that site's semantic outcome.
                all.extend(conversion.order.resolve(
                    supplier,
                    site.source_word,
                    ZeroPolicy::Null,
                )?);
            }
        }
        let mut ordered = all
            .into_iter()
            .map(|key| {
                conversion
                    .order
                    .identity_sort_tuple(key)
                    .map(|rank| (rank, key))
            })
            .collect::<Result<Vec<_>>>()?;
        ordered.sort_unstable_by_key(|(rank, _)| *rank);
        let skyrim = conversion.order.table().id("Skyrim.esm");
        let mut keys = HashMap::with_capacity(ordered.len());
        let mut reverse = HashMap::with_capacity(ordered.len());
        let mut plugin_ids = HashMap::with_capacity(ordered.len());
        let mut next = 0x0100_0000u32;
        for (_, key) in ordered {
            let handle = if Some(key.plugin()) == skyrim && key.local() != 0 {
                key.local()
            } else {
                let handle = next;
                next = next
                    .checked_add(1)
                    .ok_or_else(|| eyre!("CPU scalar-projection index capacity exceeded"))?;
                handle
            };
            ensure!(
                keys.insert(handle, key).is_none(),
                "duplicate CPU projection index"
            );
            reverse.insert(key, handle);
            let owner = conversion
                .order
                .table()
                .entry(key.plugin())
                .ok_or_else(|| eyre!("unknown native owner in projection"))?;
            plugin_ids.insert(
                handle,
                StableId {
                    plugin: owner.display_name.clone(),
                    local_id: key.local(),
                },
            );
        }
        let mut stage = Self {
            master: HashMap::with_capacity(conversion.records.len()),
            keys,
            reverse,
            plugin_ids,
            originals: HashMap::with_capacity(conversion.records.len()),
        };
        for (&key, record) in &conversion.records {
            let handle = stage.handle(key)?;
            let mut canonical = record.canonical_record().clone();
            canonical.form_id = handle;
            canonical.cell_form_id = record.cell.map(|key| stage.handle(key)).transpose()?;
            canonical.worldspace_form_id =
                record.worldspace.map(|key| stage.handle(key)).transpose()?;
            let mut fields = Vec::with_capacity(canonical.subrecords.len());
            for (ordinal, (signature, mut bytes)) in canonical.subrecords.into_iter().enumerate() {
                let source = record.source_ordinal(ordinal)?;
                if record.runtime_omitted.contains(&source) {
                    continue;
                }
                for site in record.links.iter().filter(|site| site.subrecord == source) {
                    ensure!(
                        signature.as_slice() == site.signature,
                        "CPU projection site signature mismatch"
                    );
                    let end = site
                        .byte_offset
                        .checked_add(4)
                        .ok_or_else(|| eyre!("CPU projection site offset overflow"))?;
                    let word = bytes
                        .get_mut(site.byte_offset..end)
                        .ok_or_else(|| eyre!("CPU projection site outside canonical occurrence"))?;
                    ensure!(
                        *word == site.source_word.to_le_bytes(),
                        "CPU projection site differs from native evidence"
                    );
                    let projected = match (site.outcome, site.key) {
                        (LinkOutcome::Resolved, Some(key)) => stage.handle(key)?,
                        (LinkOutcome::Null | LinkOutcome::Cleared, None) => 0,
                        _ => return Err(eyre!("native projection site has inconsistent outcome")),
                    };
                    word.copy_from_slice(&projected.to_le_bytes());
                }
                match record.localized_text.get(&source) {
                    Some(None) => continue,
                    Some(Some(text)) => bytes = super::inhouse::runtime_string(text),
                    None => {}
                }
                fields.push((signature, bytes));
            }
            canonical.subrecords = fields;
            stage.master.insert(handle, canonical);
            stage.originals.insert(handle, record);
        }
        Ok(stage)
    }

    /// SQL zero is the scalar intermediate's null sentinel; unknown indices are errors.
    pub fn key(&self, handle: u32) -> Result<Option<FormKey>> {
        if handle == 0 {
            return Ok(None);
        }
        self.keys
            .get(&handle)
            .copied()
            .map(Some)
            .ok_or_else(|| eyre!("unknown CPU scalar-projection index {handle}"))
    }

    /// Resolve an existing intermediate index without allocating runtime/session identifiers.
    pub fn handle(&self, key: FormKey) -> Result<u32> {
        self.reverse
            .get(&key)
            .copied()
            .ok_or_else(|| eyre!("native key is missing from CPU projection index"))
    }

    /// Materialized row BLOBs always come from the original contextual canonical record.
    pub fn original(&self, handle: u32) -> Result<&'a FormKeyRecord> {
        self.originals
            .get(&handle)
            .copied()
            .ok_or_else(|| eyre!("CPU projection index has no winning source record"))
    }

    /// Run pure scalar projection in private memory; the connection is never a published world.
    pub fn project(&self) -> Result<Connection> {
        self.project_with_source_offsets(&HashMap::new())
    }

    /// Preserve physical LAND precedence while adapting only private CPU indices.
    pub fn project_with_source_offsets(
        &self,
        source_offsets: &HashMap<FormKey, u64>,
    ) -> Result<Connection> {
        let connection = Connection::open_in_memory()?;
        let offsets = source_offsets
            .iter()
            .map(|(&key, &offset)| self.handle(key).map(|handle| (handle, offset)))
            .collect::<Result<HashMap<_, _>>>()?;
        super::exporter::project_native_rows_with_offsets(
            &connection,
            &self.master,
            &self.plugin_ids,
            &offsets,
        )?;
        // Pure scalar table setup may create the ordinary schema row. This private generation
        // has no pack identity and is deliberately stamped outside the runtime-accepted range.
        connection.execute("UPDATE schema_info SET version=0", [])?;
        Ok(connection)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::esm::{form_key_merge::ResolvedLink, form_keys::FormKeyOrder};
    use shared::{form_key::PluginId, plugin_table::PluginFacts};

    /// Synthetic source facts keep alphabetical owner allocation separate from supplier priority.
    fn conversion() -> FormKeyConversion {
        let order = FormKeyOrder::from_facts(vec![
            ("Zulu.esm".into(), PluginFacts::default()),
            ("Skyrim.esm".into(), PluginFacts::default()),
            (
                "Alpha.esp".into(),
                PluginFacts {
                    masters: vec!["Zulu.esm".into()],
                    ..Default::default()
                },
            ),
        ])
        .unwrap();
        let key = FormKey::new(PluginId::new(1).unwrap(), 0x811).unwrap();
        let missing = FormKey::new(PluginId::new(1).unwrap(), 0x912).unwrap();
        let source = RawRecord {
            form_id: 0x0100_0811,
            record_type: *b"REFR",
            flags: 0,
            subrecords: vec![
                (b"NAME".to_vec(), 0x0100_0912u32.to_le_bytes().to_vec()),
                (b"ZZZZ".to_vec(), 0x0100_0912u32.to_le_bytes().to_vec()),
                (b"FULL".to_vec(), 17u32.to_le_bytes().to_vec()),
            ],
            cell_form_id: None,
            worldspace_form_id: None,
            load_order: 2,
        };
        let record = FormKeyRecord {
            source: source.clone(),
            canonical: Some(source),
            canonical_ordinals: vec![0, 1, 2],
            runtime_omitted: [1].into(),
            localized_text: [(2, Some("Named room".into()))].into(),
            cell: None,
            worldspace: None,
            links: vec![ResolvedLink {
                subrecord: 0,
                signature: *b"NAME",
                byte_offset: 0,
                source_word: 0x0100_0912,
                key: Some(missing),
                outcome: LinkOutcome::Resolved,
            }],
        };
        FormKeyConversion {
            order,
            records: [(key, record)].into(),
            warnings: vec![],
            notices: vec![],
        }
    }

    /// Only recognized physical words are changed in the private scalar clone.
    #[test]
    fn compact_indices_preserve_source_bytes_missing_keys_and_owner_metadata() {
        let conversion = conversion();
        let stage = ProjectionStage::build(&conversion).unwrap();
        let key = FormKey::new(PluginId::new(1).unwrap(), 0x811).unwrap();
        let handle = stage.handle(key).unwrap();
        let projected = &stage.master[&handle];
        let target = u32::from_le_bytes(projected.subrecords[0].1.as_slice().try_into().unwrap());
        assert_eq!(stage.key(target).unwrap().unwrap().local(), 0x912);
        assert!(target >= 0x0100_0000);
        assert!(
            !projected
                .subrecords
                .iter()
                .any(|(signature, _)| signature == b"ZZZZ")
        );
        assert_eq!(projected.subrecords[1].1, b"Named room\0");
        assert_eq!(
            stage.original(handle).unwrap().source.subrecords[0].1,
            0x0100_0912u32.to_le_bytes()
        );
        assert_eq!(stage.plugin_ids[&handle].plugin, "Alpha.esp");
        assert_eq!(stage.plugin_ids[&handle].local_id, 0x811);
        assert!(stage.original(target).is_err());
        assert_eq!(stage.key(0).unwrap(), None);
    }

    /// Actual Skyrim built-in keys keep scalar predicates without assigning legacy slots.
    #[test]
    fn skyrim_scalar_origins_keep_their_native_local_indices() {
        let mut conversion = conversion();
        let skyrim = conversion.order.table().id("Skyrim.esm").unwrap();
        for local in [0x28, 0x3580D] {
            let key = FormKey::new(skyrim, local).unwrap();
            let source = RawRecord {
                form_id: local,
                record_type: *b"STAT",
                flags: 0,
                subrecords: vec![],
                cell_form_id: None,
                worldspace_form_id: None,
                load_order: 1,
            };
            conversion.records.insert(
                key,
                FormKeyRecord {
                    source,
                    canonical: None,
                    canonical_ordinals: vec![],
                    runtime_omitted: Default::default(),
                    localized_text: Default::default(),
                    cell: None,
                    worldspace: None,
                    links: vec![],
                },
            );
        }
        let stage = ProjectionStage::build(&conversion).unwrap();
        assert_eq!(
            stage.handle(FormKey::new(skyrim, 0x28).unwrap()).unwrap(),
            0x28
        );
        assert_eq!(
            stage
                .handle(FormKey::new(skyrim, 0x3580D).unwrap())
                .unwrap(),
            0x3580D
        );
    }
}
