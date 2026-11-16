//! Missing and deleted LAND textures have distinct runtime and preserved cache policies.
use converter::{
    AssetPipeline, PipelineConfig, TextureEncoder,
    config::RecordReader,
    esm::{
        EsmParser,
        cell_cache::{read_form_key_cell_cache, write_form_key_cell_cache},
    },
};
use shared::{form_key::FormKey, pack_identity::PackIdentity, records::RecordContextReader};
use std::{collections::HashMap, fs, path::Path};

/// Bound every original native field independently of the production parser.
fn sub(tag: &[u8; 4], bytes: &[u8]) -> Vec<u8> {
    [tag.as_slice(), &(bytes.len() as u16).to_le_bytes(), bytes].concat()
}

/// Keep the source header's word, deletion bit and version as literal test facts.
fn record(tag: &[u8; 4], word: u32, flags: u32, fields: &[u8]) -> Vec<u8> {
    [
        tag.as_slice(),
        &(fields.len() as u32).to_le_bytes(),
        &flags.to_le_bytes(),
        &word.to_le_bytes(),
        &[0; 4],
        &44u16.to_le_bytes(),
        &[0; 2],
        fields,
    ]
    .concat()
}

/// Parent CELL and WRLD references come from genuine bounded native group headers.
fn group(kind: u32, word: u32, body: &[u8]) -> Vec<u8> {
    [
        b"GRUP".as_slice(),
        &((body.len() + 24) as u32).to_le_bytes(),
        &word.to_le_bytes(),
        &kind.to_le_bytes(),
        &[0; 8],
        body,
    ]
    .concat()
}

/// One flat exterior cell declares its base texture independently in all four quadrants.
fn cell(word: u32, land: u32, x: i32, texture: u32) -> Vec<u8> {
    let mut heights = 1.0f32.to_le_bytes().to_vec();
    heights.extend(vec![0; 33 * 33 + 3]);
    let mut fields = sub(b"VHGT", &heights);
    for quadrant in 0..4u8 {
        fields.extend(sub(
            b"BTXT",
            &[texture.to_le_bytes().as_slice(), &[quadrant, 0, 0, 0]].concat(),
        ));
    }
    [
        record(
            b"CELL",
            word,
            0,
            &[
                sub(b"EDID", format!("Cell{word:X}\0").as_bytes()),
                sub(b"DATA", &[0; 2]),
                sub(b"XCLC", &[x.to_le_bytes(), [0; 4], [0; 4]].concat()),
            ]
            .concat(),
        ),
        group(6, word, &record(b"LAND", land, 0, &fields)),
    ]
    .concat()
}

/// Skyrim is supplier0 but allocated owner2; Patch's deletion still names original Skyrim4.
fn plugin(root: &Path, name: &str, masters: &[&str], body: &[u8]) {
    let mut fields = sub(
        b"HEDR",
        &[1.71f32.to_le_bytes(), [0; 4], 0x1000u32.to_le_bytes()].concat(),
    );
    for master in masters {
        fields.extend(sub(b"MAST", format!("{master}\0").as_bytes()));
        fields.extend(sub(b"DATA", &[0; 8]));
    }
    fs::write(
        root.join(name),
        [
            record(b"TES4", 0, u32::from(name.ends_with(".esm")), &fields),
            body.to_vec(),
        ]
        .concat(),
    )
    .unwrap();
}

/// Exercise both final catalogue states without changing original source words or neighbors.
async fn compare_policies(reader: RecordReader) {
    for deleted in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let data = directory.path().join("Data");
        let output = directory.path().join("assets");
        fs::create_dir(&data).unwrap();
        let mut body = [
            record(b"WRLD", 0x600, 0, &sub(b"EDID", b"TexturePolicyWorld\0")),
            group(
                1,
                0x600,
                &[cell(0x700, 0x800, 0, 4), cell(0x701, 0x801, 1, 0x844)].concat(),
            ),
            record(b"LTEX", 0x844, 0, &sub(b"EDID", b"LiveTexture\0")),
        ]
        .concat();
        if deleted {
            body.extend(record(b"LTEX", 4, 0, &sub(b"EDID", b"DeletedTexture\0")));
        }
        plugin(&data, "Skyrim.esm", &[], &body);
        let deletion = if deleted {
            record(b"LTEX", 4, 0x20, &[])
        } else {
            Vec::new()
        };
        plugin(&data, "Patch.esp", &["Skyrim.esm"], &deletion);
        let paths = [data.join("Skyrim.esm"), data.join("Patch.esp")];
        let original = paths
            .iter()
            .map(|path| fs::read(path).unwrap())
            .collect::<Vec<_>>();
        let list = directory.path().join("plugins.txt");
        fs::write(&list, "*Skyrim.esm\n*Patch.esp\n").unwrap();
        let mut config = PipelineConfig::new(&data, &output);
        config.plugins_file = Some(list);
        config.record_reader = reader;
        config.no_lod = true;
        config.cpu_jobs = 2;
        config.texture_encoder = TextureEncoder::Cpu;
        config.lod_texture_encoder = TextureEncoder::Cpu;
        let (sender, mut events) = tokio::sync::mpsc::channel(128);
        let drain = tokio::spawn(async move { while events.recv().await.is_some() {} });
        let report = AssetPipeline::run_async(config, sender).await.unwrap();
        drain.await.unwrap();
        assert_eq!(report.skipped, 0);
        if reader == RecordReader::Legacy {
            // Legacy publication has no auxiliary member; exercise the same explicit preserved
            // writer against that reader's genuine final native winners and authenticated world.
            let native = EsmParser::merge_plugins_form_keys(&paths).unwrap();
            let (_, warnings) = write_form_key_cell_cache(
                &native,
                &HashMap::new(),
                &output.join("cell_cache_preserved.rkyv"),
                true,
            )
            .unwrap();
            assert!(
                warnings.is_empty(),
                "preservation must not clear targets: {warnings:?}"
            );
        }
        let database = rusqlite::Connection::open(output.join("skyrim_world.db")).unwrap();
        let identity = PackIdentity::read_sql(&database).unwrap();
        assert_eq!(identity.table().id("Skyrim.esm").unwrap().get(), 2);
        let absent: u32 = database
            .query_row(
                "SELECT count(*) FROM records WHERE form_id=33554436",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            absent, 0,
            "the final catalogue must really lack Skyrim LTEX4"
        );
        let context = RecordContextReader::new(&database).unwrap();
        let land = context
            .record(FormKey::from_packed(0x0200_0800).unwrap())
            .unwrap();
        assert_eq!(land.bytes(*b"BTXT").unwrap()[..4], 4u32.to_le_bytes());
        let runtime = read_form_key_cell_cache(&output.join("cell_cache.rkyv")).unwrap();
        let preserved =
            read_form_key_cell_cache(&output.join("cell_cache_preserved.rkyv")).unwrap();
        assert_eq!(runtime.cells.len(), 2, "usable neighbor must survive");
        let good = runtime
            .cells
            .iter()
            .find(|cell| cell.cell_id == 0x0200_0701)
            .unwrap();
        assert_eq!(
            good.layers
                .iter()
                .map(|layer| layer.texture_form_id)
                .collect::<Vec<_>>(),
            vec![0x0200_0844; 4]
        );
        let original_bad = preserved
            .cells
            .iter()
            .find(|cell| cell.cell_id == 0x0200_0700)
            .unwrap();
        assert_eq!(
            original_bad
                .layers
                .iter()
                .map(|layer| layer.texture_form_id)
                .collect::<Vec<_>>(),
            vec![0x0200_0004; 4]
        );
        let cleared = runtime
            .cells
            .iter()
            .find(|cell| cell.cell_id == 0x0200_0700)
            .unwrap();
        assert_eq!(
            cleared
                .layers
                .iter()
                .map(|layer| layer.texture_form_id)
                .collect::<Vec<_>>(),
            vec![0; 4],
            "runtime cache must clear LTEX absent from final live catalogue"
        );
        let losses = report
            .warnings
            .iter()
            .filter(|warning| warning.contains("unavailable runtime LAND texture assignments"))
            .collect::<Vec<_>>();
        assert_eq!(
            losses.len(),
            1,
            "new clearings need one bounded supplier warning: {:?}",
            report.warnings
        );
        assert!(losses[0].contains("Skyrim.esm: cleared 4"));
        assert!(losses[0].contains("first:") && losses[0].contains("000004:Skyrim.esm"));
        for (path, bytes) in paths.iter().zip(original) {
            assert_eq!(fs::read(path).unwrap(), bytes, "input plugin changed");
        }
    }
}

/// Handwritten source resolution must not certify absent/deleted texture catalogue entries.
#[tokio::test]
async fn legacy_runtime_ltex_is_live_while_preserved_keeps_source_identity() {
    compare_policies(RecordReader::Legacy).await;
}

/// Typed built-in reference admission must not bypass runtime LTEX catalogue validation.
#[tokio::test]
async fn inhouse_runtime_ltex_is_live_while_preserved_keeps_source_identity() {
    compare_policies(RecordReader::Inhouse).await;
}
