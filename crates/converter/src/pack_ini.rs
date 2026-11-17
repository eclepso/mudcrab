//! Read-only discovery and staged publication of the player's Skyrim settings.

use crate::{PipelineConfig, PipelineReport, cache::ConversionManifest};
use shared::pack_ini::{FILES, IniOrigin, IniProvenance};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

/// Resolves a supplied settings folder or Windows' current Documents folder.
pub(crate) fn player_ini_dir(config: &PipelineConfig) -> Option<PathBuf> {
    config
        .player_ini_dir
        .clone()
        .or_else(|| match documents_dir() {
            Ok(path) => path.map(|path| path.join("My Games/Skyrim Special Edition")),
            Err(error) => {
                log::warn!("could not locate Windows Documents for Skyrim settings: {error}");
                None
            }
        })
}

/// Uses the Windows known-folder API, including redirected and OneDrive Documents.
#[cfg(windows)]
fn documents_dir() -> std::io::Result<Option<PathBuf>> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath},
    };
    let mut path = std::ptr::null_mut();
    // SAFETY: the API writes an allocated, NUL-terminated UTF-16 string to `path`.
    // Every non-null allocation is released with the matching COM allocator.
    let result =
        unsafe { SHGetKnownFolderPath(&FOLDERID_Documents, 0, std::ptr::null_mut(), &mut path) };
    if result < 0 {
        if !path.is_null() {
            // SAFETY: the API returned this allocation for release by its COM allocator.
            unsafe { CoTaskMemFree(path.cast()) };
        }
        return Err(std::io::Error::other(format!(
            "SHGetKnownFolderPath failed: {result:#x}"
        )));
    }
    if path.is_null() {
        return Ok(None);
    }
    // SAFETY: success supplies a NUL-terminated string owned until CoTaskMemFree.
    let folder = unsafe {
        let mut length = 0;
        while *path.add(length) != 0 {
            length += 1;
        }
        let folder = std::ffi::OsString::from_wide(std::slice::from_raw_parts(path, length));
        CoTaskMemFree(path.cast());
        PathBuf::from(folder)
    };
    Ok(Some(folder))
}

/// Other platforms retain their existing launch behavior without guessing Proton paths.
#[cfg(not(windows))]
fn documents_dir() -> std::io::Result<Option<PathBuf>> {
    Ok(None)
}

/// Finds a case-insensitive child without changing any source directory.
fn child(folder: &Path, name: &str) -> std::io::Result<Option<PathBuf>> {
    match fs::symlink_metadata(folder.join(name)) {
        Ok(_) => return Ok(Some(folder.join(name))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if entry
            .file_name()
            .to_string_lossy()
            .eq_ignore_ascii_case(name)
        {
            return Ok(Some(entry.path()));
        }
    }
    Ok(None)
}

/// Records an optional-settings problem in both the conversion report and terminal log.
fn warn(report: &mut PipelineReport, message: String) {
    log::warn!("{message}");
    report.warnings.push(message);
}

/// Reads a file completely before publishing it, retaining bytes for provenance hashing.
fn read_ini(path: &Path) -> std::io::Result<Vec<u8>> {
    let bytes = fs::read(path)?;
    let text = std::str::from_utf8(&bytes).map_err(std::io::Error::other)?;
    let mut section = false;
    let mut settings = false;
    for line in text.trim_start_matches('\u{feff}').lines().map(str::trim) {
        if line.starts_with('[') && line.ends_with(']') {
            section = true;
        } else if section && !line.starts_with([';', '#']) && line.contains('=') {
            settings = true;
        }
    }
    if !settings {
        return Err(std::io::Error::other("INI contains no section settings"));
    }
    Ok(bytes)
}

/// Validates the selected Data folder and executable before allowing install templates.
fn install_root(config: &PipelineConfig) -> Option<&Path> {
    let root = config.data_dir.parent()?;
    let master = child(&config.data_dir, "Skyrim.esm").ok()??;
    let executable = child(root, "SkyrimSE.exe").ok()??;
    (master.is_file() && executable.is_file()).then_some(root)
}

/// Retains existing settings when rebuilding a pack's world metadata, including player edits.
pub(crate) fn retain(
    source: &Path,
    staging: &Path,
    report: &mut PipelineReport,
    manifest: &mut ConversionManifest,
) {
    let previous = std::mem::take(&mut manifest.ini_files);
    for name in FILES {
        let relative = format!("ini/{name}");
        let input = source.join(&relative);
        if !input.exists() {
            continue;
        }
        let copied = (|| -> std::io::Result<IniProvenance> {
            let bytes = read_ini(&input)?;
            let sha256 = crate::cache::hash_bytes(&bytes);
            let provenance = previous
                .get(&relative)
                .filter(|entry| entry.sha256 == sha256)
                .cloned()
                .unwrap_or_else(|| IniProvenance {
                    source: input.clone(),
                    sha256,
                    origin: IniOrigin::PackCopy,
                });
            let destination = staging.join(&relative);
            fs::create_dir_all(destination.parent().expect("pack INIs have a parent"))?;
            let mut temporary = tempfile::NamedTempFile::new_in(destination.parent().unwrap())?;
            temporary.write_all(&bytes)?;
            temporary
                .persist(&destination)
                .map_err(|error| error.error)?;
            Ok(provenance)
        })();
        match copied {
            Ok(provenance) => {
                manifest.ini_files.insert(relative.clone(), provenance);
                report.artifacts.push(relative.into());
            }
            Err(error) => warn(
                report,
                format!(
                    "skipping optional pack INI {} during metadata rebuild: {error}",
                    input.display()
                ),
            ),
        }
    }
}

/// Captures each INI independently and refreshes resumed copies before the manifest is saved.
pub(crate) fn capture(
    config: &PipelineConfig,
    staging: &Path,
    report: &mut PipelineReport,
    manifest: &mut ConversionManifest,
) {
    manifest.ini_files.clear();
    let player = player_ini_dir(config);
    let player = match config
        .mo2
        .as_ref()
        .map(|selection| {
            mo2::Instance::open(&selection.instance_path)?.profile_ini_dir(&selection.profile)
        })
        .transpose()
    {
        Ok(profile) => profile.flatten().or(player),
        Err(error) => {
            warn(
                report,
                format!("could not resolve MO2 profile INIs: {error:#}; skipping player settings"),
            );
            None
        }
    };
    let install = install_root(config);
    let output = staging.join("ini");
    match output.symlink_metadata() {
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
            warn(
                report,
                format!(
                    "skipping optional INIs: settings output is not a regular directory: {}",
                    output.display()
                ),
            );
            return;
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            warn(
                report,
                format!(
                    "could not inspect optional settings output {}: {error}",
                    output.display()
                ),
            );
            return;
        }
        _ => {}
    }
    for (name, template) in FILES.into_iter().zip(["Skyrim_Default.ini", "High.ini"]) {
        let destination = output.join(name);
        // Removing a resumed link preserves its source, including links shared with an old pack.
        if let Err(error) = fs::remove_file(&destination)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            warn(
                report,
                format!(
                    "could not refresh optional INI {}: {error}",
                    destination.display()
                ),
            );
            continue;
        }
        let source = match player
            .as_ref()
            .map(|folder| child(folder, name))
            .transpose()
        {
            Ok(Some(Some(path))) => Some((path, IniOrigin::PlayerFile)),
            Ok(_) => match install.map(|folder| child(folder, template)).transpose() {
                Ok(Some(Some(path))) => Some((path, IniOrigin::GeneratedFromInstall)),
                Ok(_) => None,
                Err(error) => {
                    warn(
                        report,
                        format!("could not locate install INI template {template}: {error}"),
                    );
                    None
                }
            },
            Err(error) => {
                warn(
                    report,
                    format!("could not locate player INI {name}: {error}"),
                );
                None
            }
        };
        let Some((source, origin)) = source else {
            continue;
        };
        let bytes = match read_ini(&source) {
            Ok(bytes) => bytes,
            Err(error) => {
                warn(
                    report,
                    format!(
                        "skipping unreadable or invalid INI {}: {error}",
                        source.display()
                    ),
                );
                continue;
            }
        };
        let copied = (|| -> std::io::Result<()> {
            fs::create_dir_all(&output)?;
            let mut temporary = tempfile::NamedTempFile::new_in(&output)?;
            temporary.write_all(&bytes)?;
            temporary
                .persist(&destination)
                .map_err(|error| error.error)?;
            Ok(())
        })();
        match copied {
            Ok(()) => {
                let relative = format!("ini/{name}");
                manifest.ini_files.insert(
                    relative.clone(),
                    IniProvenance {
                        source,
                        sha256: crate::cache::hash_bytes(&bytes),
                        origin,
                    },
                );
                report.artifacts.push(relative.into());
            }
            Err(error) => warn(
                report,
                format!("could not copy optional INI {}: {error}", source.display()),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Metadata rebuilds preserve original provenance and label player edits as pack copies.
    #[test]
    fn metadata_retain_preserves_original_and_edited_pack_inis() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let staging = directory.path().join("staging");
        fs::create_dir_all(source.join("ini")).unwrap();
        let bytes = b"[General]\nuGridsToLoad=7\n";
        fs::write(source.join("ini/Skyrim.ini"), bytes).unwrap();
        fs::write(
            source.join("ini/SkyrimPrefs.ini"),
            b"[Display]\nfShadowDistance=7777\n",
        )
        .unwrap();
        let proof = IniProvenance {
            source: PathBuf::from("player/Skyrim.ini"),
            sha256: crate::cache::hash_bytes(bytes),
            origin: IniOrigin::PlayerFile,
        };
        let mut manifest = ConversionManifest::default();
        manifest
            .ini_files
            .insert("ini/Skyrim.ini".into(), proof.clone());
        manifest
            .ini_files
            .insert("ini/SkyrimPrefs.ini".into(), proof.clone());
        let mut report = PipelineReport::default();
        retain(&source, &staging, &mut report, &mut manifest);
        assert_eq!(fs::read(staging.join("ini/Skyrim.ini")).unwrap(), bytes);
        assert_eq!(manifest.ini_files["ini/Skyrim.ini"], proof);
        assert_eq!(
            fs::read(staging.join("ini/SkyrimPrefs.ini")).unwrap(),
            b"[Display]\nfShadowDistance=7777\n"
        );
        assert_eq!(manifest.ini_files.len(), 2);
        assert_eq!(
            manifest.ini_files["ini/SkyrimPrefs.ini"].origin,
            IniOrigin::PackCopy
        );
        assert_eq!(
            manifest.ini_files["ini/SkyrimPrefs.ini"].source,
            source.join("ini/SkyrimPrefs.ini")
        );
        assert!(report.warnings.is_empty());
    }
}
