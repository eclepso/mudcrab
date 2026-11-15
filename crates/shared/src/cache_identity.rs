//! Authenticated FormKey payloads shared by cache producers and runtime readers.
use crate::{
    CELL_CACHE_VERSION, CellCache,
    form_key::{FormKey, NamedFormKey},
    pack_identity::PackIdentity,
    terrain_mesh::{
        LandTextureArray, LandTextureEntry, LandTextureSettings, LandTextureTable,
        TERRAIN_MESH_FORMAT_VERSION, TerrainChunkFile,
    },
};
use color_eyre::{Result, eyre::ensure};
use rkyv::rancor::Error;
use serde::{Deserialize, Serialize};

/// Independent discriminators prevent a preserved cache being loaded as runtime terrain.
pub const RUNTIME_CELL_CACHE: u32 = 0x4345_4c52;
pub const PRESERVED_CELL_CACHE: u32 = 0x4345_4c50;
pub const TERRAIN_CHUNK: u32 = 0x5445_5252;

/// Check a non-null stored key against the authenticated allocation.
pub fn required(identity: &PackIdentity, packed: u64) -> Result<FormKey> {
    let key = FormKey::from_packed(packed)?;
    identity.required_key(key.to_sql_i64())
}

/// Zero is the explicit default texture/null slot, never a plugin-zero identity.
pub fn optional(identity: &PackIdentity, packed: u64) -> Result<Option<FormKey>> {
    if packed == 0 {
        Ok(None)
    } else {
        required(identity, packed).map(Some)
    }
}

/// Preserve the established full/light owner ordering independently of allocated integers.
pub fn owner_order(identity: &PackIdentity, key: FormKey) -> (u8, usize, u32) {
    let entry = identity
        .table()
        .entry(key.plugin())
        .expect("validated owner key");
    (
        u8::from(entry.facts.flags.light),
        identity
            .order()
            .iter()
            .position(|owner| *owner == key.plugin())
            .unwrap_or(usize::MAX),
        key.local(),
    )
}

/// Validate cell payload identities before archiving them.
pub fn validate_cell_cache(identity: &PackIdentity, cache: &CellCache) -> Result<()> {
    ensure!(
        cache.version == CELL_CACHE_VERSION,
        "cell cache is out of date; convert again"
    );
    for cell in &cache.cells {
        required(identity, cell.cell_id)?;
        if let Some(water) = cell.water_type_form_id {
            required(identity, water)?;
        }
        for layer in &cell.layers {
            optional(identity, layer.texture_form_id)?;
        }
    }
    Ok(())
}

/// Write the new cache generation with its independently readable envelope.
pub fn encode_cell_cache(
    identity: &PackIdentity,
    cache: &CellCache,
    preserved: bool,
) -> Result<Vec<u8>> {
    validate_cell_cache(identity, cache)?;
    let payload = rkyv::to_bytes::<Error>(cache)?;
    Ok(identity.envelope(
        if preserved {
            PRESERVED_CELL_CACHE
        } else {
            RUNTIME_CELL_CACHE
        },
        CELL_CACHE_VERSION,
        &payload,
    ))
}

/// Authenticate format/table/bytes before interpreting the changed rkyv root.
pub fn cell_cache_payload<'a>(
    identity: &PackIdentity,
    bytes: &'a [u8],
    preserved: bool,
) -> Result<&'a [u8]> {
    let payload = identity.payload(
        bytes,
        if preserved {
            PRESERVED_CELL_CACHE
        } else {
            RUNTIME_CELL_CACHE
        },
        CELL_CACHE_VERSION,
    )?;
    let archived = rkyv::access::<crate::ArchivedCellCache, Error>(payload)?;
    ensure!(
        archived.version == CELL_CACHE_VERSION,
        "cell cache is out of date; convert again"
    );
    for cell in &archived.cells {
        required(identity, cell.cell_id.into())?;
        if let Some(water) = cell.water_type_form_id.as_ref() {
            required(identity, (*water).into())?;
        }
        for layer in &cell.layers {
            optional(identity, layer.texture_form_id.into())?;
        }
    }
    Ok(payload)
}

/// Validate every semantic key in a merged terrain chunk.
pub fn validate_terrain_chunk(identity: &PackIdentity, chunk: &TerrainChunkFile) -> Result<()> {
    ensure!(
        chunk.format_version == TERRAIN_MESH_FORMAT_VERSION,
        "terrain chunk is out of date; convert again"
    );
    required(identity, chunk.worldspace_id)?;
    for cell in &chunk.cells {
        required(identity, cell.cell_id)?;
        for &slot in &cell.quadrant_slots {
            optional(identity, slot)?;
        }
    }
    Ok(())
}

/// Archive a terrain chunk only after authenticating its owner keys.
pub fn encode_terrain_chunk(identity: &PackIdentity, chunk: &TerrainChunkFile) -> Result<Vec<u8>> {
    validate_terrain_chunk(identity, chunk)?;
    let payload = rkyv::to_bytes::<Error>(chunk)?;
    Ok(identity.envelope(TERRAIN_CHUNK, TERRAIN_MESH_FORMAT_VERSION, &payload))
}

/// Authenticate a chunk before the runtime or reuse path accesses an archived root.
pub fn terrain_chunk_payload<'a>(identity: &PackIdentity, bytes: &'a [u8]) -> Result<&'a [u8]> {
    let payload = identity.payload(bytes, TERRAIN_CHUNK, TERRAIN_MESH_FORMAT_VERSION)?;
    let archived = rkyv::access::<crate::terrain_mesh::ArchivedTerrainChunkFile, Error>(payload)?;
    ensure!(
        archived.format_version == TERRAIN_MESH_FORMAT_VERSION,
        "terrain chunk is out of date; convert again"
    );
    required(identity, archived.worldspace_id.into())?;
    for cell in &archived.cells {
        required(identity, cell.cell_id.into())?;
        for slot in &cell.quadrant_slots {
            optional(identity, (*slot).into())?;
        }
    }
    Ok(payload)
}

#[derive(Serialize, Deserialize)]
struct PortableEntry {
    id: u16,
    ltex_form_id: NamedFormKey,
    diffuse: crate::terrain_mesh::ArrayLayer,
    normal: Option<crate::terrain_mesh::ArrayLayer>,
}

#[derive(Serialize, Deserialize)]
struct PortableTable {
    format_version: u32,
    plugin_table_digest: [u8; 32],
    max_layers_per_array: u32,
    arrays: Vec<LandTextureArray>,
    entries: Vec<PortableEntry>,
    settings: Option<LandTextureSettings>,
}

impl LandTextureTable {
    /// Emit portable LTEX names and null defaults rather than pack-local JSON numbers.
    pub fn to_portable_json(&self, identity: &PackIdentity) -> Result<Vec<u8>> {
        ensure!(
            self.format_version == TERRAIN_MESH_FORMAT_VERSION,
            "land texture table is out of date; convert again"
        );
        let entries = self
            .entries
            .iter()
            .map(|entry| {
                let named = match optional(identity, entry.ltex_form_id)? {
                    Some(key) => identity.table().named(key)?,
                    None => NamedFormKey::Null,
                };
                Ok(PortableEntry {
                    id: entry.id,
                    ltex_form_id: named,
                    diffuse: entry.diffuse,
                    normal: entry.normal,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(serde_json::to_vec_pretty(&PortableTable {
            format_version: TERRAIN_MESH_FORMAT_VERSION,
            plugin_table_digest: identity.digest(),
            max_layers_per_array: self.max_layers_per_array,
            arrays: self.arrays.clone(),
            entries,
            settings: self.settings.clone(),
        })?)
    }

    /// Restore names through this pack's authenticated allocation before exposing numeric keys.
    pub fn from_portable_json(bytes: &[u8], identity: &PackIdentity) -> Result<Self> {
        let table: PortableTable = serde_json::from_slice(bytes)?;
        ensure!(
            table.format_version == TERRAIN_MESH_FORMAT_VERSION,
            "land texture table is out of date; convert again"
        );
        ensure!(
            table.plugin_table_digest == identity.digest(),
            "land texture table belongs to another plugin allocation; convert again"
        );
        let entries = table
            .entries
            .into_iter()
            .map(|entry| {
                let key = identity.table().intern(&entry.ltex_form_id)?;
                if let Some(key) = key {
                    required(identity, key.to_packed())?;
                }
                Ok(LandTextureEntry {
                    id: entry.id,
                    ltex_form_id: key.map_or(0, FormKey::to_packed),
                    diffuse: entry.diffuse,
                    normal: entry.normal,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let restored = Self {
            format_version: table.format_version,
            max_layers_per_array: table.max_layers_per_array,
            arrays: table.arrays,
            entries,
            settings: table.settings,
        };
        restored.validate()?;
        Ok(restored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CachedLand, TerrainLayer,
        plugin_table::{PluginFacts, PluginTable},
        terrain_mesh::{ArrayLayer, LandTextureFormat, LandTextureUsage},
    };

    /// Allocation order is alphabetical while supplying order is deliberately reversed.
    fn identity(proof: u8) -> PackIdentity {
        let table = PluginTable::fresh(vec![
            (
                "Zulu.esm".into(),
                PluginFacts {
                    content_hash: Some([proof; 32]),
                    ..Default::default()
                },
            ),
            (
                "Alpha.esm".into(),
                PluginFacts {
                    content_hash: Some([0x37; 32]),
                    ..Default::default()
                },
            ),
        ])
        .unwrap();
        let order = vec![
            table.id("Zulu.esm").unwrap(),
            table.id("Alpha.esm").unwrap(),
        ];
        PackIdentity::new(table, order).unwrap()
    }

    fn aligned(bytes: &[u8]) -> rkyv::util::AlignedVec<16> {
        let mut result = rkyv::util::AlignedVec::with_capacity(bytes.len());
        result.extend_from_slice(bytes);
        result
    }

    fn cell_cache() -> CellCache {
        CellCache {
            version: CELL_CACHE_VERSION,
            cells: vec![CachedLand {
                cell_id: 0x0200_1357,
                width: 1,
                height: 1,
                heights: vec![21.5],
                normals: vec![0, 0, 127],
                vertex_colors: vec![],
                water_height: Some(11.25),
                water_type_form_id: Some(0x0100_2468),
                layers: vec![TerrainLayer {
                    texture_form_id: 0x0200_369a,
                    quadrant: 0,
                    layer: 0,
                    is_base: true,
                    weights: vec![],
                }],
            }],
        }
    }

    #[test]
    fn cell_envelope_roundtrip_keeps_owner_allocation_independent_of_priority() {
        let identity = identity(0x91);
        let bytes = aligned(&encode_cell_cache(&identity, &cell_cache(), false).unwrap());
        let payload = cell_cache_payload(&identity, &bytes, false).unwrap();
        let archived = rkyv::access::<crate::ArchivedCellCache, Error>(payload).unwrap();
        assert_eq!(u64::from(archived.cells[0].cell_id), 0x0200_1357);
        assert_eq!(
            identity
                .table()
                .named(required(&identity, archived.cells[0].cell_id.into()).unwrap())
                .unwrap()
                .to_string(),
            "001357:Zulu.esm"
        );
        assert_eq!(
            u64::from(archived.cells[0].layers[0].texture_form_id),
            0x0200_369a
        );
        assert_eq!(
            u64::from(*archived.cells[0].water_type_form_id.as_ref().unwrap()),
            0x0100_2468
        );
    }

    #[test]
    fn cell_envelope_refuses_unbound_old_wrong_kind_and_changed_content() {
        let identity = identity(0x91);
        let cache = cell_cache();
        let old = rkyv::to_bytes::<Error>(&cache).unwrap();
        assert!(cell_cache_payload(&identity, &old, false).is_err());
        let bytes = aligned(&encode_cell_cache(&identity, &cache, false).unwrap());
        assert!(cell_cache_payload(&identity, &bytes, true).is_err());
        assert!(cell_cache_payload(&self::identity(0x92), &bytes, false).is_err());
        assert!(cell_cache_payload(&identity, &bytes[..95], false).is_err());
        let mut tampered = bytes.clone();
        *tampered.last_mut().unwrap() ^= 0x04;
        assert!(cell_cache_payload(&identity, &tampered, false).is_err());
        let mut unknown = cell_cache();
        unknown.cells[0].cell_id = 0x0300_1357;
        assert!(encode_cell_cache(&identity, &unknown, false).is_err());
    }

    #[test]
    fn cache_roundtrip_preserves_keys_wider_than_runtime_ids() {
        let table = PluginTable::fresh(
            (0..300)
                .map(|index| {
                    (
                        format!("Large{index:03}.esm"),
                        PluginFacts {
                            content_hash: Some([0x5b; 32]),
                            ..Default::default()
                        },
                    )
                })
                .collect(),
        )
        .unwrap();
        let order = table.entries().iter().map(|entry| entry.id).collect();
        let identity = PackIdentity::new(table, order).unwrap();
        let mut source = cell_cache();
        source.cells[0].cell_id = 0x0000_0001_2c00_abcd;
        let bytes = aligned(&encode_cell_cache(&identity, &source, false).unwrap());
        let payload = cell_cache_payload(&identity, &bytes, false).unwrap();
        let archived = rkyv::access::<crate::ArchivedCellCache, Error>(payload).unwrap();
        assert_eq!(u64::from(archived.cells[0].cell_id), 0x0000_0001_2c00_abcd);
        assert_eq!(
            identity
                .table()
                .named(required(&identity, archived.cells[0].cell_id.into()).unwrap())
                .unwrap()
                .to_string(),
            "00ABCD:Large299.esm"
        );
    }

    #[test]
    fn terrain_envelope_refuses_unbound_and_cross_pack_chunks() {
        let identity = identity(0x91);
        let chunk = TerrainChunkFile {
            format_version: TERRAIN_MESH_FORMAT_VERSION,
            worldspace_id: 0x0200_005a,
            cells: vec![],
        };
        let old = rkyv::to_bytes::<Error>(&chunk).unwrap();
        assert!(terrain_chunk_payload(&identity, &old).is_err());
        let bytes = aligned(&encode_terrain_chunk(&identity, &chunk).unwrap());
        let payload = terrain_chunk_payload(&identity, &bytes).unwrap();
        let archived =
            rkyv::access::<crate::terrain_mesh::ArchivedTerrainChunkFile, Error>(payload).unwrap();
        assert_eq!(u64::from(archived.worldspace_id), 0x0200_005a);
        assert!(terrain_chunk_payload(&self::identity(0x92), &bytes).is_err());
    }

    #[test]
    fn portable_texture_table_uses_names_and_null_with_digest_refusal() {
        let identity = identity(0x91);
        let table = LandTextureTable {
            format_version: TERRAIN_MESH_FORMAT_VERSION,
            max_layers_per_array: 2048,
            settings: None,
            arrays: vec![LandTextureArray {
                path: "terrain/land-diffuse-bc1-1024.ktx2".into(),
                usage: LandTextureUsage::DiffuseSrgb,
                format: LandTextureFormat::Bc1,
                size: 1024,
                layers: 2,
            }],
            entries: vec![
                LandTextureEntry {
                    id: 0,
                    ltex_form_id: 0,
                    diffuse: ArrayLayer { array: 0, layer: 0 },
                    normal: None,
                },
                LandTextureEntry {
                    id: 1,
                    ltex_form_id: 0x0200_369a,
                    diffuse: ArrayLayer { array: 0, layer: 1 },
                    normal: None,
                },
            ],
        };
        let bytes = table.to_portable_json(&identity).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(json["entries"][0]["ltex_form_id"].is_null());
        assert_eq!(json["entries"][1]["ltex_form_id"], "00369A:Zulu.esm");
        assert_eq!(
            LandTextureTable::from_portable_json(&bytes, &identity).unwrap(),
            table
        );
        assert!(LandTextureTable::from_portable_json(&bytes, &self::identity(0x92)).is_err());
        assert!(
            LandTextureTable::from_portable_json(&serde_json::to_vec(&table).unwrap(), &identity)
                .is_err()
        );
    }
}
