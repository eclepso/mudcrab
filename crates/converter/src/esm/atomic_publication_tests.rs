//! A real production conversion failure must retain the entire previous database snapshot.
use super::{EsmParser, PublicationProvenanceFault};
use crate::config::RecordReader;
use rusqlite::{Connection, types::ValueRef};
use std::{fs, path::Path};

fn sub(tag: &[u8; 4], data: &[u8]) -> Vec<u8> {
    [
        tag.as_slice(),
        &u16::try_from(data.len()).unwrap().to_le_bytes(),
        data,
    ]
    .concat()
}

fn record(tag: &[u8; 4], word: u32, data: &[u8]) -> Vec<u8> {
    [
        tag.as_slice(),
        &u32::try_from(data.len()).unwrap().to_le_bytes(),
        &[0; 4],
        &word.to_le_bytes(),
        &[0; 8],
        data,
    ]
    .concat()
}

fn write_source(path: &Path, editor: &str) {
    let header = sub(
        b"HEDR",
        &[1.71f32.to_le_bytes(), [0; 4], 0x800u32.to_le_bytes()].concat(),
    );
    fs::write(
        path,
        [
            record(b"TES4", 0, &header),
            record(
                b"STAT",
                0x800,
                &sub(b"EDID", format!("{editor}\0").as_bytes()),
            ),
        ]
        .concat(),
    )
    .unwrap();
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Cell {
    Null,
    Integer(i64),
    Real(u64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}

#[derive(Debug, PartialEq, Eq)]
struct Table {
    name: String,
    definition: Option<String>,
    rows: Vec<Vec<Cell>>,
}

/// Compare every stored value, including context/digest/provenance and caller-owned tables.
/// Real values compare by bits; row order is independent of SQLite's current query plan.
fn snapshot(connection: &Connection) -> Vec<Table> {
    let definitions:Vec<(String,Option<String>)>=connection.prepare("SELECT name,sql FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").unwrap()
        .query_map([],|row|Ok((row.get(0)?,row.get(1)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap();
    definitions
        .into_iter()
        .map(|(name, definition)| {
            let sql = format!("SELECT * FROM \"{}\"", name.replace('"', "\"\""));
            let mut statement = connection.prepare(&sql).unwrap();
            let columns = statement.column_count();
            let mut rows: Vec<Vec<Cell>> = statement
                .query_map([], |row| {
                    (0..columns)
                        .map(|column| {
                            Ok(match row.get_ref(column)? {
                                ValueRef::Null => Cell::Null,
                                ValueRef::Integer(value) => Cell::Integer(value),
                                ValueRef::Real(value) => Cell::Real(value.to_bits()),
                                ValueRef::Text(value) => Cell::Text(value.to_vec()),
                                ValueRef::Blob(value) => Cell::Blob(value.to_vec()),
                            })
                        })
                        .collect::<rusqlite::Result<_>>()
                })
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            rows.sort();
            Table {
                name,
                definition,
                rows,
            }
        })
        .collect()
}

/// Baseline worlds have no pack_identity; candidate worlds additionally pin its exact digest.
fn identity_digest(connection: &Connection) -> Option<Vec<u8>> {
    let present:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='pack_identity')",[],|row|row.get(0)).unwrap();
    present.then(|| {
        connection
            .query_row(
                "SELECT digest FROM pack_identity WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .unwrap()
    })
}

#[test]
fn canonical_and_provenance_failure_preserves_complete_prior_world() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("Atomic.esm");
    let database = root.path().join("skyrim_world.db");
    write_source(&source, "PriorWorld");
    EsmParser::convert_plugins_with_reader(
        std::slice::from_ref(&source),
        &database,
        RecordReader::Inhouse,
        root.path(),
    )
    .unwrap();
    let connection = Connection::open(&database).unwrap();
    connection.execute_batch("CREATE TABLE caller_owned(value INTEGER NOT NULL,proof BLOB NOT NULL); INSERT INTO caller_owned VALUES(77,X'11BB3377');").unwrap();
    let before = snapshot(&connection);
    let digest = identity_digest(&connection);
    drop(connection);
    write_source(&source, "ChangedCandidateWorld");
    let fault = PublicationProvenanceFault::arm();
    let error = EsmParser::convert_plugins_with_reader(
        std::slice::from_ref(&source),
        &database,
        RecordReader::Inhouse,
        root.path(),
    )
    .unwrap_err();
    assert!(
        fault.triggered(),
        "the injected boundary must be reached after canonical writes: {error:#}"
    );
    assert!(
        format!("{error:#}").contains("injected canonical/provenance boundary failure"),
        "{error:#}"
    );
    let connection = Connection::open(&database).unwrap();
    assert_eq!(
        snapshot(&connection),
        before,
        "canonical, projections, source history and caller-owned values must all roll back together"
    );
    assert_eq!(
        identity_digest(&connection),
        digest,
        "the prior plugin-table digest must remain unchanged"
    );
    let caller: (i64, Vec<u8>) = connection
        .query_row("SELECT value,proof FROM caller_owned", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(caller, (77, vec![0x11, 0xbb, 0x33, 0x77]));
}
