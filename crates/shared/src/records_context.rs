//! Authenticated canonical native bytes and physical semantic links, shared by all pack readers.
use crate::{
    form_key::{FormKey, PluginId},
    pack_identity::PackIdentity,
    records::ArchivedRecordData,
};
use color_eyre::{
    Result,
    eyre::{WrapErr, ensure, eyre},
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::collections::BTreeMap;

/// Canonical bytes keep native layouts; this version owns their contextual interpretation.
pub const RECORD_CONTEXT_VERSION: u32 = 1;

/// Install the paired context relation in the same transaction as canonical records.
pub fn create_record_context(tx: &Transaction<'_>, identity: &PackIdentity) -> Result<()> {
    identity.write_sql(tx)?;
    tx.execute_batch("CREATE TABLE canonical_record_contract(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL,identity_digest BLOB NOT NULL);
        CREATE TABLE record_context(record_key INTEGER PRIMARY KEY REFERENCES records(form_id),supplier INTEGER NOT NULL REFERENCES pack_plugins(id),source_header INTEGER NOT NULL,source_cell INTEGER,source_world INTEGER,source_field_count INTEGER NOT NULL);
        CREATE TABLE record_field_origins(record_key INTEGER NOT NULL REFERENCES records(form_id),canonical_ordinal INTEGER NOT NULL,source_ordinal INTEGER NOT NULL,signature BLOB NOT NULL CHECK(length(signature)=4),runtime_usable INTEGER NOT NULL CHECK(runtime_usable IN (0,1)),PRIMARY KEY(record_key,canonical_ordinal),UNIQUE(record_key,source_ordinal));
        CREATE TABLE record_source_fields(record_key INTEGER NOT NULL REFERENCES records(form_id),source_ordinal INTEGER NOT NULL,signature BLOB NOT NULL CHECK(length(signature)=4),data BLOB NOT NULL,PRIMARY KEY(record_key,source_ordinal));
        CREATE TABLE resolved_links(record_key INTEGER NOT NULL,source_ordinal INTEGER NOT NULL,byte_offset INTEGER NOT NULL,source_word INTEGER NOT NULL,target_key INTEGER,outcome TEXT NOT NULL CHECK(outcome IN ('resolved','null','cleared')),PRIMARY KEY(record_key,source_ordinal,byte_offset),FOREIGN KEY(record_key,source_ordinal) REFERENCES record_source_fields(record_key,source_ordinal));")?;
    tx.execute_batch("CREATE TABLE land_context(cell_key INTEGER PRIMARY KEY,record_key INTEGER NOT NULL REFERENCES records(form_id)); CREATE TABLE record_text(record_key INTEGER NOT NULL REFERENCES records(form_id),source_ordinal INTEGER NOT NULL,text TEXT,PRIMARY KEY(record_key,source_ordinal));")?;
    tx.execute(
        "INSERT INTO canonical_record_contract VALUES(1,?1,?2)",
        params![RECORD_CONTEXT_VERSION, identity.digest().as_slice()],
    )?;
    Ok(())
}

/// One canonical record with original source context, never legacy packed links.
#[derive(Debug)]
pub struct ContextualRecord {
    pub key: FormKey,
    pub supplier: PluginId,
    pub source_header: u32,
    pub source_cell: Option<u32>,
    pub source_world: Option<u32>,
    pub subrecords: Vec<crate::records::ArchivedSubRecord>,
    pub source_ordinals: Vec<usize>,
    usable: Vec<bool>,
    links: BTreeMap<(usize, usize), Option<FormKey>>,
    text: BTreeMap<usize, Option<String>>,
}
impl ContextualRecord {
    /// Opaque source evidence is never certified as a runtime scalar field.
    pub fn is_runtime_usable(&self, ordinal: usize) -> bool {
        self.usable.get(ordinal) == Some(&true)
    }
    /// Borrow certified scalar fields while preserving original canonical ordinals elsewhere.
    pub fn runtime_subrecords(&self) -> impl Iterator<Item = &crate::records::ArchivedSubRecord> {
        self.subrecords
            .iter()
            .enumerate()
            .filter(|(index, _)| self.is_runtime_usable(*index))
            .map(|(_, field)| field)
    }
    /// Resolve only an authenticated recognized occurrence; absent metadata is a format error.
    pub fn resolve_at(
        &self,
        canonical_ordinal: usize,
        byte_offset: usize,
    ) -> Result<Option<FormKey>> {
        if self.usable.get(canonical_ordinal) == Some(&false) {
            return Ok(None);
        }
        let source = *self
            .source_ordinals
            .get(canonical_ordinal)
            .ok_or_else(|| eyre!("unknown canonical occurrence"))?;
        self.links
            .get(&(source, byte_offset))
            .copied()
            .ok_or_else(|| eyre!("missing canonical link site; convert again"))
    }
    /// Read the first occurrence of a known native link field.
    pub fn first_link(&self, signature: [u8; 4], byte_offset: usize) -> Result<Option<FormKey>> {
        match self
            .subrecords
            .iter()
            .enumerate()
            .position(|(index, field)| field.tag == signature && self.is_runtime_usable(index))
        {
            Some(index) => self.resolve_at(index, byte_offset),
            None => Ok(None),
        }
    }
    /// Inspect scalar bytes without interpreting them as pack keys.
    pub fn bytes(&self, signature: [u8; 4]) -> Option<&[u8]> {
        self.subrecords
            .iter()
            .enumerate()
            .find(|(index, field)| field.tag == signature && self.usable[*index])
            .map(|(_, field)| field.data.as_slice())
    }
    /// Text projections preserve localization and source character decoding independently of bytes.
    pub fn text(&self, signature: [u8; 4]) -> Option<String> {
        let index = self
            .subrecords
            .iter()
            .enumerate()
            .position(|(index, field)| field.tag == signature && self.usable[index])?;
        match self.text.get(&self.source_ordinals[index]) {
            Some(text) => text.clone(),
            None => Some(
                String::from_utf8_lossy(&self.subrecords[index].data)
                    .trim_end_matches('\0')
                    .to_owned(),
            ),
        }
    }
}

/// Validated table scope precedes every key or rkyv payload read.
pub struct RecordContextReader<'a> {
    connection: &'a Connection,
    identity: PackIdentity,
}
impl<'a> RecordContextReader<'a> {
    /// Reject unbound or mixed generations before touching a canonical payload.
    pub fn new(connection: &'a Connection) -> Result<Self> {
        let identity = PackIdentity::read_sql(connection)?;
        let contract: Option<(u32, Vec<u8>)> = connection
            .query_row(
                "SELECT version,identity_digest FROM canonical_record_contract WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .wrap_err("canonical context is out of date; convert again")?;
        ensure!(
            contract.is_some_and(|(version, digest)| version == RECORD_CONTEXT_VERSION
                && digest.as_slice() == identity.digest()),
            "canonical context is out of date; convert again"
        );
        Ok(Self {
            connection,
            identity,
        })
    }
    /// Expose the already authenticated immutable allocation.
    pub fn identity(&self) -> &PackIdentity {
        &self.identity
    }
    /// Resolve the winning LAND context before interpreting its native VTEX bytes.
    pub fn land(&self, cell: FormKey) -> Result<ContextualRecord> {
        self.identity.required_key(cell.to_sql_i64())?;
        let land: i64 = self.connection.query_row(
            "SELECT record_key FROM land_context WHERE cell_key=?1",
            [cell.to_sql_i64()],
            |row| row.get(0),
        )?;
        self.record(self.identity.required_key(land)?)
    }

    /// Restore native bytes, source origins and explicitly resolved semantic outcomes together.
    pub fn record(&self, key: FormKey) -> Result<ContextualRecord> {
        self.identity.required_key(key.to_sql_i64())?;
        let (supplier,source_header,source_cell,source_world,source_count):(u32,u32,Option<u32>,Option<u32>,usize)=self.connection.query_row("SELECT supplier,source_header,source_cell,source_world,source_field_count FROM record_context WHERE record_key=?1",[key.to_sql_i64()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)))?;
        let supplier = PluginId::new(supplier)?;
        ensure!(
            self.identity.order().contains(&supplier),
            "unknown record supplier; convert again"
        );
        let (signature, cell, world, blob): (String, Option<i64>, Option<i64>, Vec<u8>) =
            self.connection.query_row(
                "SELECT record_type,cell_id,worldspace_id,data FROM records WHERE form_id=?1",
                [key.to_sql_i64()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
        ensure!(
            signature == "GMST"
                || source_key(&self.identity, supplier, source_header)? == Some(key),
            "record owner disagrees with original supplier/header; convert again"
        );
        ensure!(
            self.identity.optional_key(cell)?
                == source_cell
                    .map(|raw| source_key(&self.identity, supplier, raw))
                    .transpose()?
                    .flatten(),
            "cell context disagrees with original supplier/group; convert again"
        );
        ensure!(
            self.identity.optional_key(world)?
                == source_world
                    .map(|raw| source_key(&self.identity, supplier, raw))
                    .transpose()?
                    .flatten(),
            "world context disagrees with original supplier/group; convert again"
        );
        let mut aligned = rkyv::util::AlignedVec::<16>::new();
        aligned.extend_from_slice(&blob);
        let data = rkyv::from_bytes::<ArchivedRecordData, rkyv::rancor::Error>(&aligned)
            .wrap_err("invalid canonical record payload; convert again")?;
        let mut source_ordinals = Vec::new();
        let mut usable = Vec::new();
        let mut origins=self.connection.prepare("SELECT canonical_ordinal,source_ordinal,signature,runtime_usable FROM record_field_origins WHERE record_key=?1 ORDER BY canonical_ordinal")?;
        let mut origins = origins.query([key.to_sql_i64()])?;
        while let Some(row) = origins.next()? {
            let ordinal: usize = row.get(0)?;
            let source: usize = row.get(1)?;
            let signature: Vec<u8> = row.get(2)?;
            ensure!(
                ordinal == source_ordinals.len()
                    && source < source_count
                    && data
                        .subrecords
                        .get(ordinal)
                        .is_some_and(|field| field.tag.as_slice() == signature),
                "invalid canonical source origin; convert again"
            );
            let (source_signature,source_bytes):(Vec<u8>,Vec<u8>)=self.connection.query_row("SELECT signature,data FROM record_source_fields WHERE record_key=?1 AND source_ordinal=?2",params![key.to_sql_i64(),source as i64],|row|Ok((row.get(0)?,row.get(1)?)))?;
            ensure!(
                source_signature == signature && source_bytes == data.subrecords[ordinal].data,
                "canonical bytes disagree with native evidence; convert again"
            );
            source_ordinals.push(source);
            usable.push(row.get::<_, bool>(3)?);
        }
        ensure!(
            source_ordinals.len() == data.subrecords.len(),
            "incomplete canonical context; convert again"
        );
        let mut links = BTreeMap::new();
        let mut sites=self.connection.prepare("SELECT source_ordinal,byte_offset,source_word,target_key,outcome FROM resolved_links WHERE record_key=?1 ORDER BY source_ordinal,byte_offset")?;
        let mut sites = sites.query([key.to_sql_i64()])?;
        while let Some(row) = sites.next()? {
            let source: usize = row.get(0)?;
            let offset: usize = row.get(1)?;
            let raw: u32 = row.get(2)?;
            let target = self.identity.optional_key(row.get(3)?)?;
            let outcome: String = row.get(4)?;
            let canonical = source_ordinals
                .iter()
                .position(|value| *value == source)
                .ok_or_else(|| eyre!("link outside canonical occurrences"))?;
            ensure!(
                offset
                    .checked_add(4)
                    .and_then(|end| data.subrecords[canonical].data.get(offset..end))
                    == Some(raw.to_le_bytes().as_slice()),
                "link disagrees with physical source bytes; convert again"
            );
            ensure!(
                match outcome.as_str() {
                    "resolved" =>
                        target.is_some() && target == source_key(&self.identity, supplier, raw)?,
                    "null" => raw == 0 && target.is_none(),
                    "cleared" => raw != 0 && target.is_none(),
                    _ => false,
                },
                "link outcome disagrees with supplier context; convert again"
            );
            links.insert((source, offset), target);
        }
        let mut text = BTreeMap::new();
        let mut fields = self
            .connection
            .prepare("SELECT source_ordinal,text FROM record_text WHERE record_key=?1")?;
        for row in fields.query_map([key.to_sql_i64()], |row| {
            Ok((row.get::<_, usize>(0)?, row.get::<_, Option<String>>(1)?))
        })? {
            let (ordinal, value) = row?;
            text.insert(ordinal, value);
        }
        Ok(ContextualRecord {
            key,
            supplier,
            source_header,
            source_cell,
            source_world,
            subrecords: data.subrecords,
            source_ordinals,
            usable,
            links,
            text,
        })
    }
}

/// Native converter ownership permits original light locals and every at/past-MAST self selector.
pub fn source_key(
    identity: &PackIdentity,
    supplier: PluginId,
    raw: u32,
) -> Result<Option<FormKey>> {
    let entry = identity
        .table()
        .entry(supplier)
        .ok_or_else(|| eyre!("unknown source supplier"))?;
    if raw == 0 {
        return Ok(None);
    }
    let owner = match entry.facts.masters.get((raw >> 24) as usize) {
        Some(name) => identity
            .table()
            .id(name)
            .ok_or_else(|| eyre!("unknown original source master"))?,
        None => supplier,
    };
    Ok(Some(FormKey::new(owner, raw & 0x00ff_ffff)?))
}
