//! Authenticated plugin allocation shared by all identity-bearing pack members.
//!
//! A packed integer is meaningful only after this contract has been validated.
//! This module never translates old worlds or assigns legacy runtime slots.
use crate::{
    form_key::{FormKey, PluginId},
    plugin_table::{PluginEntry, PluginFacts, PluginFlags, PluginTable},
};
use color_eyre::{
    Result,
    eyre::{WrapErr, ensure, eyre},
};
use rusqlite::{Connection, Transaction, params};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

/// First persisted owner/local table contract; unrelated to supplier priority.
pub const PACK_IDENTITY_VERSION: u32 = 1;
/// Fixed envelope size, independent of any archived Rust root layout.
pub const IDENTITY_ENVELOPE_SIZE: usize = 96;
const MAGIC: &[u8; 8] = b"MCFKPACK";

/// Immutable plugin allocation and source priority authenticated as one identity scope.
#[derive(Debug, Clone)]
pub struct PackIdentity {
    table: PluginTable,
    order: Vec<PluginId>,
    digest: [u8; 32],
}

impl PackIdentity {
    /// Validate a complete source order and compute a reproducible content-bound identity.
    /// Missing source proofs are packaging errors, not errors in an individual mod field.
    pub fn new(table: PluginTable, order: Vec<PluginId>) -> Result<Self> {
        let mut seen = HashSet::new();
        for &id in &order {
            let entry = table
                .entry(id)
                .ok_or_else(|| eyre!("unknown ordered plugin {id:?}"))?;
            ensure!(
                entry.loaded && seen.insert(id),
                "invalid or duplicate source priority {id:?}"
            );
        }
        for entry in table.entries() {
            ensure!(
                entry.loaded == seen.contains(&entry.id),
                "incomplete loaded plugin order"
            );
            ensure!(
                !entry.loaded || entry.facts.content_hash.is_some(),
                "plugin {} has no content proof",
                entry.display_name
            );
        }
        let digest = mapping_digest(&table, &order);
        Ok(Self {
            table,
            order,
            digest,
        })
    }

    /// Authenticated owner/name allocation; cannot be mutated independently of its proof.
    pub fn table(&self) -> &PluginTable {
        &self.table
    }

    /// Supplying priority, deliberately separate from the allocation sequence.
    pub fn order(&self) -> &[PluginId] {
        &self.order
    }

    /// Exact table, facts, content and priority proof for every reusable pack member.
    pub fn digest(&self) -> [u8; 32] {
        self.digest
    }

    /// Validate a stored required key and its membership before exposing it to callers.
    pub fn required_key(&self, value: i64) -> Result<FormKey> {
        let key = FormKey::from_sql_i64(value)?;
        ensure!(
            self.table.entry(key.plugin()).is_some(),
            "key has unknown plugin owner"
        );
        Ok(key)
    }

    /// SQL NULL alone means an absent optional link; SQL zero is an invalid stored key.
    pub fn optional_key(&self, value: Option<i64>) -> Result<Option<FormKey>> {
        value.map(|value| self.required_key(value)).transpose()
    }

    /// Write the allocation in the caller's snapshot transaction.
    /// Existing allocations must agree exactly; this API never migrates or relabels a pack.
    pub fn write_sql(&self, transaction: &Transaction<'_>) -> Result<()> {
        let exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='pack_identity')",
            [],
            |row| row.get(0),
        )?;
        if exists {
            let previous = Self::read_sql(transaction)?;
            ensure!(
                previous.digest == self.digest,
                "pack identity differs; convert into a fresh world generation"
            );
            return Ok(());
        }
        transaction.execute_batch(
            "CREATE TABLE pack_identity (singleton INTEGER PRIMARY KEY CHECK(singleton=1), version INTEGER NOT NULL, digest BLOB NOT NULL CHECK(length(digest)=32));
             CREATE TABLE pack_plugins (id INTEGER PRIMARY KEY CHECK(id>0), normalized_name TEXT NOT NULL UNIQUE, display_name TEXT NOT NULL, loaded INTEGER NOT NULL CHECK(loaded IN (0,1)), flags INTEGER NOT NULL CHECK(flags BETWEEN 0 AND 7), header_version_bits INTEGER, masters_json TEXT NOT NULL, content_hash BLOB, supplier_priority INTEGER UNIQUE);"
        )?;
        let priorities: std::collections::HashMap<_, _> = self
            .order
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, i as u32))
            .collect();
        let mut insert =
            transaction.prepare("INSERT INTO pack_plugins VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)")?;
        for entry in self.table.entries() {
            let flags = u8::from(entry.facts.flags.master)
                | (u8::from(entry.facts.flags.light) << 1)
                | (u8::from(entry.facts.flags.medium) << 2);
            insert.execute(params![
                entry.id.get(),
                entry.normalized_name,
                entry.display_name,
                entry.loaded,
                flags,
                entry.facts.header_version.map(f32::to_bits),
                serde_json::to_string(&entry.facts.masters)?,
                entry.facts.content_hash.as_ref().map(|v| v.as_slice()),
                priorities.get(&entry.id)
            ])?;
        }
        transaction.execute(
            "INSERT INTO pack_identity VALUES (1,?1,?2)",
            params![PACK_IDENTITY_VERSION, self.digest.as_slice()],
        )?;
        Ok(())
    }

    /// Restore and authenticate a table before reading any key-bearing rows or caches.
    pub fn read_sql(connection: &Connection) -> Result<Self> {
        let (version, expected): (u32, Vec<u8>) = connection
            .query_row(
                "SELECT version,digest FROM pack_identity WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .wrap_err(
                "world is out of date or has no plugin identity table; it must be converted again",
            )?;
        ensure!(
            version == PACK_IDENTITY_VERSION,
            "world plugin identity format {version} is unsupported; it must be converted again"
        );
        let mut statement = connection.prepare("SELECT id,normalized_name,display_name,loaded,flags,header_version_bits,masters_json,content_hash,supplier_priority FROM pack_plugins ORDER BY id")?;
        let mut rows = statement.query([])?;
        let mut entries = Vec::new();
        let mut priorities = std::collections::BTreeMap::new();
        while let Some(row) = rows.next()? {
            let id = PluginId::new(row.get(0)?)?;
            let loaded: i64 = row.get(3)?;
            let flags: u8 = row.get(4)?;
            ensure!(
                matches!(loaded, 0 | 1) && flags <= 7,
                "invalid stored plugin flags"
            );
            let hash: Option<Vec<u8>> = row.get(7)?;
            let hash = hash
                .map(|bytes| {
                    bytes
                        .try_into()
                        .map_err(|_| eyre!("invalid plugin content proof"))
                })
                .transpose()?;
            let priority: Option<u32> = row.get(8)?;
            if let Some(priority) = priority {
                ensure!(
                    priorities.insert(priority, id).is_none(),
                    "duplicate supplier priority"
                );
            }
            entries.push(PluginEntry {
                id,
                normalized_name: row.get(1)?,
                display_name: row.get(2)?,
                loaded: loaded == 1,
                facts: PluginFacts {
                    flags: PluginFlags {
                        master: flags & 1 != 0,
                        light: flags & 2 != 0,
                        medium: flags & 4 != 0,
                    },
                    header_version: row.get::<_, Option<u32>>(5)?.map(f32::from_bits),
                    masters: serde_json::from_str(&row.get::<_, String>(6)?)?,
                    content_hash: hash,
                },
            });
        }
        ensure!(
            priorities.keys().copied().eq(0..priorities.len() as u32),
            "noncontiguous supplier priorities"
        );
        let identity = Self::new(
            PluginTable::from_entries(entries)?,
            priorities.into_values().collect(),
        )?;
        ensure!(
            expected.as_slice() == identity.digest,
            "world plugin identity proof does not match its table; it must be converted again"
        );
        Ok(identity)
    }

    /// Wrap an archived payload with a fixed format/table discriminator and byte proof.
    /// Header inspection must precede rkyv root access, even when bytecheck would succeed.
    pub fn envelope(&self, kind: u32, version: u32, payload: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(IDENTITY_ENVELOPE_SIZE + payload.len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&kind.to_le_bytes());
        bytes.extend_from_slice(&version.to_le_bytes());
        bytes.extend_from_slice(&PACK_IDENTITY_VERSION.to_le_bytes());
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&self.digest);
        bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&Sha256::digest(payload));
        bytes.extend_from_slice(payload);
        bytes
    }

    /// Authenticate the complete envelope before the caller interprets a changed root.
    pub fn payload<'a>(&self, bytes: &'a [u8], kind: u32, version: u32) -> Result<&'a [u8]> {
        ensure!(
            bytes.len() >= IDENTITY_ENVELOPE_SIZE && &bytes[..8] == MAGIC,
            "world cache is out of date; it must be converted again"
        );
        ensure!(
            bytes[8..12] == kind.to_le_bytes()
                && bytes[12..16] == version.to_le_bytes()
                && bytes[16..20] == PACK_IDENTITY_VERSION.to_le_bytes()
                && bytes[20..24] == [0; 4],
            "world cache format is unsupported; it must be converted again"
        );
        ensure!(
            bytes[24..56] == self.digest,
            "world cache belongs to a different plugin table; it must be converted again"
        );
        let length = u64::from_le_bytes(bytes[56..64].try_into()?);
        let payload = &bytes[IDENTITY_ENVELOPE_SIZE..];
        ensure!(
            length == payload.len() as u64 && bytes[64..96] == Sha256::digest(payload)[..],
            "world cache payload proof is invalid"
        );
        Ok(payload)
    }
}

/// Hash unambiguous length-prefixed facts; preserve f32 source bits, including nonfinite HEDR.
fn mapping_digest(table: &PluginTable, order: &[PluginId]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"MudCrab pack identity v1\0");
    hash.update((table.entries().len() as u64).to_le_bytes());
    for entry in table.entries() {
        hash.update(entry.id.get().to_le_bytes());
        for text in [&entry.normalized_name, &entry.display_name] {
            hash.update((text.len() as u64).to_le_bytes());
            hash.update(text.as_bytes());
        }
        hash.update([
            u8::from(entry.loaded),
            u8::from(entry.facts.flags.master),
            u8::from(entry.facts.flags.light),
            u8::from(entry.facts.flags.medium),
        ]);
        hash.update([u8::from(entry.facts.header_version.is_some())]);
        hash.update(
            entry
                .facts
                .header_version
                .map_or(0, f32::to_bits)
                .to_le_bytes(),
        );
        hash.update((entry.facts.masters.len() as u64).to_le_bytes());
        for master in &entry.facts.masters {
            hash.update((master.len() as u64).to_le_bytes());
            hash.update(master.as_bytes());
        }
        hash.update([u8::from(entry.facts.content_hash.is_some())]);
        if let Some(proof) = entry.facts.content_hash {
            hash.update(proof);
        }
    }
    hash.update((order.len() as u64).to_le_bytes());
    for id in order {
        hash.update(id.get().to_le_bytes());
    }
    hash.finalize().into()
}
