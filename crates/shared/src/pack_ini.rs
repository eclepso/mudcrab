//! Provenance for optional Skyrim settings carried with a converted pack.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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

/// Uses readable Windows drive/UNC paths for provenance while preserving special namespaces.
pub fn readable_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        use std::{
            ffi::OsString,
            os::windows::ffi::{OsStrExt, OsStringExt},
            path::{Component, Prefix},
        };
        let (skip, unc) = match path.components().next() {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::VerbatimDisk(_) => (4, false),
                Prefix::VerbatimUNC(_, _) => (8, true),
                Prefix::Verbatim(_) | Prefix::DeviceNS(_) => return path.to_owned(),
                _ => (0, false),
            },
            _ => (0, false),
        };
        if skip != 0
            && path.components().any(|component| match component {
                Component::CurDir | Component::ParentDir => true,
                Component::Normal(name) => {
                    let text = name.to_string_lossy();
                    let stem = text
                        .split('.')
                        .next()
                        .unwrap_or_default()
                        .to_ascii_uppercase();
                    text.ends_with(['.', ' '])
                        || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                        || (stem.len() == 4
                            && (stem.starts_with("COM") || stem.starts_with("LPT"))
                            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
                }
                _ => false,
            })
        {
            return path.to_owned();
        }
        let mut units = if unc {
            vec![b'\\' as u16; 2]
        } else {
            Vec::new()
        };
        units.extend(path.as_os_str().encode_wide().skip(skip).map(|unit| {
            if unit == b'/' as u16 {
                b'\\' as u16
            } else {
                unit
            }
        }));
        PathBuf::from(OsString::from_wide(&units))
    }
    #[cfg(not(windows))]
    {
        path.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ordinary paths retain the platform's own path spelling.
    #[test]
    fn readable_path_preserves_native_paths() {
        assert_eq!(
            readable_path(Path::new("settings/Skyrim.ini")),
            PathBuf::from("settings").join("Skyrim.ini")
        );
    }

    /// Canonicalized drive and network paths use consistent readable Windows spelling.
    #[cfg(windows)]
    #[test]
    fn readable_path_normalizes_windows_drive_and_unc_paths() {
        for (source, expected) in [
            (
                r"\\?\D:\Profiles\Selected\Skyrim.ini",
                r"D:\Profiles\Selected\Skyrim.ini",
            ),
            (
                r"\\?\UNC\server\share\Selected\Skyrim.ini",
                r"\\server\share\Selected\Skyrim.ini",
            ),
            (
                r"D:\Profiles/Selected\Skyrim.ini",
                r"D:\Profiles\Selected\Skyrim.ini",
            ),
        ] {
            assert_eq!(
                readable_path(Path::new(source)).as_os_str(),
                std::ffi::OsStr::new(expected)
            );
        }
    }

    /// Namespace-dependent names retain the prefix required to represent the same path.
    #[cfg(windows)]
    #[test]
    fn readable_path_preserves_namespace_dependent_names() {
        for source in [
            r"\\?\Volume{synthetic}\Skyrim.ini",
            r"\\?\D:\Profiles\trailing.\Skyrim.ini",
            r"\\?\D:\Profiles\NUL.ini",
        ] {
            assert_eq!(
                readable_path(Path::new(source)).as_os_str(),
                std::ffi::OsStr::new(source)
            );
        }
    }
}
