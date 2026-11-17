//! Pack INI selection and startup provenance, shared with the explicit INI reader.

use crate::{config::EngineConfig, skyrim_ini::SkyrimIni};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use shared::pack_ini::{FILES, IniOrigin, IniProvenance, read_ini_bytes, readable_path};
use std::{collections::BTreeMap, fs, path::Path};

/// The optional part of the conversion manifest needed to identify copied settings.
#[derive(Default, Deserialize)]
struct Manifest {
    #[serde(default)]
    ini_files: BTreeMap<String, IniProvenance>,
}

/// Loads pack files in Mudcrab's documented order, or replaces them with the explicit `--ini` list.
/// CLI parsing applies explicit scalar options after this function, independent of argument order.
pub(crate) fn load(config: &mut EngineConfig, args: &[String]) {
    let explicit = args.iter().any(|argument| argument == "--ini");
    let assets = args
        .windows(2)
        .rfind(|pair| pair[0] == "--assets")
        .map(|pair| Path::new(&pair[1]))
        .unwrap_or(&config.assets_dir);
    let manifest = if explicit {
        Manifest::default()
    } else {
        read_manifest(assets)
    };
    let paths: Vec<_> = if explicit {
        args.windows(2)
            .filter(|pair| pair[0] == "--ini")
            .map(|pair| pair[1].clone().into())
            .collect()
    } else {
        FILES
            .iter()
            .map(|name| assets.join("ini").join(name))
            .collect()
    };
    let mut ini = SkyrimIni::default();
    let mut sources = BTreeMap::new();
    for path in paths {
        let bytes = match read_ini_bytes(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                if explicit || error.kind() != std::io::ErrorKind::NotFound {
                    eprintln!(
                        "warning: ignoring INI {}: {error}",
                        readable_path(&path).display()
                    );
                }
                continue;
            }
        };
        let text = ini.merge_bytes(&bytes, &path);
        let source = if explicit {
            format!("--ini ({})", readable_path(&path).display())
        } else {
            pack_source(&path, &bytes, &manifest)
        };
        let mut file = SkyrimIni::default();
        file.merge_str(&text);
        for key in file.key_names() {
            sources.insert(key, source.clone());
        }
    }
    let existing_overrides = config.settings_overrides.len();
    ini.apply(config);
    if !explicit {
        // Pack settings seed menu values; explicit INIs retain launch ownership.
        config.settings_overrides.truncate(existing_overrides);
    }
    for (key, source) in &mut config.ini_value_sources {
        if let Some(found) = sources.get(key) {
            source.clone_from(found);
        }
    }
    if !explicit {
        crate::settings::clamp_pack_menu_values(config);
    }
}

/// Reads optional provenance; older packs and edited copies remain usable.
fn read_manifest(assets: &Path) -> Manifest {
    let path = assets.join("conversion-manifest.json");
    let result = fs::read(&path).and_then(|bytes| {
        serde_json::from_slice(&bytes)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    });
    match result {
        Ok(manifest) => manifest,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!(
                    "warning: ignoring INI provenance {}: {error}",
                    readable_path(&path).display()
                );
            }
            Manifest::default()
        }
    }
}

/// Trusts an original source label only while the copied bytes match the recorded hash.
fn pack_source(path: &Path, bytes: &[u8], manifest: &Manifest) -> String {
    let key = format!(
        "ini/{}",
        path.file_name().unwrap_or_default().to_string_lossy()
    );
    if let Some(provenance) = manifest.ini_files.get(&key)
        && format!("{:x}", Sha256::digest(bytes)) == provenance.sha256
    {
        let kind = match provenance.origin {
            IniOrigin::PlayerFile => "player file",
            IniOrigin::GeneratedFromInstall => "generated-from-install",
            IniOrigin::PackCopy => "pack copy",
        };
        return format!(
            "{kind} ({}; pack copy {})",
            readable_path(&provenance.source).display(),
            readable_path(path).display()
        );
    }
    format!("pack copy ({})", readable_path(path).display())
}

/// Records explicit CLI ownership and fills supported keys with their default source.
pub(crate) fn record_cli_sources(config: &mut EngineConfig) {
    for (flag, key) in [
        ("--always-active", "general.balwaysactive"),
        ("--stream-radius", "general.ugridstoload"),
    ] {
        if config
            .settings_overrides
            .iter()
            .any(|argument| argument == flag)
        {
            config
                .ini_value_sources
                .insert(key.into(), format!("CLI ({flag})"));
        }
    }
    for (key, _) in setting_values(config) {
        config
            .ini_value_sources
            .entry(key)
            .or_insert_with(|| "default".into());
    }
}

/// Marks only explicitly saved INI menu groups after a valid settings file loads.
pub(crate) fn record_saved_sources(config: &mut EngineConfig, owned: u8) {
    for (group, key) in crate::settings::INI_MENU_KEYS.into_iter().enumerate() {
        if owned & (1 << group) != 0
            && !crate::settings::ini_menu_launch_owned(group, &config.settings_overrides)
        {
            config
                .ini_value_sources
                .insert(key.into(), "saved in-game settings".into());
        }
    }
}

/// Lists supported values after the relevant startup layers have applied.
fn setting_values(config: &EngineConfig) -> Vec<(String, String)> {
    let lod = config.terrain_lod;
    let mut values = vec![
        (
            "general.balwaysactive".to_owned(),
            config.always_active.to_string(),
        ),
        (
            "general.ugridstoload".to_owned(),
            (config.stream_radius * 2 + 1).to_string(),
        ),
        (
            "display.fshadowdistance".to_owned(),
            format!("{:?}", config.sun_shadow_distance_limit),
        ),
        (
            "terrainmanager.fblocklevel0distance".to_owned(),
            lod.block_level0_distance.to_string(),
        ),
        (
            "terrainmanager.fblocklevel1distance".to_owned(),
            lod.block_level1_distance.to_string(),
        ),
        (
            "terrainmanager.fblockmaximumdistance".to_owned(),
            lod.block_maximum_distance.to_string(),
        ),
        (
            "terrainmanager.fsplitdistancemult".to_owned(),
            lod.split_distance_mult.to_string(),
        ),
    ];
    for (index, key) in crate::sun::SUN_KEYS.iter().enumerate() {
        values.push((
            format!("sky.{key}").to_ascii_lowercase(),
            match config.sun_settings_overrides[index] {
                Some(value) => value.to_string(),
                None => "pack GMST or retail default".into(),
            },
        ));
    }
    values
}

/// Reports effective non-default sources after saved interactive settings and CLI precedence.
pub(crate) fn report(config: &mut EngineConfig) {
    record_cli_sources(config);
    for (key, value) in setting_values(config) {
        let source = &config.ini_value_sources[&key];
        if source != "default" {
            eprintln!("INI launch setting {key}={value}; source={source}");
        }
    }
    if let Some(position) = config.sun_position {
        eprintln!("sun direction={position:?}; source=CLI (--sun-position)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigAction;

    /// Parses settings from an explicitly selected synthetic pack.
    fn start(pack: &Path, options: &[&str]) -> EngineConfig {
        let args = ["--assets".to_owned(), pack.display().to_string()]
            .into_iter()
            .chain(options.iter().map(|value| (*value).to_owned()));
        match EngineConfig::from_args(args).unwrap() {
            ConfigAction::Run(config) => *config,
            action => panic!("expected run, got {action:?}"),
        }
    }

    /// Makes a pack with distinct values so each precedence layer can be distinguished.
    fn pack() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("ini")).unwrap();
        fs::write(
            directory.path().join("ini/Skyrim.ini"),
            "[General]\nuGridsToLoad=7\nbAlwaysActive=1\n",
        )
        .unwrap();
        fs::write(
            directory.path().join("ini/SkyrimPrefs.ini"),
            "[General]\nuGridsToLoad=9\n[TerrainManager]\nfSplitDistanceMult=2.25\n",
        )
        .unwrap();
        directory
    }

    /// Pack files layer automatically, and explicit files replace their entire settings set.
    #[test]
    fn ini_pack_precedence_cli_explicit_pack_default() {
        let directory = pack();
        let found = start(directory.path(), &[]);
        assert_eq!(
            found.stream_radius, 4,
            "pack prefs must override pack Skyrim.ini"
        );
        assert!(found.always_active);
        assert_eq!(found.terrain_lod.split_distance_mult, 2.25);
        assert!(found.ini_value_sources["general.ugridstoload"].starts_with("pack copy"));
        let explicit = directory.path().join("explicit.ini");
        fs::write(&explicit, "[General]\nuGridsToLoad=3\n").unwrap();
        let path = explicit.display().to_string();
        let replaced = start(directory.path(), &["--ini", &path]);
        assert_eq!(replaced.stream_radius, 1);
        assert!(
            !replaced.always_active,
            "explicit files replace pack files rather than merge"
        );
        assert_eq!(replaced.terrain_lod.split_distance_mult, 1.5);
        assert!(replaced.ini_value_sources["general.ugridstoload"].starts_with("--ini"));
        assert_eq!(
            replaced.ini_value_sources["general.balwaysactive"],
            "default"
        );
        let cli = start(
            directory.path(),
            &["--stream-radius", "5", "--ini", &path, "--always-active"],
        );
        assert_eq!(cli.stream_radius, 5);
        assert!(cli.always_active);
        assert_eq!(
            cli.ini_value_sources["general.ugridstoload"],
            "CLI (--stream-radius)"
        );
        let absent = tempfile::tempdir().unwrap();
        let defaults = start(absent.path(), &[]);
        assert_eq!(
            defaults.stream_radius,
            EngineConfig::default().stream_radius
        );
        assert_eq!(
            defaults.ini_value_sources["general.ugridstoload"],
            "default"
        );
    }

    /// Unreadable files and malformed values warn while valid neighboring files still apply.
    #[test]
    fn ini_pack_bad_files_and_scalars_continue_startup() {
        let directory = pack();
        let path = directory.path().join("ini/Skyrim.ini");
        fs::remove_file(&path).unwrap();
        fs::create_dir(path).unwrap();
        fs::write(
            directory.path().join("ini/SkyrimPrefs.ini"),
            "[General]\nuGridsToLoad=bad\n[TerrainManager]\nfSplitDistanceMult=3.25\n",
        )
        .unwrap();
        let config = start(directory.path(), &[]);
        assert_eq!(config.stream_radius, EngineConfig::default().stream_radius);
        assert_eq!(config.terrain_lod.split_distance_mult, 3.25);
        assert_eq!(config.ini_value_sources["general.ugridstoload"], "default");
        assert!(
            config.ini_value_sources["terrainmanager.fsplitdistancemult"].starts_with("pack copy")
        );
        let missing = directory.path().join("missing.ini").display().to_string();
        let explicit = start(directory.path(), &["--ini", &missing]);
        assert_eq!(
            explicit.terrain_lod.split_distance_mult, 1.5,
            "even an unreadable explicit list replaces the pack"
        );
    }

    /// Original provenance applies to exact copied bytes; local edits become pack copies.
    #[test]
    fn ini_pack_sources_require_matching_hashes() {
        let directory = pack();
        let mut files = BTreeMap::new();
        for (name, origin) in FILES
            .into_iter()
            .zip([IniOrigin::PlayerFile, IniOrigin::GeneratedFromInstall])
        {
            let key = format!("ini/{name}");
            let bytes = fs::read(directory.path().join(&key)).unwrap();
            files.insert(
                key,
                IniProvenance {
                    source: fs::canonicalize(directory.path().join(format!("ini/{name}"))).unwrap(),
                    origin,
                    sha256: format!("{:x}", Sha256::digest(bytes)),
                },
            );
        }
        fs::write(
            directory.path().join("conversion-manifest.json"),
            serde_json::to_vec(&serde_json::json!({"ini_files": files})).unwrap(),
        )
        .unwrap();
        let config = start(directory.path(), &[]);
        assert!(config.ini_value_sources["general.balwaysactive"].starts_with("player file"));
        assert!(
            config.ini_value_sources["general.ugridstoload"].starts_with("generated-from-install")
        );
        #[cfg(windows)]
        assert!(
            config
                .ini_value_sources
                .values()
                .all(|source| !source.contains(r"\\?\")),
            "INI diagnostics use readable Windows paths"
        );
        fs::write(
            directory.path().join("ini/SkyrimPrefs.ini"),
            "[General]\nuGridsToLoad=11\n",
        )
        .unwrap();
        let edited = start(directory.path(), &[]);
        assert_eq!(edited.stream_radius, 5);
        assert!(edited.ini_value_sources["general.ugridstoload"].starts_with("pack copy"));
    }

    /// Provenance hashes describe the copied bytes for each supported text encoding.
    #[test]
    fn ini_pack_encoded_bytes_preserve_provenance_hash() {
        let directory = pack();
        let text = "[General]\nuGridsToLoad=11\n";
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        fs::write(directory.path().join("ini/SkyrimPrefs.ini"), &bytes).unwrap();
        fs::write(
            directory.path().join("conversion-manifest.json"),
            serde_json::to_vec(&serde_json::json!({
                "ini_files": {
                    "ini/SkyrimPrefs.ini": {
                        "source": "SkyrimPrefs.ini",
                        "origin": "player_file",
                        "sha256": format!("{:x}", Sha256::digest(&bytes)),
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let config = start(directory.path(), &[]);
        assert_eq!(config.stream_radius, 5);
        assert!(config.ini_value_sources["general.ugridstoload"].starts_with("player file"));
    }

    /// The engine verifies a manifest serialized by the converter using its published hash helper.
    #[test]
    fn ini_converter_manifest_hash_matches_engine_loader() {
        let directory = pack();
        let bytes = b"[General]\nuGridsToLoad=13\n";
        fs::write(directory.path().join("ini/SkyrimPrefs.ini"), bytes).unwrap();
        let mut manifest = converter::cache::ConversionManifest::default();
        manifest.ini_files.insert(
            "ini/SkyrimPrefs.ini".into(),
            IniProvenance {
                source: "SkyrimPrefs.ini".into(),
                sha256: converter::cache::hash_bytes(bytes),
                origin: IniOrigin::PlayerFile,
            },
        );
        manifest
            .save(&directory.path().join("conversion-manifest.json"))
            .unwrap();
        let config = start(directory.path(), &[]);
        assert_eq!(config.stream_radius, 6);
        assert!(config.ini_value_sources["general.ugridstoload"].starts_with("player file"));
    }

    /// Huge pack grids become bounded configuration values before any streaming grid is allocated.
    #[test]
    fn ini_pack_huge_grid_is_bounded_while_explicit_launch_values_keep_their_rules() {
        let directory = pack();
        fs::write(
            directory.path().join("ini/SkyrimPrefs.ini"),
            "[General]\nuGridsToLoad=2000001\n[TerrainManager]\nfSplitDistanceMult=4.5\n",
        )
        .unwrap();
        let bounded = start(directory.path(), &[]);
        assert_eq!((bounded.stream_radius, bounded.unload_radius), (16, 17));
        assert_eq!(bounded.terrain_lod.split_distance_mult, 4.0);
        assert!(bounded.ini_value_sources["general.ugridstoload"].contains("clamped pack INI"));
        let path = directory.path().join("explicit.ini");
        fs::write(
            &path,
            "[General]\nuGridsToLoad=35\n[TerrainManager]\nfSplitDistanceMult=4.5\n",
        )
        .unwrap();
        let explicit = start(directory.path(), &["--ini", path.to_str().unwrap()]);
        assert_eq!((explicit.stream_radius, explicit.unload_radius), (17, 18));
        assert_eq!(explicit.terrain_lod.split_distance_mult, 4.5);
        assert!(!explicit.ini_value_sources["general.ugridstoload"].contains("clamped"));
        let cli = start(directory.path(), &["--stream-radius", "18"]);
        assert_eq!((cli.stream_radius, cli.unload_radius), (18, 19));
        assert_eq!(
            cli.ini_value_sources["general.ugridstoload"],
            "CLI (--stream-radius)"
        );
    }
}
