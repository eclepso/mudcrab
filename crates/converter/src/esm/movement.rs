//! Rebuild movement scalar projections from authenticated native bytes and physical links.
use color_eyre::{
    Result,
    eyre::{WrapErr, ensure, eyre},
};
use rusqlite::{Connection, params};
use shared::{form_key::FormKey, records::RecordContextReader};

/// Reproject the current pack without changing canonical bytes, identity allocation or schema.
pub fn rebuild_movement_projection(connection: &Connection) -> Result<usize> {
    let version: u32 = connection
        .query_row("SELECT version FROM schema_info", [], |row| row.get(0))
        .wrap_err("world schema is unavailable; convert again")?;
    ensure!(
        version == shared::WORLD_DATABASE_SCHEMA_VERSION,
        "old world schema {version}; convert again"
    );
    let transaction = connection.unchecked_transaction()?;
    let reader = RecordContextReader::new(&transaction)
        .wrap_err("world native context is unavailable; convert again")?;
    let identity = reader.identity();
    transaction.execute_batch("CREATE TABLE IF NOT EXISTS movement_types(id INTEGER PRIMARY KEY,editor_id TEXT NOT NULL,left_walk REAL NOT NULL,left_run REAL NOT NULL,right_walk REAL NOT NULL,right_run REAL NOT NULL,forward_walk REAL NOT NULL,forward_run REAL NOT NULL,back_walk REAL NOT NULL,back_run REAL NOT NULL,load_order INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS race_movement_links(id INTEGER PRIMARY KEY,walk_movt_id INTEGER,run_movt_id INTEGER,load_order INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS movement_game_settings(id INTEGER PRIMARY KEY,editor_id TEXT NOT NULL,value REAL NOT NULL,load_order INTEGER NOT NULL); DELETE FROM movement_types; DELETE FROM race_movement_links; DELETE FROM movement_game_settings;")?;
    let mut statement=transaction.prepare("SELECT form_id,record_type,load_order FROM records WHERE record_type IN ('RACE','MOVT','GMST')")?;
    let mut count = 0usize;
    let mut omitted = 0usize;
    let mut first = None;
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, u32>(2)?,
        ))
    })? {
        let (stored, kind, priority) = row?;
        let key = identity.required_key(stored)?;
        let record = reader.record(key)?;
        let projected = (|| -> Result<bool> {
            match kind.as_str() {
                "RACE" => {
                    let walk = record.first_link(*b"WKMV", 0)?.map(FormKey::to_sql_i64);
                    let run = record.first_link(*b"RNMV", 0)?.map(FormKey::to_sql_i64);
                    transaction.execute(
                        "INSERT INTO race_movement_links VALUES(?1,?2,?3,?4)",
                        params![stored, walk, run, priority],
                    )?;
                }
                "MOVT"
                    if key.local() == 0x3580d
                        && identity
                            .table()
                            .entry(key.plugin())
                            .is_some_and(|entry| entry.normalized_name == "skyrim.esm") =>
                {
                    ensure!(
                        record.text(*b"EDID").as_deref() == Some("NPC_Default_MT"),
                        "MOVT EDID does not name NPC_Default_MT"
                    );
                    let bytes = record
                        .bytes(*b"SPED")
                        .ok_or_else(|| eyre!("MOVT lacks usable SPED"))?;
                    ensure!(matches!(bytes.len(), 40 | 44), "invalid MOVT SPED layout");
                    let mut speeds = [0f32; 8];
                    for (value, word) in speeds.iter_mut().zip(bytes.as_chunks::<4>().0) {
                        *value = f32::from_le_bytes(*word);
                    }
                    ensure!(
                        speeds.iter().all(|value| value.is_finite() && *value > 0.),
                        "invalid MOVT speeds"
                    );
                    transaction.execute("INSERT INTO movement_types VALUES(?1,'NPC_Default_MT',?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![stored,speeds[0],speeds[1],speeds[2],speeds[3],speeds[4],speeds[5],speeds[6],speeds[7],priority])?;
                }
                "GMST" => {
                    let name = record.text(*b"EDID").unwrap_or_default();
                    let Some(name) = ["fMoveCharWalkBase", "fJumpHeightMin"]
                        .into_iter()
                        .find(|known| known.eq_ignore_ascii_case(&name))
                    else {
                        return Ok(false);
                    };
                    let bytes = record
                        .bytes(*b"DATA")
                        .ok_or_else(|| eyre!("GMST lacks usable DATA"))?;
                    ensure!(bytes.len() == 4, "invalid movement GMST DATA layout");
                    let value = f32::from_le_bytes(bytes.try_into()?);
                    ensure!(value.is_finite(), "nonfinite movement GMST value");
                    transaction.execute(
                        "INSERT INTO movement_game_settings VALUES(?1,?2,?3,?4)",
                        params![stored, name, value, priority],
                    )?;
                }
                _ => return Ok(false),
            }
            Ok(true)
        })();
        match projected {
            Ok(true) => count += 1,
            Ok(false) => {}
            Err(error) => {
                omitted += 1;
                first.get_or_insert_with(|| {
                    format!(
                        "{}: {error:#}",
                        identity.table().named(key).expect("authenticated owner")
                    )
                });
            }
        }
    }
    if omitted != 0 {
        log::warn!(
            "movement projection omitted {omitted} unusable records; first {}",
            first.as_deref().unwrap_or("unknown")
        );
    }
    drop(statement);
    drop(reader);
    transaction.commit()?;
    Ok(count)
}
