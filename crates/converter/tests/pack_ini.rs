//! Synthetic settings inputs through real staged conversion and pack publication.

use converter::{AssetPipeline, PipelineConfig, PipelineReport, cache::ConversionManifest};
use shared::pack_ini::IniOrigin;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// A synthetic install, isolated player settings and a real converter destination.
struct Fixture {
    _root: tempfile::TempDir,
    config: PipelineConfig,
    player: PathBuf,
    install: PathBuf,
}

impl Fixture {
    /// Keeps every test input independent of ambient Documents and game installs.
    fn new(installed: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let install = root.path().join("game");
        let data = install.join("Data");
        let player = root
            .path()
            .join("redirected Documents/My Games/Skyrim Special Edition");
        fs::create_dir_all(data.join("scripts")).unwrap();
        fs::create_dir_all(&player).unwrap();
        fs::write(
            data.join("scripts/Neighbor.pex"),
            dummy_content::pex::minimal("Neighbor").unwrap(),
        )
        .unwrap();
        if installed {
            // An empty synthetic TES4 header supplies the existing Skyrim Data detection signal.
            let mut master = b"TES4".to_vec();
            master.extend_from_slice(&[0; 20]);
            fs::write(data.join("Skyrim.esm"), master).unwrap();
            fs::write(install.join("SkyrimSE.exe"), b"synthetic executable marker").unwrap();
            fs::write(
                install.join("Skyrim_Default.ini"),
                "[General]\nuGridsToLoad=9\n",
            )
            .unwrap();
            fs::write(
                install.join("High.ini"),
                "[Display]\nfShadowDistance=7345\n",
            )
            .unwrap();
        }
        let mut config = PipelineConfig::new(data, root.path().join("pack"));
        config.player_ini_dir = Some(player.clone());
        config.no_lod = true;
        config.cpu_jobs = 1;
        Self {
            _root: root,
            config,
            player,
            install,
        }
    }

    /// Publishes usable neighboring script data even when optional settings are unavailable.
    async fn publish(&self) -> PipelineReport {
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let result = AssetPipeline::run_async(self.config.clone(), tx).await;
        drain.await.unwrap();
        let report = result.expect("optional INIs leave conversion usable");
        assert!(
            report.complete,
            "optional INIs preserve completeness: {report:?}"
        );
        assert!(self.manifest().complete);
        assert!(
            self.config
                .output_dir
                .join("scripts/neighbor.luau")
                .is_file(),
            "valid neighboring script must survive: {report:?}"
        );
        report
    }

    /// Loads the metadata from the published pack, rather than inspecting staging.
    fn manifest(&self) -> ConversionManifest {
        ConversionManifest::load(&self.config.output_dir.join("conversion-manifest.json")).unwrap()
    }

    /// Checks independent bytes and their exact source/hash identity in the published manifest.
    fn assert_copy(&self, name: &str, source: &Path, bytes: &[u8], origin: IniOrigin) {
        let relative = format!("ini/{name}");
        assert!(
            self.config.output_dir.join(&relative).is_file(),
            "expected published INI {relative}"
        );
        assert_eq!(
            fs::read(self.config.output_dir.join(&relative)).unwrap(),
            bytes
        );
        let provenance = self.manifest().ini_files[&relative].clone();
        assert_eq!(
            provenance.source.file_name().unwrap(),
            source.file_name().unwrap()
        );
        assert!(
            provenance.source.is_relative(),
            "settings provenance stays portable"
        );
        #[cfg(windows)]
        assert!(
            !provenance.source.to_string_lossy().starts_with(r"\\?\"),
            "INI provenance uses a readable Windows path"
        );
        assert_eq!(provenance.sha256, converter::cache::hash_bytes(bytes));
        assert_eq!(provenance.origin, origin);
    }

    /// Configures the selected MO2 profile over the same physical synthetic Data folder.
    fn mo2_profile(
        &mut self,
        profile_setting: Option<bool>,
        instance_setting: Option<bool>,
    ) -> PathBuf {
        let instance = self._root.path().join("MO2");
        let profile = instance.join("profiles/Selected");
        for folder in [
            instance.join("mods"),
            instance.join("overwrite"),
            profile.clone(),
        ] {
            fs::create_dir_all(folder).unwrap();
        }
        let mut settings =
            "[General]\ngameName=Skyrim Special Edition\nselected_profile=Selected\n".to_owned();
        if let Some(enabled) = instance_setting {
            settings.push_str(&format!("[Settings]\nprofile_local_inis={enabled}\n"));
        }
        fs::write(instance.join("ModOrganizer.ini"), settings).unwrap();
        let mut plugin = b"TES4".to_vec();
        plugin.extend_from_slice(&[0; 20]);
        fs::write(self.config.data_dir.join("Neighbor.esp"), plugin).unwrap();
        fs::write(profile.join("plugins.txt"), "*Neighbor.esp\n").unwrap();
        fs::write(profile.join("modlist.txt"), "").unwrap();
        if let Some(enabled) = profile_setting {
            fs::write(
                profile.join("settings.ini"),
                format!("[General]\nLocalSettings={enabled}\n"),
            )
            .unwrap();
        }
        self.config.mo2 = Some(mo2::Selection {
            instance_path: instance,
            profile: "Selected".into(),
        });
        profile
    }
}

/// Both player files carry exact original bytes without needing an install to generate them.
#[tokio::test]
async fn found_player_inis_are_copied_and_hashed() {
    let fixture = Fixture::new(false);
    for (name, bytes) in [
        ("Skyrim.ini", b"[General]\nuGridsToLoad=7\n".as_slice()),
        (
            "SkyrimPrefs.ini",
            b"[Display]\nfShadowDistance=4567\n".as_slice(),
        ),
    ] {
        fs::write(fixture.player.join(name), bytes).unwrap();
    }
    let report = fixture.publish().await;
    assert!(report.complete, "{report:?}");
    fixture.assert_copy(
        "Skyrim.ini",
        &fixture.player.join("Skyrim.ini"),
        b"[General]\nuGridsToLoad=7\n",
        IniOrigin::PlayerFile,
    );
    fixture.assert_copy(
        "SkyrimPrefs.ini",
        &fixture.player.join("SkyrimPrefs.ini"),
        b"[Display]\nfShadowDistance=4567\n",
        IniOrigin::PlayerFile,
    );
}

/// A confirmed install contributes its own default and High template for missing player files.
#[tokio::test]
async fn missing_player_inis_generate_only_from_a_confirmed_install() {
    let fixture = Fixture::new(true);
    fixture.publish().await;
    fixture.assert_copy(
        "Skyrim.ini",
        &fixture.install.join("Skyrim_Default.ini"),
        b"[General]\nuGridsToLoad=9\n",
        IniOrigin::GeneratedFromInstall,
    );
    fixture.assert_copy(
        "SkyrimPrefs.ini",
        &fixture.install.join("High.ini"),
        b"[Display]\nfShadowDistance=7345\n",
        IniOrigin::GeneratedFromInstall,
    );
}

/// A partial player pair preserves the authored file and fills only its missing neighbor.
#[tokio::test]
async fn a_partial_pair_only_generates_the_missing_ini() {
    let fixture = Fixture::new(true);
    fs::write(
        fixture.player.join("Skyrim.ini"),
        "[General]\nuGridsToLoad=3\n",
    )
    .unwrap();
    fixture.publish().await;
    fixture.assert_copy(
        "Skyrim.ini",
        &fixture.player.join("Skyrim.ini"),
        b"[General]\nuGridsToLoad=3\n",
        IniOrigin::PlayerFile,
    );
    fixture.assert_copy(
        "SkyrimPrefs.ini",
        &fixture.install.join("High.ini"),
        b"[Display]\nfShadowDistance=7345\n",
        IniOrigin::GeneratedFromInstall,
    );
}

/// Missing install evidence leaves runtime defaults in effect, including on a warm conversion.
#[tokio::test]
async fn missing_install_writes_no_inis_and_reconversion_drops_stale_copies() {
    let fixture = Fixture::new(false);
    fs::write(
        fixture.player.join("Skyrim.ini"),
        "[General]\nuGridsToLoad=7\n",
    )
    .unwrap();
    fixture.publish().await;
    fs::remove_file(fixture.player.join("Skyrim.ini")).unwrap();
    fixture.publish().await;
    assert!(fixture.manifest().ini_files.is_empty());
    assert!(!fixture.config.output_dir.join("ini/Skyrim.ini").exists());
    assert!(
        !fixture
            .config
            .output_dir
            .join("ini/SkyrimPrefs.ini")
            .exists()
    );
}

/// Generation requires the installed game's executable alongside the templates.
#[tokio::test]
async fn templates_without_the_game_executable_write_no_inis() {
    let fixture = Fixture::new(true);
    fs::remove_file(fixture.install.join("SkyrimSE.exe")).unwrap();
    fixture.publish().await;
    assert!(fixture.manifest().ini_files.is_empty());
    assert!(!fixture.config.output_dir.join("ini").exists());
}

/// Each template is optional independently, so a partial install contributes its usable file.
#[tokio::test]
async fn a_missing_template_only_skips_its_corresponding_ini() {
    let fixture = Fixture::new(true);
    fs::remove_file(fixture.install.join("High.ini")).unwrap();
    fixture.publish().await;
    fixture.assert_copy(
        "Skyrim.ini",
        &fixture.install.join("Skyrim_Default.ini"),
        b"[General]\nuGridsToLoad=9\n",
        IniOrigin::GeneratedFromInstall,
    );
    assert!(
        !fixture
            .config
            .output_dir
            .join("ini/SkyrimPrefs.ini")
            .exists()
    );
    assert_eq!(fixture.manifest().ini_files.len(), 1);
}

/// Generation requires the existing Data-folder detection signal as well as the executable.
#[tokio::test]
async fn templates_without_skyrim_data_write_no_inis() {
    let fixture = Fixture::new(true);
    fs::remove_file(fixture.config.data_dir.join("Skyrim.esm")).unwrap();
    fixture.publish().await;
    assert!(fixture.manifest().ini_files.is_empty());
    assert!(!fixture.config.output_dir.join("ini").exists());
}

/// A directory at an INI path is unreadable as a file and warns while usable assets publish.
#[tokio::test]
async fn unreadable_player_ini_warns_and_keeps_the_other_copy() {
    let fixture = Fixture::new(true);
    fs::create_dir(fixture.player.join("Skyrim.ini")).unwrap();
    fs::write(
        fixture.player.join("SkyrimPrefs.ini"),
        "[Display]\nfShadowDistance=5137\n",
    )
    .unwrap();
    let report = fixture.publish().await;
    assert!(
        report.notices.iter().any(
            |warning| warning.contains("skipping unreadable or invalid INI")
                && warning.contains("Skyrim.ini")
        ),
        "{report:?}"
    );
    assert!(!fixture.config.output_dir.join("ini/Skyrim.ini").exists());
    fixture.assert_copy(
        "SkyrimPrefs.ini",
        &fixture.player.join("SkyrimPrefs.ini"),
        b"[Display]\nfShadowDistance=5137\n",
        IniOrigin::PlayerFile,
    );
}

/// Structurally invalid files stay out of the pack while usable assets publish.
#[tokio::test]
async fn malformed_player_inis_warn_and_skip() {
    let fixture = Fixture::new(false);
    fs::write(fixture.player.join("Skyrim.ini"), [0xff, 0xfe, 0x00]).unwrap();
    fs::write(
        fixture.player.join("SkyrimPrefs.ini"),
        "invalid fixture without settings",
    )
    .unwrap();
    let report = fixture.publish().await;
    assert_eq!(
        report
            .notices
            .iter()
            .filter(|warning| warning.contains("skipping unreadable or invalid INI"))
            .count(),
        2
    );
    assert!(fixture.manifest().ini_files.is_empty());
}

/// The selected MO2 profile wins when its local settings checkbox is enabled.
#[tokio::test]
async fn mo2_local_settings_override_documents_and_obey_the_checkbox() {
    let mut fixture = Fixture::new(false);
    let profile = fixture.mo2_profile(Some(true), Some(false));
    fs::write(profile.join("Skyrim.ini"), "[General]\nuGridsToLoad=11\n").unwrap();
    fs::write(
        fixture.player.join("Skyrim.ini"),
        "[General]\nuGridsToLoad=3\n",
    )
    .unwrap();
    fixture.publish().await;
    fixture.assert_copy(
        "Skyrim.ini",
        &profile.join("Skyrim.ini"),
        b"[General]\nuGridsToLoad=11\n",
        IniOrigin::PlayerFile,
    );
    fs::write(
        profile.join("settings.ini"),
        "[General]\nLocalSettings=false\n",
    )
    .unwrap();
    fixture.publish().await;
    fixture.assert_copy(
        "Skyrim.ini",
        &fixture.player.join("Skyrim.ini"),
        b"[General]\nuGridsToLoad=3\n",
        IniOrigin::PlayerFile,
    );
}

/// Profiles without an override use MO2's instance default, whose own default is enabled.
#[tokio::test]
async fn mo2_local_settings_follow_the_instance_default() {
    for enabled in [None, Some(false), Some(true)] {
        let mut fixture = Fixture::new(false);
        let profile = fixture.mo2_profile(None, enabled);
        fs::write(profile.join("Skyrim.ini"), "[General]\nuGridsToLoad=13\n").unwrap();
        fs::write(
            fixture.player.join("Skyrim.ini"),
            "[General]\nuGridsToLoad=5\n",
        )
        .unwrap();
        fixture.publish().await;
        let expected = if enabled == Some(false) {
            &fixture.player
        } else {
            &profile
        };
        assert_eq!(
            fs::read(fixture.config.output_dir.join("ini/Skyrim.ini")).unwrap(),
            fs::read(expected.join("Skyrim.ini")).unwrap()
        );
        #[cfg(windows)]
        assert!(
            !fixture.manifest().ini_files["ini/Skyrim.ini"]
                .source
                .to_string_lossy()
                .starts_with(r"\\?\"),
            "INI provenance uses a readable Windows path"
        );
    }
}

/// An unreadable optional MO2 checkbox file warns and leaves usable conversion inputs intact.
#[tokio::test]
async fn unreadable_mo2_ini_settings_warn_and_conversion_continues() {
    let mut fixture = Fixture::new(false);
    let profile = fixture.mo2_profile(None, None);
    fs::create_dir(profile.join("settings.ini")).unwrap();
    fs::write(
        fixture.player.join("Skyrim.ini"),
        "[General]\nuGridsToLoad=3\n",
    )
    .unwrap();
    let report = fixture.publish().await;
    assert!(
        report
            .notices
            .iter()
            .any(|warning| warning.contains("could not resolve MO2 profile INIs")),
        "{report:?}"
    );
    assert!(fixture.manifest().ini_files.is_empty());
}

/// The source folder is read-only even when a supplied output would overlap it.
#[tokio::test]
async fn player_ini_source_is_protected_from_converter_write_paths() {
    let mut fixture = Fixture::new(false);
    let source = fixture.player.join("Skyrim.ini");
    fs::write(&source, b"[General]\nuGridsToLoad=3\n").unwrap();
    fixture.config.output_dir = fixture.player.join("pack");
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let result = AssetPipeline::run_async(fixture.config.clone(), tx).await;
    drain.await.unwrap();
    let error = result.expect_err("conversion must protect the player settings folder");
    assert!(error.to_string().contains("overlaps"), "{error}");
    assert!(
        error.to_string().contains("player settings folder"),
        "{error}"
    );
    assert!(!error.to_string().contains(r"\\?\"), "{error}");
    assert_eq!(fs::read(source).unwrap(), b"[General]\nuGridsToLoad=3\n");
}

/// Skyrim encodings retain ASCII settings and exact-byte hashes, including single-byte comments.
#[tokio::test]
async fn tolerated_ini_encodings_preserve_settings_and_complete_conversion() {
    let text = "[General]\nuGridsToLoad=7\n; comment\n";
    let mut encodings = vec![
        text.as_bytes().to_vec(),
        [b"\xef\xbb\xbf".as_slice(), text.as_bytes()].concat(),
    ];
    encodings.push(
        [
            vec![0xff, 0xfe],
            text.encode_utf16().flat_map(u16::to_le_bytes).collect(),
        ]
        .concat(),
    );
    encodings.push(
        [
            vec![0xfe, 0xff],
            text.encode_utf16().flat_map(u16::to_be_bytes).collect(),
        ]
        .concat(),
    );
    encodings.push(b"[General]\nuGridsToLoad=7\n; caf\xe9\n".to_vec());
    for bytes in encodings {
        let fixture = Fixture::new(false);
        fs::write(fixture.player.join("Skyrim.ini"), &bytes).unwrap();
        let report = fixture.publish().await;
        fixture.assert_copy(
            "Skyrim.ini",
            &fixture.player.join("Skyrim.ini"),
            &bytes,
            IniOrigin::PlayerFile,
        );
        if bytes.contains(&0xe9) {
            assert!(report.notices.iter().any(|notice| notice.contains("lossy")));
        }
    }
}

/// Failed profile configuration lookup preserves absence rather than generating replacement values.
#[tokio::test]
async fn failed_mo2_lookup_never_generates_from_install() {
    let mut fixture = Fixture::new(true);
    let profile = fixture.mo2_profile(None, None);
    fs::create_dir(profile.join("settings.ini")).unwrap();
    fixture.publish().await;
    assert!(fixture.manifest().ini_files.is_empty());
}

/// MO2 initializes an absent profile file from Documents independently of its neighbor.
#[tokio::test]
async fn missing_mo2_profile_ini_falls_back_per_file_to_documents() {
    let mut fixture = Fixture::new(true);
    let profile = fixture.mo2_profile(Some(true), None);
    fs::write(profile.join("Skyrim.ini"), b"[General]\nuGridsToLoad=11\n").unwrap();
    let bytes = b"[Display]\nfShadowDistance=3123\n";
    fs::write(fixture.player.join("SkyrimPrefs.ini"), bytes).unwrap();
    fixture.publish().await;
    fixture.assert_copy(
        "SkyrimPrefs.ini",
        &fixture.player.join("SkyrimPrefs.ini"),
        bytes,
        IniOrigin::PlayerFile,
    );
}

/// An explicit unreadable settings folder preserves lookup failure for both files.
#[tokio::test]
async fn failed_settings_folder_lookup_never_generates_from_install() {
    let fixture = Fixture::new(true);
    fs::remove_dir(&fixture.player).unwrap();
    fs::write(&fixture.player, b"synthetic file in place of folder").unwrap();
    fixture.publish().await;
    assert!(fixture.manifest().ini_files.is_empty());
}

/// Edited pack settings survive both ordinary reconversion and metadata rebuild.
#[tokio::test]
async fn edited_pack_ini_is_kept_and_reported_on_reconversion() {
    let fixture = Fixture::new(true);
    fixture.publish().await;
    let edited = b"[General]\nuGridsToLoad=15\n";
    fs::write(fixture.config.output_dir.join("ini/Skyrim.ini"), edited).unwrap();
    let report = fixture.publish().await;
    assert_eq!(
        fs::read(fixture.config.output_dir.join("ini/Skyrim.ini")).unwrap(),
        edited
    );
    assert_eq!(
        fixture.manifest().ini_files["ini/Skyrim.ini"].origin,
        IniOrigin::PackCopy
    );
    assert!(
        report
            .notices
            .iter()
            .any(|notice| notice.contains("kept") && notice.contains("Skyrim.ini"))
    );
}

/// Account identifiers are removed while portable provenance identifies the source file.
#[tokio::test]
async fn captured_inis_omit_account_identifiers() {
    let fixture = Fixture::new(false);
    fs::write(
        fixture.player.join("SkyrimPrefs.ini"),
        b"[Bethesda.net]\nuAccountId=123456\n[Display]\nfShadowDistance=4567\n",
    )
    .unwrap();
    fixture.publish().await;
    let bytes = fs::read(fixture.config.output_dir.join("ini/SkyrimPrefs.ini")).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("123456"));
    assert!(String::from_utf8_lossy(&bytes).contains("fShadowDistance=4567"));
    assert_eq!(
        fixture.manifest().ini_files["ini/SkyrimPrefs.ini"].sha256,
        converter::cache::hash_bytes(&bytes)
    );
}

/// Optional-INI diagnostics preserve the converter process success status as well as the manifest.
#[test]
fn optional_ini_notice_preserves_cli_exit_zero() {
    let fixture = Fixture::new(false);
    fs::write(fixture.player.join("Skyrim.ini"), b"invalid settings").unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_converter"))
        .args([
            fixture.config.data_dir.as_os_str(),
            fixture.config.output_dir.as_os_str(),
        ])
        .arg("--player-ini-dir")
        .arg(&fixture.player)
        .arg("--no-lod")
        .arg("--cpu-jobs")
        .arg("1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(fixture.manifest().complete);
}
