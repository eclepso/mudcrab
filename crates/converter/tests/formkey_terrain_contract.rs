//! Paired production terrain/cache/LOD artifacts from independent native inputs.
#[path = "common/native_terrain.rs"]
mod fixture;

use converter::config::RecordReader;
use shared::{
    form_key::FormKey,
    pack_identity::PackIdentity,
    records_context::RecordContextReader,
    terrain_mesh::{LAND_TEXTURE_TABLE_PATH, LandTextureTable, terrain_chunk_path},
};

fn key(packed: u64) -> FormKey {
    FormKey::from_packed(packed).unwrap()
}

fn aligned(bytes: &[u8]) -> rkyv::util::AlignedVec<16> {
    let mut aligned = rkyv::util::AlignedVec::with_capacity(bytes.len());
    aligned.extend_from_slice(bytes);
    aligned
}

#[tokio::test]
async fn both_readers_publish_paired_terrain_with_exact_physical_link_oracles() {
    for reader in [RecordReader::Legacy, RecordReader::Inhouse] {
        let publication = fixture::publish(reader, true, false).await;
        let output = &publication.output;
        let database = rusqlite::Connection::open(output.join("skyrim_world.db")).unwrap();
        let context = RecordContextReader::new(&database).unwrap();
        let identity = context.identity();
        assert_eq!(
            identity
                .table()
                .named(key(0x0300_0700))
                .unwrap()
                .to_string(),
            "000700:Zulu.esm"
        );
        let land = context.record(key(0x0300_0800)).unwrap();
        assert_eq!(land.supplier.get(), 2);
        assert_eq!(land.source_header, 0x0100_0800);
        assert_eq!(land.source_cell, Some(0x0100_0700));
        assert_eq!(land.source_world, Some(0x0100_0600));
        for source_ordinal in [1, 3, 5] {
            let ordinal = land
                .source_ordinals
                .iter()
                .position(|source| *source == source_ordinal)
                .unwrap();
            assert_eq!(&land.subrecords[ordinal].data[..4], &0x855u32.to_le_bytes());
            assert_eq!(land.resolve_at(ordinal, 0).unwrap(), Some(key(0x0100_0855)));
        }
        let opaque:i64=database.query_row("SELECT count(*) FROM resolved_links WHERE record_key=50333696 AND source_ordinal=2",[],|row|row.get(0)).unwrap();
        assert_eq!(
            opaque, 0,
            "equal bytes in opaque ZZZZ must not create a recognized link site"
        );
        let outcomes:Vec<(usize,u32,Option<i64>,String)>=database.prepare("SELECT source_ordinal,source_word,target_key,outcome FROM resolved_links WHERE record_key=50333696 AND source_ordinal IN (5,7,9) ORDER BY source_ordinal").unwrap()
            .query_map([],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap();
        assert_eq!(
            outcomes,
            vec![
                (5, 0x855, Some(0x0100_0855), "resolved".into()),
                (7, 0, None, "null".into()),
                (9, 0x999, None, "cleared".into())
            ]
        );
        let neighbor = context.record(key(0x0300_0801)).unwrap();
        assert_eq!(neighbor.supplier.get(), 3);
        assert_eq!(neighbor.source_header, 0x801);
        assert_eq!(
            &neighbor.bytes(*b"VHGT").unwrap()[..4],
            &2.5f32.to_le_bytes()
        );

        let runtime =
            converter::esm::cell_cache::read_form_key_cell_cache(&output.join("cell_cache.rkyv"))
                .unwrap();
        let cell = runtime
            .cells
            .iter()
            .find(|cell| cell.cell_id == 0x0300_0700)
            .unwrap();
        assert_eq!(cell.heights, vec![28.0; 33 * 33]);
        assert_eq!(cell.normals, [1, 2, 126].repeat(33 * 33));
        assert_eq!(
            cell.layers
                .iter()
                .map(|layer| (layer.texture_form_id, layer.layer))
                .collect::<Vec<_>>(),
            vec![(0x0100_0855, 0), (0x0100_0855, 2), (0, 4)]
        );
        assert_eq!(
            (
                cell.layers[1].weights[0].vertex,
                cell.layers[1].weights[0].opacity
            ),
            (9, 0.75)
        );
        assert_eq!(
            (
                cell.layers[2].weights[0].vertex,
                cell.layers[2].weights[0].opacity
            ),
            (17, 0.5)
        );
        assert_eq!(
            runtime
                .cells
                .iter()
                .find(|cell| cell.cell_id == 0x0300_0701)
                .unwrap()
                .heights,
            vec![20.0; 33 * 33]
        );
        if reader == RecordReader::Inhouse {
            let preserved = converter::esm::cell_cache::read_form_key_cell_cache(
                &output.join("cell_cache_preserved.rkyv"),
            )
            .unwrap();
            let source = preserved
                .cells
                .iter()
                .find(|cell| cell.cell_id == 0x0300_0700)
                .unwrap();
            assert_eq!(source.layers[2].texture_form_id, 0x0100_0999);
            assert_eq!(
                (
                    source.layers[2].weights[0].vertex,
                    source.layers[2].weights[0].opacity
                ),
                (17, 0.5)
            );
        }

        let table_bytes = std::fs::read(output.join(LAND_TEXTURE_TABLE_PATH)).unwrap();
        let table = LandTextureTable::from_portable_json(&table_bytes, identity).unwrap();
        assert_eq!(
            table
                .entries
                .iter()
                .map(|entry| entry.ltex_form_id)
                .collect::<Vec<_>>(),
            vec![0, 0x0100_0855]
        );
        let json: serde_json::Value = serde_json::from_slice(&table_bytes).unwrap();
        assert!(json["entries"][0]["ltex_form_id"].is_null());
        assert_eq!(json["entries"][1]["ltex_form_id"], "000855:Alpha.esm");
        for (x, cell_id, height) in [(7, 0x0300_0700, 28.0), (8, 0x0300_0701, 20.0)] {
            let path = output.join(terrain_chunk_path(0x0300_0600, x, -3));
            let chunk = converter::terrain_chunks::read_terrain_chunk(&path).unwrap();
            assert_eq!(chunk.worldspace_id, 0x0300_0600);
            let cell = chunk
                .cells
                .iter()
                .find(|cell| cell.cell_id == cell_id)
                .unwrap();
            assert!(cell.heights.iter().all(|sample| *sample == height));
        }
        let other = PackIdentity::new(
            identity.table().clone(),
            identity.order().iter().copied().rev().collect(),
        )
        .unwrap();
        let cache_bytes = aligned(&std::fs::read(output.join("cell_cache.rkyv")).unwrap());
        assert!(shared::cache_identity::cell_cache_payload(&other, &cache_bytes, false).is_err());
        assert!(LandTextureTable::from_portable_json(&table_bytes, &other).is_err());
        assert!(
            converter::terrain_chunks::read_form_key_terrain_chunk(
                &output.join(terrain_chunk_path(0x0300_0600, 8, -3)),
                &other
            )
            .is_err()
        );
        assert!(publication.report.terrain.as_ref().unwrap().cells_written >= 2);
    }
}

#[tokio::test]
async fn both_readers_publish_named_lod_cells_bound_to_plugin_table() {
    for reader in [RecordReader::Legacy, RecordReader::Inhouse] {
        let publication = fixture::publish(reader, false, true).await;
        let output = &publication.output;
        let database = rusqlite::Connection::open(output.join("skyrim_world.db")).unwrap();
        let identity = PackIdentity::read_sql(&database).unwrap();
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output.join("lod-manifest.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["plugin_table_digest"], identity.digest_hex());
        let paths:Vec<String>=database.prepare("SELECT payload_path FROM lod_chunks WHERE worldspace_id=50333184 ORDER BY tier,anchor_x,anchor_y").unwrap()
            .query_map([],|row|row.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
        assert!(
            !paths.is_empty(),
            "healthy native terrain must produce actual LOD payloads: {:?}",
            publication.report.warnings
        );
        let mut names = std::collections::BTreeSet::new();
        for path in paths {
            assert!(path.starts_with("lod/0000000003000600/"));
            let bytes = std::fs::read(output.join(path)).unwrap();
            assert_eq!(&bytes[..4], b"glTF");
            let length = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
            let json: serde_json::Value = serde_json::from_slice(&bytes[20..20 + length]).unwrap();
            for node in json["nodes"].as_array().unwrap() {
                if let Some(name) = node.get("extras").and_then(|extra| extra.get("cell_id")) {
                    names.insert(
                        name.as_str()
                            .expect("external LOD identity is a name")
                            .to_owned(),
                    );
                }
            }
        }
        assert!(names.contains("000700:Zulu.esm"));
        assert!(names.contains("000701:Zulu.esm"));
    }
}
