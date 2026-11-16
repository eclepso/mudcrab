//! Native record admission and winner identity before the compatibility writer packs IDs.
//! FormKey follows Mutagen's owner/local identity; source payload words remain source-local.
use super::{
    EsmParser, GameSettingIdentities, RemapWarnings, binary::parse_plugin_file_with_warnings,
    form_keys::FormKeyOrder, records::RawRecord, remap_record_at_sites_with_origins,
};
use color_eyre::Result;
use shared::{form_key::FormKey, source_form_id::ZeroPolicy};
use std::{cell::RefCell, collections::HashMap, path::PathBuf};

/// Stored semantic outcome, independent of original four-byte source evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkOutcome {
    Resolved,
    Null,
    Cleared,
}

impl LinkOutcome {
    /// Stable SQL spelling for the versioned resolved-link contract.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Resolved => "resolved",
            Self::Null => "null",
            Self::Cleared => "cleared",
        }
    }
}

/// A recognized native link resolved without any runtime slot allocation.
#[derive(Debug, Clone)]
pub struct ResolvedLink {
    /// Original physical subrecord ordinal, including opaque/unrecognized occurrences.
    pub subrecord: usize,
    pub signature: [u8; 4],
    /// Byte offset within that subrecord's payload; never an offset in a rewritten BLOB.
    pub byte_offset: usize,
    pub source_word: u32,
    pub key: Option<FormKey>,
    pub outcome: LinkOutcome,
}

/// Native supplier bytes and resolved identities are separate from legacy writer projections.
#[derive(Debug)]
pub struct FormKeyRecord {
    pub source: RawRecord,
    /// Sanitized runtime fields retain native words; original source is diagnostic evidence.
    pub canonical: Option<RawRecord>,
    /// Original source ordinal for each sanitized canonical occurrence.
    pub canonical_ordinals: Vec<usize>,
    /// Resolved localized scalar text by original occurrence; None means omit the runtime value.
    pub localized_text: std::collections::BTreeMap<usize, Option<String>>,
    /// Original occurrences retained as evidence but not certified for scalar runtime projection.
    pub runtime_omitted: std::collections::BTreeSet<usize>,
    pub cell: Option<FormKey>,
    pub worldspace: Option<FormKey>,
    pub links: Vec<ResolvedLink>,
}

impl FormKeyRecord {
    /// Bounded canonical fields for projection without rewriting native links.
    pub fn canonical_record(&self) -> &RawRecord {
        self.canonical.as_ref().unwrap_or(&self.source)
    }

    /// Keep omitted/repeated/opaque occurrences distinct across canonical serialization.
    pub fn source_ordinal(&self, canonical_ordinal: usize) -> Result<usize> {
        if self.canonical.is_none() {
            return Ok(canonical_ordinal);
        }
        self.canonical_ordinals
            .get(canonical_ordinal)
            .copied()
            .ok_or_else(|| color_eyre::eyre::eyre!("canonical occurrence has no source origin"))
    }

    /// Resolve only a recognized physical site; missing metadata is never guessed from bytes.
    pub fn resolved_at(
        &self,
        source_ordinal: usize,
        byte_offset: usize,
    ) -> Result<Option<FormKey>> {
        self.links
            .iter()
            .find(|site| site.subrecord == source_ordinal && site.byte_offset == byte_offset)
            .map(|site| site.key)
            .ok_or_else(|| {
                color_eyre::eyre::eyre!("missing resolved link site {source_ordinal}:{byte_offset}")
            })
    }
}

/// A cap-free native winner set. Numeric keys have meaning only with this run's plugin table.
#[derive(Debug)]
pub struct FormKeyConversion {
    pub order: FormKeyOrder,
    pub records: HashMap<FormKey, FormKeyRecord>,
    pub warnings: Vec<String>,
    pub notices: Vec<String>,
}

/// Historical API result name. Publication now always uses the contextual native writer.
#[derive(Debug, PartialEq, Eq)]
pub enum LegacyWriterOutcome {
    Published,
    Unavailable { notice: String },
}

impl EsmParser {
    /// Select and convert native FormKeys with Skyrim's priority rules and no slot exclusions.
    pub fn convert_selected_form_key_plugins(
        data: &std::path::Path,
        plugins: &std::path::Path,
        ccc: Option<&std::path::Path>,
        ini: Option<&std::path::Path>,
        database: &std::path::Path,
    ) -> Result<(FormKeyConversion, LegacyWriterOutcome)> {
        let paths =
            super::plugins_txt::read_form_key_plugins_with_options(plugins, data, ccc, ini)?;
        Self::convert_form_key_plugins(&paths, database)
    }

    /// Publish the complete native contextual generation without any legacy slot-cap gate.
    pub fn convert_form_key_plugins(
        paths: &[PathBuf],
        database: &std::path::Path,
    ) -> Result<(FormKeyConversion, LegacyWriterOutcome)> {
        let root = database
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        let mut converted = super::form_key_world::convert(
            paths,
            database,
            crate::config::RecordReader::Legacy,
            super::inhouse::StringSources::default_language(
                super::inhouse::StringRoots::PluginLooseFirst(root),
            ),
            None,
        )?;
        let conversion = converted
            .native
            .take()
            .ok_or_else(|| color_eyre::eyre::eyre!("native writer lost its FormKey context"))?;
        Ok((conversion, LegacyWriterOutcome::Published))
    }

    /// Resolve native records before packing, retaining every accepted plugin beyond legacy limits.
    /// Inputs must be in their recovered load order; this API neither selects nor reorders plugins.
    /// Source words stay native; no runtime or stand-in numbers are allocated.
    pub fn merge_plugins_form_keys(paths: &[PathBuf]) -> Result<FormKeyConversion> {
        let (accepted_paths, name_warnings) = super::form_keys::recover_plugin_paths(paths)?;
        let paths = accepted_paths.as_slice();
        let order = FormKeyOrder::read(paths)?;
        let warnings = RemapWarnings::default();
        let mut summaries = name_warnings;
        let mut records = HashMap::new();
        let mut settings = GameSettingIdentities::default();
        for (priority, path) in paths.iter().enumerate() {
            let mut sources = HashMap::<FormKey, (u32, bool)>::new();
            let name = &order
                .table
                .entry(order.order[priority])
                .expect("allocated supplier")
                .display_name;
            let (native, parser_warnings) = parse_plugin_file_with_warnings(path)?;
            let source_keys = native
                .iter()
                .filter_map(|record| {
                    order
                        .resolve(priority, record.form_id, ZeroPolicy::Null)
                        .ok()
                        .flatten()
                })
                .collect::<std::collections::HashSet<_>>();
            summaries.extend(parser_warnings);
            for mut source in native {
                source.load_order = priority as u32;
                let raw = source.form_id;
                let result = if source.record_type == *b"GMST" {
                    order.gmst_header_key(priority, raw)
                } else {
                    order.resolve(priority, raw, ZeroPolicy::Null)
                };
                let mut key = match result {
                    Ok(key) => key,
                    Err(error) => {
                        warnings.skipped_record(name, &source, raw, error.to_string());
                        continue;
                    }
                };
                let links = RefCell::new(Vec::new());
                let mut checked = source.clone();
                let admission = remap_record_at_sites_with_origins(
                    &mut checked,
                    name,
                    &warnings,
                    |word, strict| {
                        let resolved = if !strict && source.record_type == *b"GMST" {
                            order.gmst_header_key(priority, word)?
                        } else {
                            order.resolve(priority, word, ZeroPolicy::Null)?
                        };
                        if resolved.is_some() {
                            order.note_projection(priority, word);
                        }
                        // Validate framing and link ownership without packing or changing native words.
                        Ok(word)
                    },
                    |subrecord, signature, byte_offset, word, _| {
                        let mut resolved = order.resolve(priority, word, ZeroPolicy::Null)?;
                        let past_master = (word >> 24) as usize
                            > order
                                .table
                                .entry(order.order[priority])
                                .expect("supplier")
                                .facts
                                .masters
                                .len();
                        let mut outcome = if resolved.is_some() {
                            LinkOutcome::Resolved
                        } else {
                            LinkOutcome::Null
                        };
                        if past_master
                            && resolved.is_some_and(|key| {
                                key.local() >= 0x800 && !source_keys.contains(&key)
                            })
                        {
                            warnings.skipped_optional(name, format!("{} record source {raw:08X}: optional FormID {word:08X} exceeds master list", String::from_utf8_lossy(&source.record_type)));
                            resolved = None;
                            outcome = LinkOutcome::Cleared;
                        }
                        if resolved.is_some() {
                            order.note_projection(priority, word);
                        }
                        links.borrow_mut().push(ResolvedLink {
                            subrecord,
                            signature: signature.try_into().expect("native four-byte signature"),
                            byte_offset,
                            source_word: word,
                            key: resolved,
                            outcome,
                        });
                        Ok(word)
                    },
                    false,
                );
                let canonical_ordinals = match admission {
                    Ok(ordinals) => ordinals,
                    Err(error) => {
                        warnings.skipped_record(name, &source, raw, error.to_string());
                        continue;
                    }
                };
                links
                    .borrow_mut()
                    .retain(|link| canonical_ordinals.contains(&link.subrecord));
                if !source.is_deleted() {
                    let validation = if matches!(&source.record_type, b"GMST" | b"RACE") {
                        super::exporter::validate_inhouse_record(&checked)
                            .map_err(color_eyre::eyre::Error::from)
                    } else if source.record_type == *b"MOVT"
                        && key.is_some_and(|key| {
                            key.local() == 0x3580d
                                && order
                                    .table()
                                    .entry(key.plugin())
                                    .is_some_and(|entry| entry.normalized_name == "skyrim.esm")
                        })
                    {
                        let mut selected = checked.clone();
                        selected.form_id = 0x3580d;
                        super::exporter::validate_inhouse_record(&selected)
                            .map_err(color_eyre::eyre::Error::from)
                    } else if source.record_type == *b"LAND" {
                        super::cell_cache::validate_inhouse_land(&source)
                    } else {
                        Ok(())
                    };
                    if let Err(error) = validation {
                        warnings.skipped_record(name, &source, raw, error.to_string());
                        continue;
                    }
                }
                // from_facts allocates every supplier and requires all original masters to precede it.
                if let Some(key) = key {
                    let projected = (raw >> 24) as usize
                        > order
                            .table
                            .entry(order.order[priority])
                            .expect("supplier")
                            .facts
                            .masters
                            .len();
                    if let Some(&(earlier, earlier_projected)) = sources.get(&key)
                        && earlier != raw
                        && (projected || earlier_projected)
                    {
                        warnings.skipped_record(name, &source, raw,
                            format!("records {earlier:08X} and {raw:08X} resolve to the same key; an out-of-range master index would replace another record"));
                        continue;
                    }
                }
                let source_key = key;
                if source.record_type == *b"GMST" {
                    match settings.resolve(&source, key, name, &records) {
                        Ok(canonical) => key = canonical,
                        Err(error) => {
                            warnings.skipped_record(name, &source, raw, error.to_string());
                            continue;
                        }
                    }
                } else if key.is_some_and(|key| settings.aliases.contains_key(&key)) {
                    warnings.skipped_record(
                        name,
                        &source,
                        raw,
                        "record collides with GMST identity".into(),
                    );
                    continue;
                }
                let Some(key) = key else {
                    warnings.skipped_record(
                        name,
                        &source,
                        raw,
                        "record has no publishable identity".into(),
                    );
                    continue;
                };
                let cell = source
                    .cell_form_id
                    .map(|id| order.resolve(priority, id, ZeroPolicy::Null))
                    .transpose();
                let world = source
                    .worldspace_form_id
                    .map(|id| order.resolve(priority, id, ZeroPolicy::Null))
                    .transpose();
                let (cell, worldspace) = match (cell, world) {
                    (Ok(cell), Ok(world)) => (cell.flatten(), world.flatten()),
                    (cell, world) => {
                        warnings.skipped_record(
                            name,
                            &source,
                            raw,
                            format!(
                                "invalid containing group: {:?} {:?}",
                                cell.err(),
                                world.err()
                            ),
                        );
                        continue;
                    }
                };
                // Admission is complete. Tombstones reserve the pre-canonical source key too.
                if let Some(source_key) = source_key {
                    let projected = (raw >> 24) as usize
                        > order
                            .table
                            .entry(order.order[priority])
                            .expect("allocated supplier")
                            .facts
                            .masters
                            .len();
                    sources.insert(source_key, (raw, projected));
                }
                if source.is_deleted() {
                    records.remove(&key);
                } else {
                    records.insert(
                        key,
                        FormKeyRecord {
                            source,
                            canonical: Some(checked),
                            canonical_ordinals,
                            localized_text: Default::default(),
                            runtime_omitted: Default::default(),
                            cell,
                            worldspace,
                            links: links.into_inner(),
                        },
                    );
                }
            }
        }
        let (remap_warnings, mut notices) = warnings.report();
        summaries.extend(remap_warnings);
        notices.extend(
            order
                .projection_warnings()
                .into_iter()
                .map(|line| line.replacen("warning: ", "notice: ", 1)),
        );
        if let Some(notice) = order.notice() {
            notices.push(notice);
        }
        Ok(FormKeyConversion {
            order,
            records,
            warnings: summaries,
            notices,
        })
    }
}

impl std::borrow::Borrow<RawRecord> for FormKeyRecord {
    /// Allow shared GMST collision validation to inspect the source signature.
    fn borrow(&self) -> &RawRecord {
        &self.source
    }
}
