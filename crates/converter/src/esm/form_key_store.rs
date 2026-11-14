//! Native source archive: pack-scoped keys plus physical reference occurrences.
//!
//! This is a diagnostic source archive, not a runtime world database. Native payloads retain
//! four-byte file-relative words. Only the separately authenticated keys are semantic identities.
use super::{EsmParser, form_key_merge::FormKeyConversion, records::RawRecord};
use color_eyre::{Result, eyre::ensure};
use rusqlite::{Connection, params};
use shared::{
    form_key::{FormKey, PluginId},
    pack_identity::PackIdentity,
};
use std::path::PathBuf;

/// Source archive version, independent of runtime world and native plugin versions.
pub const SOURCE_ARCHIVE_VERSION: u32 = 1;

/// One restored source winner. Raw words always belong to `supplier`, not to its owner.
#[derive(Debug)]
pub struct SourceArchiveRecord {
    pub key: FormKey,
    pub supplier: PluginId,
    pub source: RawRecord,
    pub cell: Option<FormKey>,
    pub worldspace: Option<FormKey>,
    pub links: Vec<super::form_key_merge::ResolvedLink>,
}

/// A complete source snapshot, ordered by the original full/light owner order and local ID.
#[derive(Debug)]
pub struct SourceArchive {
    pub identity: PackIdentity,
    pub records: Vec<SourceArchiveRecord>,
    pub warnings: Vec<String>,
    pub notices: Vec<String>,
}

impl EsmParser {
    /// Publish native source winners without invoking a legacy writer or allocating runtime slots.
    /// Destination contents are committed together, or left unchanged on any storage failure.
    pub fn export_form_key_source_archive(
        paths: &[PathBuf],
        connection: &mut Connection,
    ) -> Result<()> {
        Self::merge_plugins_form_keys(paths)?.write_source_archive(connection)
    }
}

impl FormKeyConversion {
    /// Write a fresh diagnostic archive. Existing worlds/archives are refused before mutation.
    /// This deliberately does not stamp source bytes as a current runtime world format.
    pub fn write_source_archive(&self, connection: &mut Connection) -> Result<()> {
        let identity = self.order.pack_identity()?;
        let mut ordered = self
            .records
            .iter()
            .map(|(key, record)| Ok((self.order.identity_sort_tuple(*key)?, *key, record)))
            .collect::<Result<Vec<_>>>()?;
        ordered.sort_unstable_by_key(|row| row.0);
        let transaction = connection.transaction()?;
        let occupied: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name IN ('records','source_archive','source_records','source_fields','source_links','source_messages'))",
            [],
            |row| row.get(0),
        )?;
        ensure!(
            !occupied,
            "source archive requires a fresh destination; retain existing output"
        );
        identity.write_sql(&transaction)?;
        transaction.execute_batch(
            "CREATE TABLE source_archive (singleton INTEGER PRIMARY KEY CHECK(singleton=1), version INTEGER NOT NULL, scope TEXT NOT NULL CHECK(scope='diagnostic_source_only'));
             CREATE TABLE source_records (key INTEGER PRIMARY KEY, sequence INTEGER NOT NULL UNIQUE, supplier INTEGER NOT NULL REFERENCES pack_plugins(id), source_word INTEGER NOT NULL, signature BLOB NOT NULL CHECK(length(signature)=4), flags INTEGER NOT NULL, source_cell INTEGER, source_world INTEGER, cell_key INTEGER, world_key INTEGER);
             CREATE TABLE source_fields (record_key INTEGER NOT NULL REFERENCES source_records(key), ordinal INTEGER NOT NULL, signature BLOB NOT NULL CHECK(length(signature)=4), data BLOB NOT NULL, PRIMARY KEY(record_key,ordinal));
             CREATE TABLE source_links (record_key INTEGER NOT NULL, field_ordinal INTEGER NOT NULL, byte_offset INTEGER NOT NULL, source_word INTEGER NOT NULL, target_key INTEGER, PRIMARY KEY(record_key,field_ordinal,byte_offset), FOREIGN KEY(record_key,field_ordinal) REFERENCES source_fields(record_key,ordinal));
             CREATE TABLE source_messages (severity TEXT NOT NULL CHECK(severity IN ('warning','notice')), ordinal INTEGER NOT NULL, message TEXT NOT NULL, PRIMARY KEY(severity,ordinal));",
        )?;
        transaction.execute(
            "INSERT INTO source_archive VALUES(1,?1,'diagnostic_source_only')",
            [SOURCE_ARCHIVE_VERSION],
        )?;
        {
            let mut records = transaction
                .prepare("INSERT INTO source_records VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)")?;
            let mut fields =
                transaction.prepare("INSERT INTO source_fields VALUES(?1,?2,?3,?4)")?;
            let mut links =
                transaction.prepare("INSERT INTO source_links VALUES(?1,?2,?3,?4,?5)")?;
            for (sequence, (_, key, record)) in ordered.into_iter().enumerate() {
                let supplier = *identity
                    .order()
                    .get(record.source.load_order as usize)
                    .ok_or_else(|| color_eyre::eyre::eyre!("unknown source supplier"))?;
                identity.optional_key(record.cell.map(FormKey::to_sql_i64))?;
                identity.optional_key(record.worldspace.map(FormKey::to_sql_i64))?;
                records.execute(params![
                    key.to_sql_i64(),
                    sequence as i64,
                    supplier.get(),
                    record.source.form_id,
                    record.source.record_type.as_slice(),
                    record.source.flags,
                    record.source.cell_form_id,
                    record.source.worldspace_form_id,
                    record.cell.map(FormKey::to_sql_i64),
                    record.worldspace.map(FormKey::to_sql_i64),
                ])?;
                for (ordinal, (signature, data)) in record.source.subrecords.iter().enumerate() {
                    fields.execute(params![key.to_sql_i64(), ordinal as i64, signature, data])?;
                }
                for link in &record.links {
                    validate_site(&record.source, link)?;
                    validate_link_context(&identity, supplier, link)?;
                    links.execute(params![
                        key.to_sql_i64(),
                        link.subrecord as i64,
                        link.byte_offset as i64,
                        link.source_word,
                        link.key.map(FormKey::to_sql_i64)
                    ])?;
                }
            }
            let mut messages =
                transaction.prepare("INSERT INTO source_messages VALUES(?1,?2,?3)")?;
            for (severity, values) in [("warning", &self.warnings), ("notice", &self.notices)] {
                for (ordinal, message) in values.iter().enumerate() {
                    messages.execute(params![severity, ordinal as i64, message])?;
                }
            }
        }
        transaction.commit()?;
        Ok(())
    }
}

/// Check every physical site against its unmodified containing source field.
fn validate_site(source: &RawRecord, link: &super::form_key_merge::ResolvedLink) -> Result<()> {
    let field = source.subrecords.get(link.subrecord);
    let bytes = field.and_then(|(_, data)| {
        link.byte_offset
            .checked_add(4)
            .and_then(|end| data.get(link.byte_offset..end))
    });
    ensure!(
        field.is_some_and(|(signature, _)| signature.as_slice() == link.signature)
            && bytes == Some(link.source_word.to_le_bytes().as_slice()),
        "source link does not match its physical field occurrence"
    );
    Ok(())
}

/// A valid integer naming another owner/local is still the wrong resolution of this source word.
fn validate_link_context(
    identity: &PackIdentity,
    supplier: PluginId,
    link: &super::form_key_merge::ResolvedLink,
) -> Result<()> {
    let entry = identity
        .table()
        .entry(supplier)
        .ok_or_else(|| color_eyre::eyre::eyre!("unknown source supplier"))?;
    let owner = match entry.facts.masters.get((link.source_word >> 24) as usize) {
        Some(master) => identity
            .table()
            .id(master)
            .ok_or_else(|| color_eyre::eyre::eyre!("unknown source master"))?,
        None => supplier,
    };
    let expected = if link.source_word == 0 {
        None
    } else {
        Some(FormKey::new(owner, link.source_word & 0x00ff_ffff)?)
    };
    ensure!(
        link.key == expected,
        "source link disagrees with its supplier and original master list"
    );
    Ok(())
}

impl SourceArchive {
    /// Authenticate plugin context and archive version before exposing any source rows or keys.
    pub fn read(connection: &Connection) -> Result<Self> {
        let (version, scope): (u32, String) = connection.query_row(
            "SELECT version,scope FROM source_archive WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        ensure!(
            version == SOURCE_ARCHIVE_VERSION && scope == "diagnostic_source_only",
            "unsupported source archive; convert again"
        );
        let identity = PackIdentity::read_sql(connection)?;
        ensure!(
            connection
                .prepare("PRAGMA foreign_key_check")?
                .query([])?
                .next()?
                .is_none(),
            "source archive has orphaned rows"
        );
        let mut records = Vec::new();
        let mut statement = connection.prepare("SELECT key,sequence,supplier,source_word,signature,flags,source_cell,source_world,cell_key,world_key FROM source_records ORDER BY sequence")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let key = identity.required_key(row.get(0)?)?;
            ensure!(
                row.get::<_, i64>(1)? == records.len() as i64,
                "invalid source sequence"
            );
            let supplier = PluginId::new(row.get(2)?)?;
            let priority = identity
                .order()
                .iter()
                .position(|id| *id == supplier)
                .ok_or_else(|| color_eyre::eyre::eyre!("unknown source supplier"))?;
            let signature: Vec<u8> = row.get(4)?;
            let mut source = RawRecord {
                form_id: row.get(3)?,
                record_type: signature
                    .try_into()
                    .map_err(|_| color_eyre::eyre::eyre!("invalid record signature"))?,
                flags: row.get(5)?,
                cell_form_id: row.get(6)?,
                worldspace_form_id: row.get(7)?,
                load_order: priority as u32,
                subrecords: Vec::new(),
            };
            let mut fields = connection.prepare("SELECT ordinal,signature,data FROM source_fields WHERE record_key=?1 ORDER BY ordinal")?;
            let mut fields = fields.query([key.to_sql_i64()])?;
            while let Some(field) = fields.next()? {
                ensure!(
                    field.get::<_, i64>(0)? == source.subrecords.len() as i64,
                    "invalid source field sequence"
                );
                let signature: Vec<u8> = field.get(1)?;
                ensure!(signature.len() == 4, "invalid source field signature");
                source.subrecords.push((signature, field.get(2)?));
            }
            let mut links = Vec::new();
            let mut sites = connection.prepare("SELECT field_ordinal,byte_offset,source_word,target_key FROM source_links WHERE record_key=?1 ORDER BY field_ordinal,byte_offset")?;
            let mut sites = sites.query([key.to_sql_i64()])?;
            while let Some(site) = sites.next()? {
                let subrecord: usize = site.get(0)?;
                let signature = source
                    .subrecords
                    .get(subrecord)
                    .ok_or_else(|| color_eyre::eyre::eyre!("unknown source field occurrence"))?
                    .0
                    .as_slice()
                    .try_into()?;
                let link = super::form_key_merge::ResolvedLink {
                    subrecord,
                    signature,
                    byte_offset: site.get(1)?,
                    source_word: site.get(2)?,
                    key: identity.optional_key(site.get(3)?)?,
                };
                validate_site(&source, &link)?;
                validate_link_context(&identity, supplier, &link)?;
                links.push(link);
            }
            records.push(SourceArchiveRecord {
                key,
                supplier,
                source,
                cell: identity.optional_key(row.get(8)?)?,
                worldspace: identity.optional_key(row.get(9)?)?,
                links,
            });
        }
        let messages = |severity| -> Result<Vec<String>> {
            let mut statement = connection.prepare(
                "SELECT message FROM source_messages WHERE severity=?1 ORDER BY ordinal",
            )?;
            Ok(statement
                .query_map([severity], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        };
        Ok(Self {
            identity,
            records,
            warnings: messages("warning")?,
            notices: messages("notice")?,
        })
    }

    /// Portable inspection JSON names every semantic identity, including nested reference targets.
    /// Explicit source words remain numeric native facts, never unqualified packed keys.
    pub fn portable_json(&self) -> Result<serde_json::Value> {
        let named =
            |key: Option<FormKey>| key.map(|key| self.identity.table().named(key)).transpose();
        let mut records = Vec::with_capacity(self.records.len());
        for record in &self.records {
            let mut links = Vec::with_capacity(record.links.len());
            for link in &record.links {
                links.push(serde_json::json!({
                    "subrecord": link.subrecord, "signature": String::from_utf8_lossy(&link.signature),
                    "byte_offset": link.byte_offset, "source_word": link.source_word, "target": named(link.key)?,
                }));
            }
            records.push(serde_json::json!({
                "key": named(Some(record.key))?,
                "supplier": self.identity.table().entry(record.supplier).expect("validated supplier").display_name,
                "source_word": record.source.form_id, "cell": named(record.cell)?, "worldspace": named(record.worldspace)?, "links": links,
            }));
        }
        Ok(
            serde_json::json!({ "version": SOURCE_ARCHIVE_VERSION, "scope": "diagnostic_source_only", "records": records, "warnings": self.warnings, "notices": self.notices }),
        )
    }
}
