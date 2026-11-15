//! Independent native terrain inputs, built only while the integration tests run.
use converter::{
    AssetPipeline, PipelineConfig, PipelineReport, TextureEncoder, config::RecordReader,
};
use dummy_content::{dds, rng::Rng};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub struct Publication {
    _directory: tempfile::TempDir,
    pub output: PathBuf,
    pub report: PipelineReport,
}

fn sub(tag: &[u8; 4], bytes: &[u8]) -> Vec<u8> {
    [
        tag.as_slice(),
        &u16::try_from(bytes.len()).unwrap().to_le_bytes(),
        bytes,
    ]
    .concat()
}

fn record(tag: &[u8; 4], word: u32, flags: u32, fields: &[u8]) -> Vec<u8> {
    [
        tag.as_slice(),
        &u32::try_from(fields.len()).unwrap().to_le_bytes(),
        &flags.to_le_bytes(),
        &word.to_le_bytes(),
        &[0; 8],
        fields,
    ]
    .concat()
}

fn group(kind: u32, word: u32, body: &[u8]) -> Vec<u8> {
    [
        b"GRUP".as_slice(),
        &u32::try_from(body.len() + 24).unwrap().to_le_bytes(),
        &word.to_le_bytes(),
        &kind.to_le_bytes(),
        &[0; 8],
        body,
    ]
    .concat()
}

fn plugin(root: &Path, name: &str, masters: &[&str], body: &[u8]) {
    let mut header = sub(
        b"HEDR",
        &[1.71f32.to_le_bytes(), [0; 4], 0x1000u32.to_le_bytes()].concat(),
    );
    for master in masters {
        header.extend(sub(b"MAST", format!("{master}\0").as_bytes()));
        header.extend(sub(b"DATA", &[0; 8]));
    }
    fs::write(
        root.join(name),
        [
            record(b"TES4", 0, u32::from(name.ends_with(".esm")), &header),
            body.to_vec(),
        ]
        .concat(),
    )
    .unwrap();
}

fn heights(offset: f32) -> Vec<u8> {
    let mut bytes = offset.to_le_bytes().to_vec();
    bytes.extend(vec![0; 33 * 33 + 3]);
    bytes
}

fn assignment(tag: &[u8; 4], word: u32, slot: u16) -> Vec<u8> {
    let mut bytes = word.to_le_bytes().to_vec();
    bytes.extend([0, 0]);
    bytes.extend(slot.to_le_bytes());
    sub(tag, &bytes)
}

fn weight(vertex: u16, opacity: f32) -> Vec<u8> {
    sub(
        b"VTXT",
        &[
            vertex.to_le_bytes().as_slice(),
            &[0; 2],
            &opacity.to_le_bytes(),
        ]
        .concat(),
    )
}

fn land(word: u32, offset: f32) -> Vec<u8> {
    record(b"LAND", word, 0, &sub(b"VHGT", &heights(offset)))
}

fn cell(word: u32, x: i32, y: i32, land_word: u32, offset: f32) -> Vec<u8> {
    [
        record(
            b"CELL",
            word,
            0,
            &[
                sub(b"EDID", format!("Cell{word:X}\0").as_bytes()),
                sub(b"DATA", &[0; 2]),
                sub(
                    b"XCLC",
                    &[x.to_le_bytes(), y.to_le_bytes(), [0; 4]].concat(),
                ),
            ]
            .concat(),
        ),
        group(6, word, &land(land_word, offset)),
    ]
    .concat()
}

/// Source priority Zulu/Alpha/Patch, allocation Alpha/Patch/Zulu, MAST Alpha/Zulu.
pub async fn publish(reader: RecordReader, damaged: bool, lod: bool) -> Publication {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path().join("Data");
    let output = directory.path().join("assets");
    fs::create_dir_all(data.join("textures/landscape")).unwrap();
    let texture = dds::generate(
        &dds::Spec::new(dds::Format::Bc1Unorm, 8, 8).with_mip_levels(4),
        &mut Rng::new(0x92),
    )
    .unwrap();
    fs::write(data.join("textures/landscape/healthy.dds"), texture).unwrap();
    plugin(
        &data,
        "Zulu.esm",
        &[],
        &[
            record(b"WRLD", 0x600, 0, &sub(b"EDID", b"WorldZulu\0")),
            group(
                1,
                0x600,
                &[
                    cell(0x700, 7, -3, 0x800, 1.5),
                    cell(0x701, 8, -3, 0x801, 2.5),
                ]
                .concat(),
            ),
        ]
        .concat(),
    );
    plugin(
        &data,
        "Alpha.esm",
        &[],
        &[
            record(
                b"TXST",
                0x844,
                0,
                &[
                    sub(b"EDID", b"HealthyTextureSet\0"),
                    sub(b"TX00", b"textures\\landscape\\healthy.dds\0"),
                ]
                .concat(),
            ),
            record(
                b"LTEX",
                0x855,
                0,
                &[
                    sub(b"EDID", b"HealthyLandTexture\0"),
                    sub(b"TNAM", &0x844u32.to_le_bytes()),
                ]
                .concat(),
            ),
        ]
        .concat(),
    );
    let mut fields = [
        sub(b"VHGT", &heights(3.5)),
        assignment(b"BTXT", 0x855, 0),
        sub(
            b"ZZZZ",
            &[0x855u32.to_le_bytes(), [0x31, 0x72, 0xb4, 0xe9]].concat(),
        ),
        assignment(b"ATXT", 0x855, 2),
        weight(5, 0.25),
        assignment(b"ATXT", 0x855, 2),
        weight(9, 0.75),
        assignment(b"ATXT", 0, 2),
        weight(11, 0.125),
    ]
    .concat();
    if damaged {
        fields.extend(assignment(b"ATXT", 0x999, 4));
        fields.extend(weight(17, 0.5));
    }
    fields.extend(sub(b"VNML", &[1, 2, 126].repeat(33 * 33)));
    let mut patch = group(
        1,
        0x0100_0600,
        &group(6, 0x0100_0700, &record(b"LAND", 0x0100_0800, 0, &fields)),
    );
    if damaged {
        patch.extend(group(
            1,
            0x0100_0600,
            &group(
                6,
                0x0100_0701,
                &[
                    record(b"LAND", 0x0100_0801, 0, &sub(b"VHGT", &[1, 2, 3])),
                    record(b"LAND", 0x0200_0802, 0, &sub(b"BTXT", &[0; 7])),
                    // The frame is bounded; this declared subrecord length overruns only its own record.
                    record(b"LAND", 0x0200_0803, 0, b"VHGT\x20\x00\x01\x02\x03"),
                ]
                .concat(),
            ),
        ));
    }
    patch.extend(record(
        b"STAT",
        0x0200_0810,
        0,
        &sub(b"EDID", b"HealthyAfterDamage\0"),
    ));
    plugin(&data, "Patch.esp", &["Alpha.esm", "Zulu.esm"], &patch);
    let list = directory.path().join("plugins.txt");
    fs::write(&list, "*Zulu.esm\n*Alpha.esm\n*Patch.esp\n").unwrap();
    let mut config = PipelineConfig::new(&data, &output);
    config.plugins_file = Some(list);
    config.record_reader = reader;
    config.no_lod = !lod;
    config.merged_terrain = true;
    config.cpu_jobs = 2;
    config.texture_encoder = TextureEncoder::Cpu;
    config.lod_texture_encoder = TextureEncoder::Cpu;
    if lod {
        config.lod_origins.insert("WorldZulu".into(), [0, 0]);
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel(128);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let report = AssetPipeline::run_async(config, tx).await.unwrap();
    drain.await.unwrap();
    Publication {
        _directory: directory,
        output,
        report,
    }
}
