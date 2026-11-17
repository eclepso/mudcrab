//! Provenance for optional Skyrim settings carried with a converted pack.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Pack settings apply in retail order: the general file, then preferences.
pub const FILES: [&str; 2] = ["Skyrim.ini", "SkyrimPrefs.ini"];

/// The original input behind a verified copy of a pack INI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IniOrigin {
    PlayerFile,
    GeneratedFromInstall,
    PackCopy,
}

/// The original path and SHA-256 of the exact bytes copied into the pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IniProvenance {
    pub source: PathBuf,
    pub sha256: String,
    pub origin: IniOrigin,
}
