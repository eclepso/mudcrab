//! Asset-only publications and their resumes obey the same native generation contract.
mod common;

use converter::{AssetPipeline, PipelineConfig, TextureEncoder, config::RecordReader};
use std::{fs, path::Path};

fn copy_current(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let destination = target.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_current(&entry.path(), &destination);
        } else {
            fs::copy(entry.path(), destination).unwrap();
        }
    }
}

async fn publish(config: PipelineConfig) {
    let (sender, mut receiver) = tokio::sync::mpsc::channel(128);
    let drain = tokio::spawn(async move { while receiver.recv().await.is_some() {} });
    let report = AssetPipeline::run_async(config, sender).await.unwrap();
    drain.await.unwrap();
    assert!(report.complete, "{report:?}");
    assert!(report.warnings.is_empty(), "{report:?}");
    assert_eq!(report.skipped, 0);
}

async fn asset_only_world(reader: RecordReader) {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path().join("Data");
    let output = directory.path().join("assets");
    fs::create_dir_all(data.join("scripts")).unwrap();
    fs::write(
        data.join("scripts/only.pex"),
        dummy_content::pex::minimal("Only").unwrap(),
    )
    .unwrap();
    let mut config = PipelineConfig::new(&data, &output);
    config.record_reader = reader;
    config.no_lod = true;
    config.cpu_jobs = 2;
    config.texture_encoder = TextureEncoder::Cpu;
    config.lod_texture_encoder = TextureEncoder::Cpu;
    publish(config.clone()).await;
    let validate = || {
        assert!(
            output.join("skyrim_world.db").is_file(),
            "asset-only publication must write an authenticated native empty world"
        );
        let manifest =
            converter::cache::ConversionManifest::load(&output.join("conversion-manifest.json"))
                .unwrap();
        let identity = manifest.validate_world_generation(&output).unwrap();
        assert!(identity.order().is_empty());
        let connection = rusqlite::Connection::open(output.join("skyrim_world.db")).unwrap();
        shared::records::RecordContextReader::new(&connection).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM records", [], |row| row
                    .get::<_, u32>(0))
                .unwrap(),
            0
        );
        let cache =
            converter::esm::cell_cache::read_form_key_cell_cache(&output.join("cell_cache.rkyv"))
                .unwrap();
        assert!(cache.cells.is_empty());
        assert!(output.join("scripts/only.luau").is_file());
    };
    validate();
    let staging = output.with_extension(format!("staging-{}-1", std::process::id()));
    copy_current(&output, &staging);
    common::current_staging_generation(&staging);
    config.resume_staging = Some(staging);
    publish(config).await;
    validate();
}

#[tokio::test]
async fn legacy_asset_only_publication_has_a_resumable_native_empty_world() {
    asset_only_world(RecordReader::Legacy).await;
}

#[tokio::test]
async fn typed_asset_only_publication_has_a_resumable_native_empty_world() {
    asset_only_world(RecordReader::Inhouse).await;
}
