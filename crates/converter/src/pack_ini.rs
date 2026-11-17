//! Read-only discovery and staged publication of the player's Skyrim settings.

use crate::{PipelineConfig, PipelineReport, cache::ConversionManifest};
use shared::pack_ini::{
    FILES, IniOrigin, IniProvenance, MAX_INI_BYTES, decode_ini, read_ini_bytes, readable_path,
};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

/// Resolves settings only for callers that explicitly requested discovery or supplied a folder.
pub(crate) fn player_ini_dir(config: &PipelineConfig) -> std::io::Result<Option<PathBuf>> {
    if config.no_player_ini {
        return Ok(None);
    }
    if let Some(folder) = &config.player_ini_dir {
        return Ok(Some(folder.clone()));
    }
    if let Some(error) = &config.player_ini_error {
        return Err(std::io::Error::other(error.clone()));
    }
    Ok(None)
}

/// Resolves a front-end request using the selected edition and redirected Documents folder.
pub(crate) fn discover_player_ini_dir(data: &Path) -> std::io::Result<PathBuf> {
    let edition = edition_folder(data)?;
    let documents = documents_dir()?.ok_or_else(|| {
        std::io::Error::other("Documents discovery is unavailable; supply --player-ini-dir")
    })?;
    Ok(documents.join("My Games").join(edition))
}

/// Selects one positively identified SE store edition and its own settings directory.
fn edition_folder(data: &Path) -> std::io::Result<&'static str> {
    let data = fs::canonicalize(data)?;
    let root = data
        .parent()
        .ok_or_else(|| std::io::Error::other("install has no parent"))?;
    if child(root, "SkyrimVR.exe")?.is_some()
        || child(root, "TESV.exe")?.is_some()
        || !child(root, "SkyrimSE.exe")?.is_some_and(|p| p.is_file())
        || !child(&data, "Skyrim.esm")?.is_some_and(|p| p.is_file())
    {
        return Err(std::io::Error::other(
            "selected install is not unambiguously Skyrim SE/AE",
        ));
    }
    let mut editions = Vec::new();
    for (marker, folder) in [
        ("steam_api64.dll", "Skyrim Special Edition"),
        ("Galaxy64.dll", "Skyrim Special Edition GOG"),
        ("EOSSDK-Win64-Shipping.dll", "Skyrim Special Edition EPIC"),
        ("appxmanifest.xml", "Skyrim Special Edition MS"),
    ] {
        if child(root, marker)?.is_some_and(|p| p.is_file()) {
            editions.push(folder);
        }
    }
    if editions.len() != 1 {
        return Err(std::io::Error::other(
            "selected install's store edition is ambiguous; supply --player-ini-dir",
        ));
    }
    Ok(editions[0])
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
    // SAFETY: the API supplies an allocated NUL-terminated UTF-16 string, released below.
    let result =
        unsafe { SHGetKnownFolderPath(&FOLDERID_Documents, 0, std::ptr::null_mut(), &mut path) };
    if result < 0 {
        if !path.is_null() {
            // SAFETY: this allocation belongs to the COM allocator.
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

/// Other platforms accept an explicitly supplied settings directory.
#[cfg(not(windows))]
fn documents_dir() -> std::io::Result<Option<PathBuf>> {
    Ok(None)
}

/// Finds a case-insensitive child through read-only lookup.
/// Settings-file links are followed for redirected Documents and OneDrive placeholders.
fn child(folder: &Path, name: &str) -> std::io::Result<Option<PathBuf>> {
    match fs::metadata(folder) {
        Ok(metadata) if !metadata.is_dir() => {
            return Err(std::io::Error::other(
                "INI lookup folder is not a directory",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
        _ => {}
    }
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

/// Records optional-settings diagnostics in the advisory channel, preserving asset completeness.
fn notice(report: &mut PipelineReport, message: String) {
    log::warn!("{message}");
    report.notices.push(message);
}

/// Validates settings, reports tolerant decoding, and strips account sections before copying.
fn read_ini(path: &Path, report: &mut PipelineReport) -> std::io::Result<Vec<u8>> {
    let bytes = read_ini_bytes(path)?;
    let decoded = decode_ini(&bytes);
    if decoded.lossy {
        notice(
            report,
            format!(
                "lossy decoding of optional INI {}",
                readable_path(path).display()
            ),
        );
    }
    let mut section = false;
    let mut settings = false;
    let mut private = false;
    let mut removed = false;
    let mut sanitized = String::new();
    for raw in decoded.text.split_inclusive('\n') {
        let line = raw.trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = true;
            private = line[1..line.len() - 1]
                .trim()
                .eq_ignore_ascii_case("Bethesda.net");
        } else if section && !line.starts_with([';', '#']) && line.contains('=') && !private {
            settings = true;
        }
        if private {
            removed = true;
        } else {
            sanitized.push_str(raw);
        }
    }
    if !settings {
        return Err(std::io::Error::other("INI contains no section settings"));
    }
    if !removed {
        return Ok(bytes);
    }
    notice(
        report,
        format!(
            "removed Bethesda.net account section from optional INI {}",
            readable_path(path).display()
        ),
    );
    if sanitized.len() as u64 > MAX_INI_BYTES {
        return Err(std::io::Error::other(
            "filtered INI exceeds the 4 MiB limit",
        ));
    }
    // Account filtering emits bounded UTF-8; unaffected files retain exact bytes.
    Ok(sanitized.into_bytes())
}

/// Confirms the canonical selected Data folder and SE executable before allowing templates.
fn install_root(config: &PipelineConfig) -> Option<PathBuf> {
    let data = fs::canonicalize(&config.data_dir).ok()?;
    let root = data.parent()?;
    let master = child(&data, "Skyrim.esm").ok()??;
    let executable = child(root, "SkyrimSE.exe").ok()??;
    (master.is_file() && executable.is_file()).then(|| root.to_owned())
}

/// Publishes through a fresh temporary file so existing hard links keep their source bytes.
fn write_copy(output: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    fs::create_dir_all(output)?;
    let mut temporary = tempfile::NamedTempFile::new_in(output)?;
    temporary.write_all(bytes)?;
    temporary
        .persist(output.join(name))
        .map_err(|error| error.error)?;
    Ok(())
}

/// Retains portable provenance for unchanged copies and identifies local edits by pack-relative path.
fn retained_provenance(
    name: &str,
    bytes: &[u8],
    previous: Option<&IniProvenance>,
) -> IniProvenance {
    let sha256 = crate::cache::hash_bytes(bytes);
    if let Some(previous) = previous.filter(|entry| entry.sha256 == sha256) {
        let mut proof = previous.clone();
        proof.source = if proof.origin == IniOrigin::PackCopy {
            PathBuf::from(format!("ini/{name}"))
        } else {
            PathBuf::from(name)
        };
        return proof;
    }
    IniProvenance {
        source: PathBuf::from(format!("ini/{name}")),
        sha256,
        origin: IniOrigin::PackCopy,
    }
}

/// Retains existing pack settings and reports each kept file during metadata rebuilds.
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
        match fs::symlink_metadata(&input) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            _ => {}
        }
        let copied = (|| -> std::io::Result<IniProvenance> {
            let bytes = read_ini(&input, report)?;
            write_copy(&staging.join("ini"), name, &bytes)?;
            Ok(retained_provenance(name, &bytes, previous.get(&relative)))
        })();
        match copied {
            Ok(proof) => {
                manifest.ini_files.insert(relative.clone(), proof);
                report.artifacts.push(relative.into());
                notice(
                    report,
                    format!("kept pack INI {name} during metadata rebuild"),
                );
            }
            Err(error) => notice(
                report,
                format!(
                    "skipping optional pack INI {} during metadata rebuild: {error}",
                    readable_path(&input).display()
                ),
            ),
        }
    }
}

/// Resolves selected MO2 settings only for SE instances; errors retain lookup-failure semantics.
fn profile_ini_dir(config: &PipelineConfig) -> Result<Option<PathBuf>, String> {
    let Some(selection) = &config.mo2 else {
        return Ok(None);
    };
    let instance = mo2::Instance::open(&selection.instance_path).map_err(|e| format!("{e:#}"))?;
    if !instance.is_skyrim_se() {
        return Err("selected MO2 game is not Skyrim SE/AE".into());
    }
    instance
        .profile_ini_dir(&selection.profile)
        .map_err(|e| format!("{e:#}"))
}

/// Refreshes unedited copies and preserves local pack edits consistently with metadata rebuilds.
pub(crate) fn capture(
    config: &PipelineConfig,
    staging: &Path,
    report: &mut PipelineReport,
    manifest: &mut ConversionManifest,
) {
    let previous = ConversionManifest::load(&config.output_dir.join("conversion-manifest.json"))
        .unwrap_or_default();
    manifest.ini_files.clear();
    let output = staging.join("ini");
    match output.symlink_metadata() {
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
            notice(
                report,
                format!(
                    "skipping optional INIs: settings output is not a regular directory: {}",
                    readable_path(&output).display()
                ),
            );
            return;
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            notice(
                report,
                format!(
                    "could not inspect optional settings output {}: {error}",
                    readable_path(&output).display()
                ),
            );
            return;
        }
        _ => {}
    }
    let requested = !config.no_player_ini
        && (config.player_ini_dir.is_some()
            || config.player_ini_error.is_some()
            || config.mo2.is_some());
    let profile = if requested {
        profile_ini_dir(config)
    } else {
        Ok(None)
    };
    if let Err(error) = &profile {
        notice(
            report,
            format!(
                "could not resolve MO2 profile INIs: {error}; skipping external optional settings"
            ),
        );
    }
    let player = if requested {
        player_ini_dir(config)
    } else {
        Ok(None)
    };
    if let Ok(Some(folder)) = &player
        && matches!(fs::metadata(folder), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
    {
        notice(
            report,
            format!(
                "player settings folder {} does not exist; genuinely absent INIs may use install templates",
                readable_path(folder).display()
            ),
        );
    }
    let install = install_root(config);
    for (name, template) in FILES.into_iter().zip(["Skyrim_Default.ini", "High.ini"]) {
        let relative = format!("ini/{name}");
        let destination = output.join(name);
        if let Err(error) = fs::remove_file(&destination)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            notice(
                report,
                format!(
                    "could not refresh optional INI {}: {error}",
                    readable_path(&destination).display()
                ),
            );
            continue;
        }
        let old = config.output_dir.join(&relative);
        let old_proof = previous.ini_files.get(&relative);
        let old_bytes = match read_ini_bytes(&old) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                notice(
                    report,
                    format!("skipping unreadable existing pack INI {name}: {error}"),
                );
                continue;
            }
        };
        let edited = old_bytes.as_ref().is_some_and(|bytes| {
            old_proof.is_none_or(|proof| {
                proof.origin == IniOrigin::PackCopy
                    || proof.sha256 != crate::cache::hash_bytes(bytes)
            })
        });
        if edited {
            match read_ini(&old, report).and_then(|bytes| {
                write_copy(&output, name, &bytes)?;
                Ok(retained_provenance(name, &bytes, None))
            }) {
                Ok(proof) => {
                    manifest.ini_files.insert(relative.clone(), proof);
                    report.artifacts.push(relative.into());
                    notice(
                        report,
                        format!("kept edited pack INI {name} during reconversion"),
                    );
                }
                Err(error) => notice(
                    report,
                    format!("skipping invalid edited pack INI {name}: {error}"),
                ),
            }
            continue;
        }
        if !requested {
            if old_bytes.is_some() {
                notice(
                    report,
                    format!(
                        "removed unedited pack INI {name}: external settings capture is disabled"
                    ),
                );
            }
            continue;
        }
        let lookup = (|| -> Result<Option<(PathBuf, IniOrigin)>, String> {
            let profile = profile
                .as_ref()
                .map_err(|error| format!("could not resolve MO2 profile INIs: {error}"))?;
            if let Some(profile) = profile
                && let Some(path) = child(profile, name).map_err(|e| e.to_string())?
            {
                return Ok(Some((path, IniOrigin::PlayerFile)));
            }
            let player = player
                .as_ref()
                .map_err(|e| format!("could not resolve player INIs: {e}"))?;
            if let Some(player) = player
                && let Some(path) = child(player, name).map_err(|e| e.to_string())?
            {
                return Ok(Some((path, IniOrigin::PlayerFile)));
            }
            if let Some(install) = &install
                && let Some(path) = child(install, template).map_err(|e| e.to_string())?
            {
                return Ok(Some((path, IniOrigin::GeneratedFromInstall)));
            }
            Ok(None)
        })();
        let (source, origin) = match lookup {
            Ok(Some(source)) => source,
            Ok(None) => {
                if old_bytes.is_some() {
                    notice(
                        report,
                        format!("removed unedited pack INI {name}: its source is absent"),
                    );
                }
                continue;
            }
            Err(error) => {
                notice(report, format!("skipping optional INI {name}: {error}"));
                continue;
            }
        };
        let bytes = match read_ini(&source, report) {
            Ok(bytes) => bytes,
            Err(error) => {
                notice(
                    report,
                    format!(
                        "skipping unreadable or invalid INI {}: {error}",
                        readable_path(&source).display()
                    ),
                );
                continue;
            }
        };
        match write_copy(&output, name, &bytes) {
            Ok(()) => {
                manifest.ini_files.insert(
                    relative.clone(),
                    IniProvenance {
                        source: PathBuf::from(
                            source.file_name().unwrap_or(std::ffi::OsStr::new(name)),
                        ),
                        sha256: crate::cache::hash_bytes(&bytes),
                        origin,
                    },
                );
                report.artifacts.push(relative.into());
                if old_bytes.as_ref().is_some_and(|old| {
                    crate::cache::hash_bytes(old) != crate::cache::hash_bytes(&bytes)
                }) {
                    notice(
                        report,
                        format!("replaced unedited pack INI {name} from its current source"),
                    );
                }
            }
            Err(error) => notice(
                report,
                format!(
                    "could not copy optional INI {}: {error}",
                    readable_path(&source).display()
                ),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Library defaults keep Documents discovery opt-in even for a confirmed synthetic install.
    #[test]
    fn library_default_ini_discovery_is_disabled() {
        let config = PipelineConfig::new("synthetic/Data", "synthetic/pack");
        assert_eq!(player_ini_dir(&config).unwrap(), None);
    }

    /// An explicitly supplied folder supersedes a previous failed discovery attempt.
    #[test]
    fn explicit_ini_folder_supersedes_failed_discovery() {
        let mut config = PipelineConfig::new("synthetic/Data", "synthetic/pack");
        config.player_ini_error = Some("synthetic failed lookup".into());
        config.player_ini_dir = Some("synthetic/player".into());
        assert_eq!(
            player_ini_dir(&config).unwrap(),
            Some(PathBuf::from("synthetic/player"))
        );
    }

    /// Portable retention strips prior Windows home paths on every host platform.
    #[test]
    fn retained_provenance_removes_windows_home_path() {
        let bytes = b"[General]\nuGridsToLoad=7\n";
        let proof = IniProvenance {
            source: PathBuf::from(
                r"C:\Users\synthetic\Documents\My Games\Skyrim Special Edition\Skyrim.ini",
            ),
            sha256: crate::cache::hash_bytes(bytes),
            origin: IniOrigin::PlayerFile,
        };
        let retained = retained_provenance("Skyrim.ini", bytes, Some(&proof));
        assert_eq!(retained.source, PathBuf::from("Skyrim.ini"));
        assert_eq!(retained.origin, IniOrigin::PlayerFile);
    }

    /// Each selected store resolves its own folder rather than probing another edition's INIs.
    #[test]
    fn edition_folder_requires_one_se_store_signal() {
        for (marker, expected) in [
            ("steam_api64.dll", "Skyrim Special Edition"),
            ("Galaxy64.dll", "Skyrim Special Edition GOG"),
            ("EOSSDK-Win64-Shipping.dll", "Skyrim Special Edition EPIC"),
            ("appxmanifest.xml", "Skyrim Special Edition MS"),
        ] {
            let root = tempfile::tempdir().unwrap();
            fs::create_dir(root.path().join("Data")).unwrap();
            fs::write(root.path().join("Data/Skyrim.esm"), b"synthetic").unwrap();
            fs::write(root.path().join("SkyrimSE.exe"), b"synthetic").unwrap();
            assert!(edition_folder(&root.path().join("Data")).is_err());
            fs::write(root.path().join(marker), b"synthetic").unwrap();
            assert_eq!(edition_folder(&root.path().join("Data")).unwrap(), expected);
            let other = if marker == "Galaxy64.dll" {
                "steam_api64.dll"
            } else {
                "Galaxy64.dll"
            };
            fs::write(root.path().join(other), b"synthetic").unwrap();
            assert!(edition_folder(&root.path().join("Data")).is_err());
            fs::remove_file(root.path().join(other)).unwrap();
            for incompatible in ["SkyrimVR.exe", "TESV.exe"] {
                fs::write(root.path().join(incompatible), b"synthetic").unwrap();
                assert!(edition_folder(&root.path().join("Data")).is_err());
                fs::remove_file(root.path().join(incompatible)).unwrap();
            }
        }
    }

    /// Failed front-end discovery remains a nonfatal lookup failure instead of template absence.
    #[test]
    fn discovery_failure_is_nonfatal_and_never_generates() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("Data")).unwrap();
        fs::write(root.path().join("Data/Skyrim.esm"), b"synthetic").unwrap();
        fs::write(root.path().join("SkyrimSE.exe"), b"synthetic").unwrap();
        fs::write(
            root.path().join("Skyrim_Default.ini"),
            b"[General]\nuGridsToLoad=9\n",
        )
        .unwrap();
        let mut config = PipelineConfig::new(root.path().join("Data"), root.path().join("pack"));
        config.discover_player_inis();
        assert!(
            config
                .player_ini_error
                .as_ref()
                .unwrap()
                .contains("ambiguous")
        );
        let mut report = PipelineReport::default();
        let mut manifest = ConversionManifest::default();
        capture(
            &config,
            &root.path().join("staging"),
            &mut report,
            &mut manifest,
        );
        assert!(manifest.ini_files.is_empty());
        assert!(crate::pipeline::conversion_is_complete(&report));
        assert_eq!(report.notices.len(), 2);
    }

    /// Bounded reads skip oversized optional files and preserve usable neighboring settings.
    #[test]
    fn oversized_ini_capture_is_nonfatal() {
        let root = tempfile::tempdir().unwrap();
        let player = root.path().join("player");
        fs::create_dir(&player).unwrap();
        fs::File::create(player.join("Skyrim.ini"))
            .unwrap()
            .set_len(4 * 1024 * 1024 + 1)
            .unwrap();
        fs::write(
            player.join("SkyrimPrefs.ini"),
            b"[Display]\nfShadowDistance=5432\n",
        )
        .unwrap();
        let mut config = PipelineConfig::new(root.path().join("Data"), root.path().join("pack"));
        config.player_ini_dir = Some(player);
        let mut report = PipelineReport::default();
        let mut manifest = ConversionManifest::default();
        capture(
            &config,
            &root.path().join("staging"),
            &mut report,
            &mut manifest,
        );
        assert!(crate::pipeline::conversion_is_complete(&report));
        assert_eq!(manifest.ini_files.len(), 1);
        assert!(
            report
                .notices
                .iter()
                .any(|message| message.contains("4 MiB"))
        );
    }

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
            source: PathBuf::from("Skyrim.ini"),
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
            PathBuf::from("ini/SkyrimPrefs.ini")
        );
        assert!(report.warnings.is_empty());
    }

    /// Optional retained-file failures preserve the metadata completion decision.
    #[test]
    fn metadata_retain_invalid_ini_is_nonfatal() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let staging = directory.path().join("staging");
        fs::create_dir_all(source.join("ini")).unwrap();
        fs::write(source.join("ini/Skyrim.ini"), b"invalid settings").unwrap();
        fs::write(
            source.join("ini/SkyrimPrefs.ini"),
            b"[Display]\nfShadowDistance=4321\n",
        )
        .unwrap();
        let mut report = PipelineReport::default();
        let mut manifest = ConversionManifest::default();
        retain(&source, &staging, &mut report, &mut manifest);
        assert!(
            crate::pipeline::conversion_is_complete(&report),
            "{report:?}"
        );
        assert!(
            report
                .notices
                .iter()
                .any(|notice| notice.contains("metadata rebuild"))
        );
        assert!(staging.join("ini/SkyrimPrefs.ini").is_file());
    }
}
