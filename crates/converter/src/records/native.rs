//! Native typed winners and physical source sites, independent of runtime slot packing.
use super::{DecodedRecord, PluginDiagnostics, Value, addons::RuntimeSchemas, decoder, scanner};
use crate::esm::{
    form_key_merge::{LinkOutcome, ResolvedLink},
    form_keys::FormKeyOrder,
    records::RawRecord,
};
use color_eyre::{Result, eyre::ensure};
use shared::{
    form_key::{FormKey, NamedFormKey},
    source_form_id::ZeroPolicy,
};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
};

/// Original source and sanitized typed canonical fields share explicit physical coordinates.
#[derive(Debug)]
pub struct NativeRecord {
    pub source: RawRecord,
    pub canonical: RawRecord,
    /// Each canonical field's original source ordinal, including skipped/opaque occurrences.
    pub canonical_ordinals: Vec<usize>,
    pub cell: Option<FormKey>,
    pub worldspace: Option<FormKey>,
    pub topic: Option<FormKey>,
    pub links: Vec<ResolvedLink>,
    /// Header metadata, typed named values and original bounded source payload.
    pub decoded: DecodedRecord,
    pub localized: bool,
}

impl NativeRecord {
    /// Emit portable typed evidence: semantic identities are named, native source facts stay raw.
    pub fn write_typed<W: std::io::Write>(
        &self,
        writer: &mut W,
        key: FormKey,
        table: &shared::plugin_table::PluginTable,
        localization: &BTreeMap<usize, serde_json::Value>,
    ) -> serde_json::Result<()> {
        super::typed_export::write_native_record(writer, Some(key), self, table, localization)
    }
    /// TES4 has no semantic record key, while all source and typed metadata remain available.
    pub fn write_typed_header<W: std::io::Write>(
        &self,
        writer: &mut W,
        table: &shared::plugin_table::PluginTable,
        localization: &BTreeMap<usize, serde_json::Value>,
    ) -> serde_json::Result<()> {
        super::typed_export::write_native_record(writer, None, self, table, localization)
    }
}

/// Physical admission evidence does not reinterpret native header words as semantic keys.
#[derive(Debug)]
pub struct NativeHistoryEntry {
    pub supplier: usize,
    pub source_word: u32,
    pub source_offset: u64,
    pub signature: [u8; 4],
    pub flags: u32,
    pub source_record_size: u32,
    pub source_group_end: u64,
    pub frame_complete: bool,
    pub source_cell_form_id: Option<u32>,
    pub source_worldspace_form_id: Option<u32>,
    pub source_topic_form_id: Option<u32>,
    pub topic_group_offset: Option<u64>,
    pub editor_id: Option<String>,
    pub key: Option<FormKey>,
    pub deleted: bool,
    pub accepted: bool,
    pub reason: Option<String>,
}

/// Native owner keys remain valid beyond both legacy slot limits.
#[derive(Debug, Default)]
pub struct NativeReadResult {
    pub records: HashMap<FormKey, NativeRecord>,
    pub headers: Vec<NativeRecord>,
    pub diagnostics: BTreeMap<String, PluginDiagnostics>,
    pub history: Vec<NativeHistoryEntry>,
}

impl NativeReadResult {
    /// Omitted fields and records remain warnings; faithful link clearing keeps its severity.
    pub fn warnings(&self) -> Vec<String> {
        self.diagnostics
            .iter()
            .flat_map(|(name, diagnostics)| diagnostics.warning_summary_lines(name))
            .collect()
    }

    /// Preserve the typed reader's bounded compatibility and faithfully cleared-link notices.
    pub fn notices(&self) -> Vec<String> {
        self.diagnostics
            .iter()
            .flat_map(|(name, diagnostics)| diagnostics.notice_summary_lines(name))
            .collect()
    }
}

/// One field's address span is used only for checked offsets, never dereferenced by address.
#[derive(Clone, Copy, Default)]
struct FieldSpan {
    address: usize,
    length: usize,
    ordinal: usize,
    signature: [u8; 4],
}

/// Per-decode state is explicit and scoped; concurrent readers never share capture state.
pub(crate) struct DecodeContext<'a> {
    pub order: &'a FormKeyOrder,
    pub supplier: usize,
    pub source_ids: &'a HashSet<FormKey>,
    pub kinds: Option<&'a HashMap<FormKey, [u8; 4]>>,
    span: Cell<FieldSpan>,
    failed_root: Cell<Option<usize>>,
    links: RefCell<Vec<ResolvedLink>>,
    origins: RefCell<Vec<usize>>,
}

impl<'a> DecodeContext<'a> {
    /// Begin one native decode without allocating any runtime identifiers.
    pub(crate) fn new(
        order: &'a FormKeyOrder,
        supplier: usize,
        source_ids: &'a HashSet<FormKey>,
        kinds: Option<&'a HashMap<FormKey, [u8; 4]>>,
    ) -> Self {
        Self {
            order,
            supplier,
            source_ids,
            kinds,
            span: Cell::new(FieldSpan::default()),
            failed_root: Cell::new(None),
            links: RefCell::new(Vec::new()),
            origins: RefCell::new(Vec::new()),
        }
    }

    /// Header/group and selector probes do not create physical field occurrences.
    pub(crate) fn resolve_word(&self, word: u32) -> std::result::Result<Option<FormKey>, String> {
        self.order
            .resolve(self.supplier, word, ZeroPolicy::Null)
            .map_err(|error| error.to_string())
    }

    /// Probe accepted catalogue kinds using supplier-relative native words.
    pub(crate) fn link_kind(&self, word: u32) -> Option<[u8; 4]> {
        self.kinds?.get(&self.resolve_word(word).ok()??).copied()
    }

    /// Retain the exact outer source occurrence before interpreting nested values.
    pub(crate) fn start_field(&self, ordinal: usize, signature: [u8; 4], bytes: &[u8]) {
        self.span.set(FieldSpan {
            address: bytes.as_ptr() as usize,
            length: bytes.len(),
            ordinal,
            signature,
        });
    }

    /// A mandatory root failure keeps an exact occurrence, independent of nested diagnostic paths.
    pub(super) fn reject_root(&self, ordinal: usize) {
        self.failed_root.set(Some(ordinal));
    }

    /// Drop captures from rejected occurrences and map surviving canonical fields explicitly.
    pub(crate) fn finish_fields(&self, fields: &[(usize, usize)]) {
        let mut ordered = fields.to_vec();
        ordered.sort_unstable_by_key(|(_, canonical)| *canonical);
        let origins: Vec<_> = ordered.into_iter().map(|(source, _)| source).collect();
        self.links
            .borrow_mut()
            .retain(|link| origins.contains(&link.subrecord));
        *self.origins.borrow_mut() = origins;
    }

    /// Resolve one physically decoded word while preserving its original four-byte payload.
    pub(crate) fn site_at(
        &self,
        containing: &[u8],
        offset: usize,
        word: u32,
        targets: &[String],
        diagnostics: &mut PluginDiagnostics,
    ) -> std::result::Result<Value, String> {
        let span = self.span.get();
        let base = (containing.as_ptr() as usize)
            .checked_sub(span.address)
            .ok_or("native site precedes source field")?;
        let byte_offset = base
            .checked_add(offset)
            .ok_or("native source site overflow")?;
        if byte_offset
            .checked_add(4)
            .is_none_or(|end| end > span.length)
            || containing.get(offset..offset.saturating_add(4))
                != Some(word.to_le_bytes().as_slice())
        {
            return Err("native site is outside its source field".into());
        }
        diagnostics.link(word);
        let mut key = self.resolve_word(word)?;
        let master_count = self
            .order
            .table()
            .entry(self.order.plugin_order()[self.supplier])
            .expect("validated supplier")
            .facts
            .masters
            .len();
        if crate::esm::legacy_projection::LegacyTypedReaderProjection::maps_to_null(
            word,
            master_count,
            true,
            diagnostics.event_context.record_type,
            diagnostics.event_context.field.as_deref(),
        ) {
            diagnostics.note("link", || {
                format!("optional FormID {word:08X} exceeds master list")
            });
            key = None;
        }
        if let Some(target) = key {
            let entry = self
                .order
                .table()
                .entry(self.order.plugin_order()[self.supplier])
                .expect("validated supplier");
            let past_master = (word >> 24) as usize > entry.facts.masters.len();
            let first_owner = self.order.plugin_order().first().copied();
            let built_in = Some(target.plugin()) == first_owner && target.local() < 0x800;
            let absent_source = past_master && !built_in && !self.source_ids.contains(&target);
            let bad_target = self.kinds.is_some_and(|kinds| {
                kinds.get(&target).map_or(!built_in, |kind| {
                    !targets.is_empty()
                        && !targets.iter().any(|allowed| {
                            allowed == "*" || allowed == "ANY_" || allowed.as_bytes() == kind
                        })
                })
            });
            if absent_source || bad_target {
                diagnostics.note("link", || {
                    format!("native source {word:08X} is absent or has a disallowed target type")
                });
                key = None;
            }
        }
        let named = key
            .map_or(Ok(NamedFormKey::Null), |key| self.order.table().named(key))
            .map_err(|error| error.to_string())?;
        self.links.borrow_mut().push(ResolvedLink {
            subrecord: span.ordinal,
            signature: span.signature,
            byte_offset,
            source_word: word,
            key,
            outcome: if key.is_some() {
                LinkOutcome::Resolved
            } else if word == 0 {
                LinkOutcome::Null
            } else {
                LinkOutcome::Cleared
            },
        });
        Ok(Value::NativeFormKey(named))
    }
}

/// Merge diagnostic counters/events without retaining another copy of source payloads.
fn merge_diagnostics(target: &mut PluginDiagnostics, source: &PluginDiagnostics) {
    target.skipped_records += source.skipped_records;
    target.skipped_fields += source.skipped_fields;
    target.unexpected_subrecords += source.unexpected_subrecords;
    target.invalid_links += source.invalid_links;
    target.compatibility_headers += source.compatibility_headers;
    target.event_log.merge_from(&source.event_log);
    target.merge_summaries(source);
}

/// One immutable read contract is shared by initial decoding and catalogue replay.
struct ReadContext<'a> {
    order: &'a FormKeyOrder,
    schemas: &'a RuntimeSchemas,
    prepared: &'a decoder::PreparedSchemas,
    source_ids: &'a HashSet<FormKey>,
}

/// Catalogue replay can reject canonical roots without rewriting their physical source bytes.
#[derive(Clone, Copy, Default)]
struct ReplayContext<'a> {
    kinds: Option<&'a HashMap<FormKey, [u8; 4]>>,
    omitted: &'a [usize],
}

/// Candidate failure may identify a physical deferred root that can be omitted locally.
#[derive(Debug, Clone)]
struct CandidateFailure {
    reason: String,
    failed_root: Option<usize>,
}

impl From<String> for CandidateFailure {
    fn from(reason: String) -> Self {
        Self {
            reason,
            failed_root: None,
        }
    }
}

impl std::fmt::Display for CandidateFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.reason)
    }
}

/// Admission failures retain their stage and physical source without granting recovery authority.
fn validate_candidate(
    record: &NativeRecord,
    stage: &str,
    validate: &mut impl FnMut(&NativeRecord) -> std::result::Result<(), String>,
) -> std::result::Result<(), CandidateFailure> {
    validate(record).map_err(|reason| {
        format!(
            "unusable runtime candidate during {stage}: {} source {:08X}: {reason}",
            String::from_utf8_lossy(&record.source.record_type),
            record.source.form_id,
        )
        .into()
    })
}

/// INFO's semantic group owner requires a live DIAL; its physical group facts stay unchanged.
fn resolve_topic(
    decoded: &mut DecodedRecord,
    capture: &DecodeContext<'_>,
    diagnostics: &mut PluginDiagnostics,
) -> std::result::Result<Option<FormKey>, String> {
    let topic = decoded
        .topic_form_id
        .map(|word| capture.resolve_word(word))
        .transpose()?
        .flatten();
    let invalid = decoded.record_type == *b"INFO"
        && capture.kinds.is_some_and(|kinds| {
            topic.map_or(decoded.source_topic_form_id == Some(0), |key| {
                kinds.get(&key) != Some(b"DIAL")
            })
        });
    if !invalid {
        return Ok(topic);
    }
    diagnostics.set_record(
        decoded.source_record_offset,
        Some(decoded.record_type),
        Some(decoded.source_form_id),
        Some(decoded.form_id),
        "targets",
    );
    diagnostics.set_field("GRUP7".into());
    let source_word = decoded.source_topic_form_id.unwrap_or(0);
    diagnostics.link(source_word);
    diagnostics.note("link", || {
        if source_word == 0 {
            format!("INFO {:08X} GRUP7 has a null owner", decoded.source_form_id)
        } else {
            format!(
                "INFO {:08X} GRUP7 owner {source_word:08X} is absent or is not DIAL",
                decoded.source_form_id,
            )
        }
    });
    decoded.topic_form_id = None;
    Ok(None)
}

/// Decode source bytes and typed values using one prepared schema and native catalogue.
fn decode_candidate(
    scanned: scanner::ScannedRecord,
    supplier: usize,
    localized: bool,
    read: &ReadContext<'_>,
    replay: ReplayContext<'_>,
    diagnostics: &mut PluginDiagnostics,
    editor_id: &mut Option<String>,
) -> std::result::Result<NativeRecord, CandidateFailure> {
    let source = RawRecord {
        form_id: scanned.source_form_id,
        record_type: scanned.record_type,
        flags: scanned.flags,
        subrecords: decoder::native_source_fields(&scanned.payload),
        cell_form_id: scanned.cell_form_id,
        worldspace_form_id: scanned.worldspace_form_id,
        load_order: supplier as u32,
    };
    let capture = DecodeContext::new(read.order, supplier, read.source_ids, replay.kinds);
    let marker = HashMap::new();
    let context = decoder::Context {
        order: None,
        native: Some(&capture),
        plugin_index: supplier,
        localized,
        source_ids: None,
        winning_types: replay.kinds.map(|_| &marker),
    };
    // Localization is represented by original string IDs here; publication supplies resolved text.
    let mut decoded = decoder::decode_with_identity_omitting(
        scanned,
        read.schemas.schema(),
        read.prepared,
        &context,
        diagnostics,
        &mut |name| *editor_id = Some(name.to_ascii_lowercase()),
        replay.omitted,
    )
    .map_err(|reason| CandidateFailure {
        reason,
        failed_root: capture.failed_root.get(),
    })?;
    if replay.kinds.is_some() && decoder::record_contains_deferred(&decoded) {
        return Err(
            "native catalogue replay retained unresolved deferred values"
                .to_owned()
                .into(),
        );
    }
    let canonical = decoded.to_raw_record();
    let cell = source
        .cell_form_id
        .map(|word| capture.resolve_word(word))
        .transpose()?
        .flatten();
    let worldspace = source
        .worldspace_form_id
        .map(|word| capture.resolve_word(word))
        .transpose()?
        .flatten();
    let topic = resolve_topic(&mut decoded, &capture, diagnostics)?;
    Ok(NativeRecord {
        source,
        canonical,
        canonical_ordinals: capture.origins.into_inner(),
        cell,
        worldspace,
        topic,
        links: capture.links.into_inner(),
        decoded,
        localized,
    })
}

/// Rebuild the exact source scanner view for catalogue replay; no legacy bytes are synthesized.
fn scanned_again(record: &NativeRecord) -> scanner::ScannedRecord {
    scanner::ScannedRecord {
        record_type: record.source.record_type,
        flags: record.source.flags,
        version_control: record.decoded.version_control,
        form_version: record.decoded.form_version,
        header_unknown: record.decoded.header_unknown,
        source_form_id: record.source.form_id,
        cell_form_id: record.source.cell_form_id,
        worldspace_form_id: record.source.worldspace_form_id,
        source_topic_form_id: record.decoded.source_topic_form_id,
        topic_group_offset: record.decoded.topic_group_offset,
        source_record_offset: record.decoded.source_record_offset,
        payload: record.decoded.raw_payload.clone(),
    }
}

/// A tentative deferred override retains the earlier usable candidate until replay admission.
struct Candidate {
    record: NativeRecord,
    diagnostics: PluginDiagnostics,
    deferred: Vec<usize>,
    omitted: Vec<usize>,
}

/// Change only the physical occurrence admitted or rejected by catalogue replay.
fn set_replay_history(
    result: &mut NativeReadResult,
    record: &NativeRecord,
    accepted: bool,
    reason: String,
) {
    if let Some(entry) = result.history.iter_mut().find(|entry| {
        entry.supplier == record.source.load_order as usize
            && entry.source_offset == record.decoded.source_record_offset
    }) {
        entry.accepted = accepted;
        entry.reason = Some(reason);
    }
}

/// Decode and merge cap-free native typed identities using the production schema.
/// Missing/broken fields recover locally; only unusable input context or I/O returns an error.
pub fn read_plugins_form_keys(
    paths: &[PathBuf],
    order: &FormKeyOrder,
    schemas: &RuntimeSchemas,
) -> Result<NativeReadResult> {
    read_plugins_form_keys_with_validation(paths, order, schemas, |_| Ok(()))
}

/// Production projectors may reject one unusable candidate before it replaces its predecessor.
pub fn read_plugins_form_keys_with_validation(
    paths: &[PathBuf],
    order: &FormKeyOrder,
    schemas: &RuntimeSchemas,
    mut validate: impl FnMut(&NativeRecord) -> std::result::Result<(), String>,
) -> Result<NativeReadResult> {
    ensure!(
        paths.len() == order.plugin_order().len(),
        "native plugin/order length mismatch"
    );
    for (supplier, path) in paths.iter().enumerate() {
        let entry = order
            .table()
            .entry(order.plugin_order()[supplier])
            .expect("allocated supplier");
        ensure!(
            path.file_name().is_some_and(|name| name
                .to_string_lossy()
                .eq_ignore_ascii_case(&entry.display_name)),
            "native plugin/order name mismatch"
        );
    }
    let prepared = decoder::prepare_runtime_schema(schemas);
    let mut result = NativeReadResult::default();
    let mut source_ids = HashSet::new();
    let mut localized_plugins = Vec::with_capacity(paths.len());
    for (supplier, path) in paths.iter().enumerate() {
        let mut localized = false;
        scanner::scan_plugin_headers(path, &mut PluginDiagnostics::default(), |header| {
            if header.record_type == *b"TES4" {
                localized = header.flags & 0x80 != 0;
            }
            if header.record_type != *b"TES4"
                && let Ok(Some(key)) =
                    order.resolve(supplier, header.source_form_id, ZeroPolicy::Null)
            {
                source_ids.insert(key);
            }
        })?;
        localized_plugins.push(localized);
    }
    let mut candidates = HashMap::<FormKey, Vec<Candidate>>::new();
    let read = ReadContext {
        order,
        schemas,
        prepared: &prepared,
        source_ids: &source_ids,
    };
    let mut gmst_names = HashMap::<String, FormKey>::new();
    let mut gmst_aliases = HashMap::<FormKey, FormKey>::new();
    for (supplier, path) in paths.iter().enumerate() {
        let name = order
            .table()
            .entry(order.plugin_order()[supplier])
            .expect("allocated supplier")
            .normalized_name
            .clone();
        let mut plugin_diagnostics = PluginDiagnostics::default();
        let mut seen = HashSet::new();
        let history = RefCell::new(Vec::<NativeHistoryEntry>::new());
        scanner::scan_plugin(
            path,
            &mut plugin_diagnostics,
            |header| {
                history.borrow_mut().push(NativeHistoryEntry {
                    supplier,
                    source_word: header.source_form_id,
                    source_offset: header.source_record_offset,
                    signature: header.record_type,
                    flags: header.flags,
                    source_record_size: header.source_record_size,
                    source_group_end: header.source_group_end,
                    frame_complete: header.frame_complete,
                    source_cell_form_id: header.cell_form_id,
                    source_worldspace_form_id: header.worldspace_form_id,
                    source_topic_form_id: header.source_topic_form_id,
                    topic_group_offset: header.topic_group_offset,
                    editor_id: None,
                    key: order
                        .resolve(supplier, header.source_form_id, ZeroPolicy::Null)
                        .ok()
                        .flatten(),
                    deleted: header.flags & 0x20 != 0,
                    accepted: false,
                    reason: header.failure,
                });
            },
            |scanned, global| {
                let raw = scanned.source_form_id;
                let mut diagnostics = PluginDiagnostics::default();
                let mut observed_editor = None;
                let decoded = decode_candidate(
                    scanned,
                    supplier,
                    localized_plugins[supplier],
                    &read,
                    ReplayContext::default(),
                    &mut diagnostics,
                    &mut observed_editor,
                )
                .and_then(|record| {
                    if !record.source.is_deleted() && record.source.record_type != *b"TES4" {
                        validate_candidate(&record, "initial native admission", &mut validate)?;
                    }
                    Ok(record)
                });
                if let Some(last) = history.borrow_mut().last_mut()
                    && last.signature == *b"GMST"
                {
                    last.editor_id.clone_from(&observed_editor);
                    if let Some(key) = observed_editor
                        .as_ref()
                        .and_then(|editor| gmst_names.get(editor))
                    {
                        last.key = Some(*key);
                    }
                }
                let mut record = match decoded {
                    Ok(record) => record,
                    Err(error) => {
                        diagnostics.note("record", || error.reason.clone());
                        if let Some(last) = history.borrow_mut().last_mut() {
                            last.reason = Some(error.reason);
                        }
                        merge_diagnostics(global, &diagnostics);
                        return;
                    }
                };
                if record.source.record_type == *b"TES4" {
                    merge_diagnostics(global, &diagnostics);
                    result.headers.push(record);
                    return;
                }
                let native_key = order
                    .resolve(supplier, raw, ZeroPolicy::Null)
                    .ok()
                    .flatten();
                let mut key = native_key;
                let deleted = record.source.is_deleted();
                let mut gmst_update = None;
                let mut gmst_alias_update = None;
                let mut identity_failure = "record has a null or unusable native identity";
                if record.source.record_type == *b"GMST" {
                    identity_failure = "GMST has no usable native identity or EditorID";
                    let editor = record
                        .decoded
                        .fields
                        .iter()
                        .find_map(|field| {
                            (field.signature == *b"EDID")
                                .then(|| match &field.value {
                                    Value::String(name) => Some(name.to_ascii_lowercase()),
                                    _ => None,
                                })
                                .flatten()
                        })
                        .filter(|name| !name.is_empty());
                    if let Some(last) = history.borrow_mut().last_mut() {
                        last.editor_id.clone_from(&editor);
                    }
                    if let Some(editor) = editor {
                        let known = gmst_names.get(&editor).copied();
                        key = known.or(native_key);
                        if let (Some(source), Some(canonical)) = (native_key, key) {
                            if gmst_aliases
                                .get(&source)
                                .is_some_and(|old| known != Some(*old))
                                || candidates
                                    .get(&source)
                                    .and_then(|chain| chain.last())
                                    .is_some_and(|old| old.record.source.record_type != *b"GMST")
                            {
                                key = None;
                                identity_failure = "GMST has a colliding identity";
                            } else {
                                gmst_alias_update = Some((source, canonical));
                            }
                        }
                        if let Some(key) = key {
                            gmst_update = Some((editor, key));
                        }
                    } else if deleted {
                        key = native_key.and_then(|key| gmst_aliases.get(&key).copied());
                        identity_failure = "deleted GMST has no known identity";
                    } else {
                        key = None;
                    }
                } else if native_key.is_some_and(|key| gmst_aliases.contains_key(&key)) {
                    key = None;
                    identity_failure = "record has a colliding identity";
                }
                let duplicate_key = native_key.or(key);
                let reason = match key {
                    None => Some(identity_failure),
                    Some(_) if duplicate_key.is_some_and(|key| seen.contains(&key)) => {
                        Some("duplicate/colliding native record candidate in one supplier")
                    }
                    Some(key)
                        if !deleted
                            && !record.decoded.payload_complete
                            && candidates
                                .get(&key)
                                .and_then(|chain| chain.last())
                                .is_some_and(|old| old.record.decoded.payload_complete) =>
                    {
                        Some("incomplete override keeps the earlier complete version")
                    }
                    Some(_) => None,
                };
                if let Some(reason) = reason {
                    diagnostics.note("record", || format!("source {raw:08X}: {reason}"));
                    if let Some(last) = history.borrow_mut().last_mut() {
                        last.reason = Some(reason.into());
                    }
                    merge_diagnostics(global, &diagnostics);
                    return;
                }
                let key = key.expect("admitted key");
                if let Some(duplicate_key) = duplicate_key {
                    seen.insert(duplicate_key);
                }
                if let Some((name, key)) = gmst_update {
                    gmst_names.insert(name, key);
                }
                if let Some((source, canonical)) = gmst_alias_update {
                    gmst_aliases.insert(source, canonical);
                }
                if let Some(last) = history.borrow_mut().last_mut() {
                    last.key = Some(key);
                    last.accepted = true;
                    last.reason = None;
                }
                if deleted {
                    if let Some(previous) = candidates.remove(&key) {
                        for old in previous {
                            merge_diagnostics(global, &old.diagnostics);
                        }
                    }
                    merge_diagnostics(global, &diagnostics);
                } else {
                    // Native semantic identities never overwrite the original header word.
                    record.canonical.form_id = raw;
                    let pending = decoder::record_contains_deferred(&record.decoded);
                    let deferred = decoder::deferred_source_positions(&record.decoded);
                    let chain = candidates.entry(key).or_default();
                    if !pending {
                        for previous in chain.drain(..) {
                            merge_diagnostics(global, &previous.diagnostics);
                        }
                    }
                    chain.push(Candidate {
                        record,
                        diagnostics,
                        deferred,
                        omitted: Vec::new(),
                    });
                }
            },
        )?;
        result.history.extend(history.into_inner());
        result.diagnostics.insert(name, plugin_diagnostics);
    }
    // Catalogue-dependent candidates are replayed against admitted winners. A rejected
    // tentative candidate exposes its retained predecessor, then rebuilds the catalogue.
    loop {
        let kinds: HashMap<_, _> = candidates
            .iter()
            .filter_map(|(key, chain)| {
                chain
                    .last()
                    .map(|candidate| (*key, candidate.record.source.record_type))
            })
            .collect();
        let mut rejected = false;
        let mut keys: Vec<_> = candidates.keys().copied().collect();
        keys.sort_unstable();
        'keys: for key in keys {
            let chain = candidates
                .get_mut(&key)
                .expect("catalogued candidate chain");
            let first_definition = chain.len() == 1;
            let Some(candidate) = chain.last_mut() else {
                continue;
            };
            let supplier = candidate.record.source.load_order as usize;
            let mut diagnostics = PluginDiagnostics::default();
            let mut observed_editor = None;
            match decode_candidate(
                scanned_again(&candidate.record),
                supplier,
                candidate.record.localized,
                &read,
                ReplayContext {
                    kinds: Some(&kinds),
                    omitted: &candidate.omitted,
                },
                &mut diagnostics,
                &mut observed_editor,
            )
            .and_then(|record| {
                validate_candidate(&record, "native catalogue replay", &mut validate)?;
                Ok(record)
            }) {
                Ok(record) => {
                    candidate.record = record;
                    candidate.diagnostics = diagnostics;
                }
                Err(error) => {
                    let deferred = !candidate.deferred.is_empty();
                    let mut reason = if deferred {
                        format!("deferred native catalogue replay rejected {key:?}: {error}")
                    } else {
                        format!("native catalogue replay rejected {key:?}: {error}")
                    };
                    if first_definition && deferred {
                        let mut omitted = candidate.omitted.clone();
                        let mut failure = error;
                        while let Some(position) = failure.failed_root.filter(|position| {
                            candidate.deferred.contains(position) && !omitted.contains(position)
                        }) {
                            omitted.push(position);
                            omitted.sort_unstable();
                            let mut recovered_diagnostics = PluginDiagnostics::default();
                            let recovered = decode_candidate(
                                scanned_again(&candidate.record),
                                supplier,
                                candidate.record.localized,
                                &read,
                                ReplayContext {
                                    kinds: Some(&kinds),
                                    omitted: &omitted,
                                },
                                &mut recovered_diagnostics,
                                &mut observed_editor,
                            )
                            .and_then(|record| {
                                if record.decoded.fields.is_empty() {
                                    return Err("no safe fields remain after deferred recovery"
                                        .to_owned()
                                        .into());
                                }
                                validate_candidate(
                                    &record,
                                    "deferred native recovery",
                                    &mut validate,
                                )?;
                                Ok(record)
                            });
                            match recovered {
                                Ok(record) => {
                                    let fields = omitted
                                        .iter()
                                        .filter_map(|&position| {
                                            candidate.record.source.subrecords.get(position).map(
                                                |(signature, _)| {
                                                    format!(
                                                        "{}[source:{position}]",
                                                        String::from_utf8_lossy(signature)
                                                    )
                                                },
                                            )
                                        })
                                        .collect::<Vec<_>>()
                                        .join(", ");
                                    set_replay_history(
                                        &mut result,
                                        &record,
                                        true,
                                        format!("deferred recovery omitted {fields}: {reason}"),
                                    );
                                    candidate.record = record;
                                    candidate.diagnostics = recovered_diagnostics;
                                    candidate.omitted = omitted;
                                    rejected = true;
                                    break 'keys;
                                }
                                Err(error) => {
                                    reason =
                                        format!("{reason}; deferred recovery unusable: {error}");
                                    failure = error;
                                }
                            }
                        }
                    }
                    diagnostics.set_record(
                        candidate.record.decoded.source_record_offset,
                        Some(candidate.record.source.record_type),
                        Some(candidate.record.source.form_id),
                        Some(candidate.record.decoded.form_id),
                        "admission",
                    );
                    diagnostics.note("record", || reason.clone());
                    let name = &order
                        .table()
                        .entry(order.plugin_order()[supplier])
                        .expect("supplier")
                        .normalized_name;
                    merge_diagnostics(
                        result.diagnostics.entry(name.clone()).or_default(),
                        &diagnostics,
                    );
                    let removed = chain.pop().expect("selected candidate");
                    set_replay_history(&mut result, &removed.record, false, reason);
                    rejected = true;
                    // Later dependents must not replay against a rejected winner's old kind.
                    break;
                }
            }
        }
        if !rejected {
            break;
        }
    }
    for (key, mut chain) in candidates {
        let Some(winner) = chain.pop() else {
            continue;
        };
        for candidate in chain.into_iter().chain(std::iter::once(winner)) {
            let name = &order
                .table()
                .entry(order.plugin_order()[candidate.record.source.load_order as usize])
                .expect("supplier")
                .normalized_name;
            merge_diagnostics(
                result.diagnostics.entry(name.clone()).or_default(),
                &candidate.diagnostics,
            );
            result.records.insert(key, candidate.record);
        }
    }
    Ok(result)
}

#[cfg(test)]
#[path = "native_tests.rs"]
mod tests;
