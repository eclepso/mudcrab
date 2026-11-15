//! Records the plugin rows that a runtime projection leaves out (#297).
//!
//! `insert_light` and `insert_emittance_sources` skip a record when its data cannot supply a row.
//! Each skip is counted here under a stable category, with a bounded sample of FormIDs, and
//! stored in the optional tables of `docs/specs/converters/db-schema.md` §18. The stored counts
//! reach `integration-report.json` and the conversion's warnings or notices, including when a
//! metadata rebuild or a cached database republishes the snapshot.

#[cfg(test)]
use rusqlite::OptionalExtension;
use rusqlite::{Connection, Result, params};
use serde::{Deserialize, Serialize};
use shared::{
    form_key::{FormKey, NamedFormKey},
    pack_identity::PackIdentity,
};
use std::collections::BTreeMap;

/// Tables owned by a fresh scalar projection and copied with checked FormKeys into its pack.
pub const PROJECTION_OMISSION_TABLES_SQL: &str = r#"
         CREATE TABLE IF NOT EXISTS projection_omission_stats (
             category TEXT PRIMARY KEY, severity TEXT NOT NULL, count INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS projection_omission_samples (
             category TEXT NOT NULL, form_id INTEGER NOT NULL, reason TEXT NOT NULL,
             PRIMARY KEY (category, form_id)
         ) WITHOUT ROWID;
"#;

/// FormIDs kept per category; the count stays exact.
pub const SAMPLE_LIMIT: usize = 8;

/// Why a projection wrote no row for a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OmissionCategory {
    /// A `LIGH` without `DATA` gets no `lights` row.
    LightMissingData,
    /// A `LIGH` whose `DATA` is too short for the row's required fields gets no `lights` row.
    LightShortData,
    /// An `XEMI` target that is not in the load order, or is deleted.
    EmittanceTargetAbsent,
    /// An `XEMI` target `LIGH` without a `DATA` long enough to hold its colour.
    EmittanceLightUnusableData,
    /// An `XEMI` target `REGN` whose `RDWT` names no weather.
    EmittanceRegionNoWeather,
    /// An `XEMI` target `REGN` whose weather has no Effect Lighting colours.
    EmittanceWeatherNoLighting,
    /// An `XEMI` target that is neither a `LIGH` nor a `REGN`.
    EmittanceTargetWrongType,
}

impl OmissionCategory {
    pub fn name(self) -> &'static str {
        match self {
            Self::LightMissingData => "light_missing_data",
            Self::LightShortData => "light_short_data",
            Self::EmittanceTargetAbsent => "emittance_target_absent",
            Self::EmittanceLightUnusableData => "emittance_light_unusable_data",
            Self::EmittanceRegionNoWeather => "emittance_region_no_weather",
            Self::EmittanceWeatherNoLighting => "emittance_weather_no_lighting",
            Self::EmittanceTargetWrongType => "emittance_target_wrong_type",
        }
    }

    /// A light that loses its `lights` row loses data the runtime would draw. An unresolved
    /// emittance target keeps the model's own emission, the documented fallback.
    pub fn severity(self) -> Severity {
        match self {
            Self::LightMissingData | Self::LightShortData => Severity::DataLoss,
            Self::EmittanceTargetAbsent
            | Self::EmittanceLightUnusableData
            | Self::EmittanceRegionNoWeather
            | Self::EmittanceWeatherNoLighting
            | Self::EmittanceTargetWrongType => Severity::Fallback,
        }
    }
}

/// How an omission counts toward a complete conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// A supported projection lost a row: the conversion warns and is incomplete.
    DataLoss,
    /// The runtime falls back to authored behaviour: the conversion notes it and stays complete.
    Fallback,
}

impl Severity {
    fn name(self) -> &'static str {
        match self {
            Self::DataLoss => "data_loss",
            Self::Fallback => "fallback",
        }
    }

    /// A severity this build does not know is treated as data loss, so it cannot pass silently.
    fn parse(name: &str) -> Self {
        match name {
            "fallback" => Self::Fallback,
            _ => Self::DataLoss,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OmissionSample<I = u32> {
    pub form_id: I,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CategoryOmissions<I = u32> {
    pub severity: Severity,
    pub count: u64,
    /// At most [`SAMPLE_LIMIT`] identities in original full/light owner, priority and local order.
    pub samples: Vec<OmissionSample<I>>,
}

/// Omissions of the last full export, keyed by category name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionOmissions<I = u32> {
    pub categories: BTreeMap<String, CategoryOmissions<I>>,
}

/// Only named identities may leave an authenticated pack in integration JSON.
pub type NativeProjectionOmissions = ProjectionOmissions<NamedFormKey>;

// The private CPU collector deliberately has no root JSON codec.
impl Serialize for ProjectionOmissions<NamedFormKey> {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        self.categories.serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for ProjectionOmissions<NamedFormKey> {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Ok(Self {
            categories: BTreeMap::deserialize(deserializer)?,
        })
    }
}

impl<I> Default for ProjectionOmissions<I> {
    fn default() -> Self {
        Self {
            categories: BTreeMap::new(),
        }
    }
}

impl<I> ProjectionOmissions<I> {
    pub fn is_empty(&self) -> bool {
        self.categories.is_empty()
    }
}

/// Summaries distinguish private CPU indices from portable source identities explicitly.
pub trait OmissionIdentity {
    fn diagnostic_identity(&self) -> String;
}
impl OmissionIdentity for u32 {
    fn diagnostic_identity(&self) -> String {
        format!("{self:08X}")
    }
}
impl OmissionIdentity for NamedFormKey {
    fn diagnostic_identity(&self) -> String {
        self.to_string()
    }
}

impl ProjectionOmissions<NamedFormKey> {
    /// One bounded portable line per category after source identities have been restored.
    pub fn print_summary(&self) {
        for (category, omissions) in &self.categories {
            eprintln!("warning: {}", summary_line(category, omissions));
        }
    }
}

/// The u32 collector is an internal scalar intermediate, never an on-disk world reader.
impl ProjectionOmissions<u32> {
    /// The private stage may report counts; its temporary indices never become source examples.
    pub fn print_summary(&self) {
        for (category, omissions) in &self.categories {
            eprintln!("warning: {}", category_count_line(category, omissions));
        }
    }

    pub fn record(&mut self, category: OmissionCategory, form_id: u32, reason: impl Into<String>) {
        let entry = self
            .categories
            .entry(category.name().to_owned())
            .or_insert_with(|| CategoryOmissions {
                severity: category.severity(),
                count: 0,
                samples: Vec::new(),
            });
        entry.count += 1;
        // Keep the lowest FormIDs whatever order the records arrive in.
        let at = entry
            .samples
            .partition_point(|sample| sample.form_id < form_id);
        if entry
            .samples
            .get(at)
            .is_some_and(|sample| sample.form_id == form_id)
        {
            return;
        }
        if at < SAMPLE_LIMIT {
            entry.samples.insert(
                at,
                OmissionSample {
                    form_id,
                    reason: reason.into(),
                },
            );
            entry.samples.truncate(SAMPLE_LIMIT);
        }
    }

    /// Replaces the stored omissions with these.
    pub(crate) fn write(&self, conn: &Connection) -> Result<()> {
        require_private_projection(conn)?;
        clear(conn)?;
        for (category, omissions) in &self.categories {
            conn.execute(
                "INSERT INTO projection_omission_stats(category, severity, count) VALUES (?1, ?2, ?3)",
                params![category, omissions.severity.name(), omissions.count as i64],
            )?;
            for sample in &omissions.samples {
                conn.execute(
                    "INSERT INTO projection_omission_samples(category, form_id, reason) VALUES (?1, ?2, ?3)",
                    params![category, sample.form_id, sample.reason],
                )?;
            }
        }
        Ok(())
    }

    /// Restore only a private CPU intermediate in scalar-projection tests.
    #[cfg(test)]
    pub(crate) fn read(conn: &Connection) -> Result<Self> {
        require_private_projection(conn)?;
        let present: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='projection_omission_stats'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let mut omissions = Self::default();
        if present.is_none() {
            return Ok(omissions);
        }
        let mut statement =
            conn.prepare("SELECT category, severity, count FROM projection_omission_stats")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)? as u64,
            ))
        })?;
        for row in rows {
            let (category, severity, count) = row?;
            omissions.categories.insert(
                category,
                CategoryOmissions {
                    severity: Severity::parse(&severity),
                    count,
                    samples: Vec::new(),
                },
            );
        }
        let mut statement = conn.prepare(
            "SELECT category, form_id, reason FROM projection_omission_samples ORDER BY category, form_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get(1)?, row.get(2)?))
        })?;
        for row in rows {
            let (category, form_id, reason) = row?;
            if let Some(entry) = omissions.categories.get_mut(&category) {
                entry.samples.push(OmissionSample { form_id, reason });
            }
        }
        Ok(omissions)
    }
}

/// `"light_short_data: 3 records (data_loss), e.g. LIGH 00000801: DATA is 12 bytes, ..."`.
pub fn summary_line<I: OmissionIdentity>(
    category: &str,
    omissions: &CategoryOmissions<I>,
) -> String {
    let mut line = category_count_line(category, omissions);
    if let Some(sample) = omissions.samples.first() {
        line.push_str(&format!(
            ", e.g. {}: {}",
            sample.form_id.diagnostic_identity(),
            sample.reason
        ));
    }
    line
}

fn category_count_line<I>(category: &str, omissions: &CategoryOmissions<I>) -> String {
    format!(
        "{category}: {} record{} ({})",
        omissions.count,
        if omissions.count == 1 { "" } else { "s" },
        omissions.severity.name()
    )
}

/// Authenticate format and scope before interpreting any stored sample or category count.
impl ProjectionOmissions<NamedFormKey> {
    pub fn read_with_identity(
        conn: &Connection,
        identity: &PackIdentity,
    ) -> color_eyre::Result<Self> {
        use color_eyre::eyre::{WrapErr, ensure};
        let version: u32 = conn
            .query_row("SELECT version FROM schema_info", [], |row| row.get(0))
            .wrap_err("world schema is unavailable; convert again")?;
        ensure!(
            shared::supports_runtime_world_database_schema(version),
            "world schema {version} is unsupported; convert again"
        );
        let context = shared::records::RecordContextReader::new(conn)
            .wrap_err("world native context is unavailable; convert again")?;
        ensure!(
            context.identity().digest() == identity.digest(),
            "projection omissions belong to another world allocation; convert again"
        );
        let (stats,samples):(bool,bool)=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='projection_omission_stats'),EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='projection_omission_samples')",[],|row|Ok((row.get(0)?,row.get(1)?)))?;
        ensure!(
            stats == samples,
            "projection omission tables are incomplete; convert again"
        );
        let mut result = Self::default();
        if !stats {
            return Ok(result);
        }
        let mut statement = conn.prepare(
            "SELECT category,severity,count FROM projection_omission_stats ORDER BY category",
        )?;
        for row in statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })? {
            let (category, severity, count) = row?;
            let count = u64::try_from(count)
                .wrap_err("negative projection omission count; convert again")?;
            result.categories.insert(
                category,
                CategoryOmissions {
                    severity: Severity::parse(&severity),
                    count,
                    samples: Vec::new(),
                },
            );
        }
        let mut retained: BTreeMap<String, Vec<(FormKey, String)>> = BTreeMap::new();
        let mut statement =
            conn.prepare("SELECT category,form_id,reason FROM projection_omission_samples")?;
        for row in statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })? {
            let (category, stored, reason) = row?;
            let key = identity
                .required_key(stored)
                .wrap_err("invalid projection omission identity; convert again")?;
            ensure!(
                result.categories.contains_key(&category),
                "projection omission sample has no category count; convert again"
            );
            let samples = retained.entry(category).or_default();
            let order = shared::cache_identity::owner_order(identity, key);
            let at = samples.partition_point(|(candidate, _)| {
                shared::cache_identity::owner_order(identity, *candidate) < order
            });
            if at < SAMPLE_LIMIT {
                samples.insert(at, (key, reason));
                samples.truncate(SAMPLE_LIMIT);
            }
        }
        for (category, samples) in retained {
            let entry = result
                .categories
                .get_mut(&category)
                .expect("checked sample category");
            entry.samples = samples
                .into_iter()
                .map(|(key, reason)| {
                    Ok(OmissionSample {
                        form_id: identity.table().named(key)?,
                        reason,
                    })
                })
                .collect::<color_eyre::Result<_>>()?;
        }
        Ok(result)
    }
}

/// A scalar index is meaningful only inside the private in-memory projection stage.
fn require_private_projection(conn: &Connection) -> Result<()> {
    let file: String = conn.query_row(
        "SELECT file FROM pragma_database_list WHERE name='main'",
        [],
        |row| row.get(0),
    )?;
    if !file.is_empty() {
        return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "CPU omission indices cannot certify an on-disk world; convert again",
            ),
        )));
    }
    Ok(())
}

/// Deletes every stored omission. Called at the start of a full export.
pub fn clear(conn: &Connection) -> Result<()> {
    for table in ["projection_omission_stats", "projection_omission_samples"] {
        conn.execute(&format!("DELETE FROM {table}"), [])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_an_exact_count_and_the_lowest_form_ids() {
        let mut omissions = ProjectionOmissions::default();
        let total = SAMPLE_LIMIT as u32 + 3;
        for form_id in (0..total).rev() {
            omissions.record(OmissionCategory::LightShortData, 0x100 + form_id, "short");
        }
        let entry = &omissions.categories["light_short_data"];
        assert_eq!(entry.count, u64::from(total));
        assert_eq!(entry.severity, Severity::DataLoss);
        let ids: Vec<u32> = entry.samples.iter().map(|sample| sample.form_id).collect();
        assert_eq!(
            ids,
            (0x100..0x100 + SAMPLE_LIMIT as u32).collect::<Vec<_>>()
        );
    }

    #[test]
    fn round_trips_through_the_tables_and_reads_empty_without_them() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(ProjectionOmissions::read(&conn).unwrap().is_empty());
        conn.execute_batch(PROJECTION_OMISSION_TABLES_SQL).unwrap();
        let mut omissions = ProjectionOmissions::default();
        omissions.record(OmissionCategory::LightMissingData, 0x900, "no DATA");
        omissions.record(
            OmissionCategory::EmittanceTargetWrongType,
            0x901,
            "is a STAT",
        );
        omissions.write(&conn).unwrap();
        assert_eq!(ProjectionOmissions::read(&conn).unwrap(), omissions);
        ProjectionOmissions::default().write(&conn).unwrap();
        assert!(ProjectionOmissions::read(&conn).unwrap().is_empty());
    }

    #[test]
    fn an_unknown_stored_severity_reads_as_data_loss() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(PROJECTION_OMISSION_TABLES_SQL).unwrap();
        conn.execute(
            "INSERT INTO projection_omission_stats VALUES ('future_category', 'advisory', 2)",
            [],
        )
        .unwrap();
        let read = ProjectionOmissions::read(&conn).unwrap();
        assert_eq!(
            read.categories["future_category"].severity,
            Severity::DataLoss
        );
    }
    fn native_identity(proof: u8) -> PackIdentity {
        use shared::plugin_table::{PluginFacts, PluginFlags, PluginTable};
        let names: Vec<_> = std::iter::once("ZetaFirst.esm".to_owned())
            .chain(std::iter::once("AlphaLight.esl".to_owned()))
            .chain((0..256).rev().map(|index| format!("Owner{index:03}.esm")))
            .collect();
        let table = PluginTable::fresh(
            names
                .iter()
                .map(|name| {
                    (
                        name.clone(),
                        PluginFacts {
                            flags: PluginFlags {
                                light: name.ends_with(".esl"),
                                ..Default::default()
                            },
                            content_hash: Some([proof; 32]),
                            ..Default::default()
                        },
                    )
                })
                .collect(),
        )
        .unwrap();
        let order = names.iter().map(|name| table.id(name).unwrap()).collect();
        PackIdentity::new(table, order).unwrap()
    }

    fn native_fixture() -> (Connection, PackIdentity) {
        let identity = native_identity(0x72);
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE schema_info(version INTEGER); CREATE TABLE records(form_id INTEGER PRIMARY KEY,record_type TEXT,data BLOB)").unwrap();
        connection
            .execute(
                "INSERT INTO schema_info VALUES(?1)",
                [shared::WORLD_DATABASE_SCHEMA_VERSION],
            )
            .unwrap();
        let transaction = connection.transaction().unwrap();
        shared::records::create_record_context(&transaction, &identity).unwrap();
        transaction.commit().unwrap();
        connection
            .execute_batch(PROJECTION_OMISSION_TABLES_SQL)
            .unwrap();
        (connection, identity)
    }

    #[test]
    fn native_samples_keep_high_owners_and_original_full_light_order_as_names() {
        let (connection, identity) = native_fixture();
        connection
            .execute(
                "INSERT INTO projection_omission_stats VALUES('light_short_data','data_loss',13)",
                [],
            )
            .unwrap();
        let key =
            |owner: &str, local| FormKey::new(identity.table().id(owner).unwrap(), local).unwrap();
        let first = key("ZetaFirst.esm", 2);
        assert!(first.to_packed() > u64::from(u32::MAX));
        for value in std::iter::once(key("AlphaLight.esl", 1))
            .chain(std::iter::once(key("Owner000.esm", 1)))
            .chain((246..256).map(|index| key(&format!("Owner{index:03}.esm"), 1)))
            .chain(std::iter::once(first))
        {
            connection.execute("INSERT INTO projection_omission_samples VALUES('light_short_data',?1,'short DATA')",[value.to_sql_i64()]).unwrap();
        }
        connection.execute("INSERT INTO projection_omission_stats VALUES('emittance_target_absent','fallback',3)",[]).unwrap();
        for value in [key("AlphaLight.esl", 1), key("Owner000.esm", 1), first] {
            connection.execute("INSERT INTO projection_omission_samples VALUES('emittance_target_absent',?1,'missing target')",[value.to_sql_i64()]).unwrap();
        }
        let read = NativeProjectionOmissions::read_with_identity(&connection, &identity).unwrap();
        let short = &read.categories["light_short_data"];
        assert_eq!(short.count, 13);
        assert_eq!(short.samples.len(), SAMPLE_LIMIT);
        let names: Vec<_> = short
            .samples
            .iter()
            .map(|sample| sample.form_id.to_string())
            .collect();
        assert_eq!(names[0], "000002:ZetaFirst.esm");
        assert_eq!(names[1], "000001:Owner255.esm");
        assert_eq!(names[7], "000001:Owner249.esm");
        let fallback = &read.categories["emittance_target_absent"];
        assert_eq!(
            fallback
                .samples
                .iter()
                .map(|sample| sample.form_id.to_string())
                .collect::<Vec<_>>(),
            [
                "000002:ZetaFirst.esm",
                "000001:Owner000.esm",
                "000001:AlphaLight.esl"
            ]
        );
        assert!(summary_line("light_short_data", short).contains("000002:ZetaFirst.esm"));
        let json = serde_json::to_value(&read).unwrap();
        assert_eq!(
            json["light_short_data"]["samples"][0]["form_id"],
            "000002:ZetaFirst.esm"
        );
        assert!(
            json["light_short_data"]["samples"]
                .as_array()
                .unwrap()
                .iter()
                .all(|sample| sample["form_id"].is_string())
        );
    }

    #[test]
    fn native_omissions_refuse_old_world_and_foreign_scope_before_sample_sql() {
        let (_, identity) = native_fixture();
        let old = Connection::open_in_memory().unwrap();
        old.execute_batch(
            "CREATE TABLE schema_info(version INTEGER); INSERT INTO schema_info VALUES(9)",
        )
        .unwrap();
        let error = NativeProjectionOmissions::read_with_identity(&old, &identity)
            .unwrap_err()
            .to_string();
        assert!(error.contains("convert again"), "{error}");
        let (connection, _) = native_fixture();
        let error =
            NativeProjectionOmissions::read_with_identity(&connection, &native_identity(0x73))
                .unwrap_err()
                .to_string();
        assert!(error.contains("another world allocation"), "{error}");
    }

    #[test]
    fn native_omissions_refuse_unknown_owner_negative_key_and_negative_count() {
        for invalid in [
            0,
            -1,
            FormKey::new(shared::form_key::PluginId::new(9999).unwrap(), 1)
                .unwrap()
                .to_sql_i64(),
        ] {
            let (connection, identity) = native_fixture();
            connection.execute("INSERT INTO projection_omission_stats VALUES('light_short_data','data_loss',1)",[]).unwrap();
            connection.execute("INSERT INTO projection_omission_samples VALUES('light_short_data',?1,'invalid fixture')",[invalid]).unwrap();
            let error = NativeProjectionOmissions::read_with_identity(&connection, &identity)
                .unwrap_err()
                .to_string();
            assert!(error.contains("convert again"), "{invalid}: {error}");
        }
        let (connection, identity) = native_fixture();
        connection
            .execute(
                "INSERT INTO projection_omission_stats VALUES('light_short_data','data_loss',-1)",
                [],
            )
            .unwrap();
        assert!(
            NativeProjectionOmissions::read_with_identity(&connection, &identity)
                .unwrap_err()
                .to_string()
                .contains("negative projection omission count")
        );
    }

    #[test]
    fn cpu_index_writer_cannot_replace_an_on_disk_omission_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let connection = Connection::open(directory.path().join("world.db")).unwrap();
        connection
            .execute_batch(PROJECTION_OMISSION_TABLES_SQL)
            .unwrap();
        connection
            .execute(
                "INSERT INTO projection_omission_stats VALUES('sentinel','data_loss',9)",
                [],
            )
            .unwrap();
        let mut cpu = ProjectionOmissions::default();
        cpu.record(OmissionCategory::LightShortData, 7, "private index");
        assert!(
            cpu.write(&connection)
                .unwrap_err()
                .to_string()
                .contains("convert again")
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count FROM projection_omission_stats WHERE category='sentinel'",
                    [],
                    |row| row.get::<_, u64>(0)
                )
                .unwrap(),
            9
        );
    }
}
