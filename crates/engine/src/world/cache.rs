use bevy::prelude::Resource;
use color_eyre::{Result, eyre::WrapErr};
use memmap2::Mmap;
use rkyv::rancor::Error;
use shared::{form_key::FormKey, pack_identity::PackIdentity};
use std::{
    collections::HashMap,
    fs::File,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone)]
pub struct TerrainSnapshot {
    pub cell_id: u32,
    pub width: u16,
    pub height: u16,
    pub heights: Vec<f32>,
    pub normals: Vec<i8>,
    pub vertex_colors: Vec<u8>,
    pub layers: Vec<TerrainLayerSnapshot>,
    pub water_height: Option<f32>,
    pub water_type_form_id: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct TerrainLayerSnapshot {
    pub texture_form_id: u32,
    pub quadrant: u8,
    pub layer: u16,
    pub is_base: bool,
    pub weights: Vec<(u16, f32)>,
}

#[derive(Resource)]
pub struct CellCache {
    mmap: Mmap,
    index: HashMap<u64, usize>,
    identity: PackIdentity,
    warned_runtime: AtomicBool,
}

impl CellCache {
    /// Authenticate the adjacent world's plugin allocation before interpreting cache bytes.
    pub fn open(path: &Path) -> Result<Self> {
        let database = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("skyrim_world.db");
        shared::world_assets::validate_world_database(&database)?;
        let connection = rusqlite::Connection::open_with_flags(
            &database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .wrap_err("cell cache has no authenticated world context; convert again")?;
        let identity = PackIdentity::read_sql(&connection)?;
        Self::open_with_identity(path, identity)
    }

    /// Open a cache using an already authenticated immutable allocation.
    pub fn open_with_identity(path: &Path, identity: PackIdentity) -> Result<Self> {
        let file =
            File::open(path).wrap_err_with(|| format!("failed to open {}", path.display()))?;
        // SAFETY: the runtime holds the asset lock and never mutates or remaps this read-only file.
        let mmap = unsafe { Mmap::map(&file) }.wrap_err("failed to map cell cache")?;
        let payload = shared::cache_identity::cell_cache_payload(&identity, &mmap, false)?;
        let archived = rkyv::access::<shared::ArchivedCellCache, Error>(payload)?;
        let index = archived
            .cells
            .iter()
            .enumerate()
            .map(|(index, cell)| (u64::from(cell.cell_id), index))
            .collect();
        Ok(Self {
            mmap,
            index,
            identity,
            warned_runtime: AtomicBool::new(false),
        })
    }

    /// Decline unrepresentable runtime requests once with a bounded explanatory diagnostic.
    pub fn terrain(&self, cell_id: u32) -> Option<TerrainSnapshot> {
        match self.terrain_checked(cell_id) {
            Ok(terrain) => terrain,
            Err(error) => {
                if !self.warned_runtime.swap(true, Ordering::Relaxed) {
                    bevy::log::warn!("cell cache runtime request declined: {error}");
                }
                None
            }
        }
    }

    /// Preserve checked failure at the temporary u32 runtime boundary; stored keys remain u64.
    pub fn terrain_checked(&self, cell_id: u32) -> Result<Option<TerrainSnapshot>> {
        let Some(key) = self.identity.runtime_key(cell_id)? else {
            return Ok(None);
        };
        let Some(index) = self.index.get(&key.to_packed()) else {
            return Ok(None);
        };
        // SAFETY: `open` already validated these exact bytes with `rkyv::access`
        // (which runs bytecheck over the whole `ArchivedCellCache`) before this
        // `CellCache` was constructed. `self.mmap` is never remapped or mutated
        // after `open` returns, so the bytes still describe a valid
        // `ArchivedCellCache` after the authenticated fixed envelope, and re-validating on
        // every lookup (this is called once per committed cell) would be wasted
        // work.
        let archived = unsafe {
            rkyv::access_unchecked::<shared::ArchivedCellCache>(
                &self.mmap[shared::pack_identity::IDENTITY_ENVELOPE_SIZE..],
            )
        };
        let cell = &archived.cells[*index];
        Ok(Some(TerrainSnapshot {
            cell_id,
            width: cell.width.into(),
            height: cell.height.into(),
            heights: cell.heights.iter().copied().map(Into::into).collect(),
            normals: cell.normals.iter().copied().collect(),
            vertex_colors: cell.vertex_colors.iter().copied().collect(),
            layers: cell
                .layers
                .iter()
                .map(|layer| -> Result<TerrainLayerSnapshot> {
                    Ok(TerrainLayerSnapshot {
                        texture_form_id: match shared::cache_identity::optional(
                            &self.identity,
                            layer.texture_form_id.into(),
                        )? {
                            Some(key) => self.identity.runtime_id(key)?,
                            None => 0,
                        },
                        quadrant: layer.quadrant,
                        layer: layer.layer.into(),
                        is_base: layer.is_base,
                        weights: layer
                            .weights
                            .iter()
                            .map(|weight| (weight.vertex.into(), weight.opacity.into()))
                            .collect(),
                    })
                })
                .collect::<Result<Vec<_>>>()?,
            water_height: cell.water_height.as_ref().copied().map(Into::into),
            water_type_form_id: cell
                .water_type_form_id
                .as_ref()
                .map(|key| {
                    self.identity
                        .runtime_id(FormKey::from_packed((*key).into())?)
                })
                .transpose()?,
        }))
    }

    /// Whether the cache holds a LAND for `cell_id`, without reading it.
    pub fn has_terrain(&self, cell_id: u32) -> bool {
        self.identity
            .runtime_key(cell_id)
            .ok()
            .flatten()
            .is_some_and(|key| self.index.contains_key(&key.to_packed()))
    }

    /// Height of the cell's centre sample (the one the start position and `coc`/`coe` stand on),
    /// or `None` for a cell with no terrain in the cache.
    pub fn centre_height(&self, cell_id: u32) -> Option<f32> {
        let terrain = self.terrain(cell_id)?;
        let width = usize::from(terrain.width);
        let height = usize::from(terrain.height);
        let index = (height / 2)
            .checked_mul(width)
            .and_then(|row| row.checked_add(width / 2))?;
        terrain.heights.get(index).copied()
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seed a generated pack allocation next to a generated cache.
    fn fixture_identity(path: &Path) -> PackIdentity {
        let table = shared::plugin_table::PluginTable::fresh(vec![(
            "Fixture.esm".into(),
            shared::plugin_table::PluginFacts {
                content_hash: Some([0x72; 32]),
                ..Default::default()
            },
        )])
        .unwrap();
        let owner = table.id("Fixture.esm").unwrap();
        let identity = PackIdentity::new(table, vec![owner]).unwrap();
        let mut connection =
            rusqlite::Connection::open(path.parent().unwrap().join("skyrim_world.db")).unwrap();
        connection
            .execute_batch("CREATE TABLE IF NOT EXISTS schema_info(version INTEGER)")
            .unwrap();
        connection
            .execute(
                "INSERT INTO schema_info SELECT ?1 WHERE NOT EXISTS(SELECT 1 FROM schema_info)",
                [shared::WORLD_DATABASE_SCHEMA_VERSION],
            )
            .unwrap();
        let tx = connection.transaction().unwrap();
        identity.write_sql(&tx).unwrap();
        tx.commit().unwrap();
        identity
    }

    /// Runtime fixtures explicitly bind native synthetic locals to a pack allocation.
    fn write_fixture(path: &Path, mut source: shared::CellCache) {
        let identity = fixture_identity(path);
        for cell in &mut source.cells {
            cell.cell_id |= 0x0100_0000;
            cell.water_type_form_id = cell.water_type_form_id.map(|id| id | 0x0100_0000);
            for layer in &mut cell.layers {
                if layer.texture_form_id != 0 {
                    layer.texture_form_id |= 0x0100_0000;
                }
            }
        }
        std::fs::write(
            path,
            shared::cache_identity::encode_cell_cache(&identity, &source, false).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn centre_height_reads_the_centre_sample_and_is_none_without_terrain() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cell_cache.rkyv");
        let land = |cell_id, side: u16, heights: Vec<f32>| shared::CachedLand {
            cell_id,
            width: side,
            height: side,
            heights,
            normals: vec![0; usize::from(side) * usize::from(side) * 3],
            vertex_colors: vec![255; usize::from(side) * usize::from(side) * 3],
            layers: vec![],
            water_height: None,
            water_type_form_id: None,
        };
        let source = shared::CellCache {
            version: shared::CELL_CACHE_VERSION,
            cells: vec![
                land(1, 3, vec![0.0, 1.0, 2.0, 3.0, 40.0, 5.0, 6.0, 7.0, 8.0]),
                land(2, 2, vec![1.0, 2.0, 3.0, 4.0]),
            ],
        };
        write_fixture(&path, source);
        let cache = CellCache::open(&path).unwrap();
        assert_eq!(cache.centre_height(1), Some(40.0));
        // An even-sized grid's centre is index (height / 2) * width + width / 2.
        assert_eq!(cache.centre_height(2), Some(4.0));
        assert_eq!(cache.centre_height(99), None);
    }

    #[test]
    fn maps_and_reads_versioned_terrain() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cell_cache.rkyv");
        let source = shared::CellCache {
            version: shared::CELL_CACHE_VERSION,
            cells: vec![shared::CachedLand {
                cell_id: 42,
                width: 2,
                height: 2,
                heights: vec![1.0, 2.0, 3.0, 4.0],
                normals: vec![0; 12],
                vertex_colors: vec![255; 12],
                layers: vec![],
                water_height: Some(8.0),
                water_type_form_id: Some(7),
            }],
        };
        write_fixture(&path, source);

        let cache = CellCache::open(&path).unwrap();
        let terrain = cache.terrain(42).unwrap();
        assert_eq!(terrain.heights, [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(terrain.water_height, Some(8.0));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn repeated_terrain_lookups_return_the_right_cells() {
        // `terrain` used to re-validate the whole mapped file (`rkyv::access`)
        // on every call; it now trusts the one-time validation done in `open`
        // and reads with `rkyv::access_unchecked` instead. Multiple cells and
        // repeated, out-of-order lookups exercise that the unchecked access
        // still indexes the right cell every time, not just once.
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cell_cache.rkyv");
        let source = shared::CellCache {
            version: shared::CELL_CACHE_VERSION,
            cells: vec![
                shared::CachedLand {
                    cell_id: 42,
                    width: 2,
                    height: 2,
                    heights: vec![1.0, 2.0, 3.0, 4.0],
                    normals: vec![0; 12],
                    vertex_colors: vec![255; 12],
                    layers: vec![],
                    water_height: Some(8.0),
                    water_type_form_id: Some(7),
                },
                shared::CachedLand {
                    cell_id: 99,
                    width: 2,
                    height: 2,
                    heights: vec![10.0, 20.0, 30.0, 40.0],
                    normals: vec![0; 12],
                    vertex_colors: vec![255; 12],
                    layers: vec![],
                    water_height: None,
                    water_type_form_id: None,
                },
            ],
        };
        write_fixture(&path, source);

        let cache = CellCache::open(&path).unwrap();
        assert_eq!(cache.terrain(99).unwrap().heights, [10.0, 20.0, 30.0, 40.0]);
        assert_eq!(cache.terrain(42).unwrap().heights, [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(cache.terrain(42).unwrap().water_height, Some(8.0));
        assert!(cache.terrain(7).is_none());
        assert_eq!(cache.terrain(99).unwrap().heights, [10.0, 20.0, 30.0, 40.0]);
    }

    #[test]
    fn rejects_previous_cache_version() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("old.rkyv");
        fixture_identity(&path);
        let bytes = rkyv::to_bytes::<Error>(&shared::CellCache {
            version: shared::CELL_CACHE_VERSION - 1,
            cells: vec![],
        })
        .unwrap();
        std::fs::write(&path, bytes).unwrap();
        assert!(CellCache::open(&path).is_err());
    }

    #[test]
    fn rejects_truncated_cache() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("truncated.rkyv");
        fixture_identity(&path);
        std::fs::write(&path, [0_u8; 7]).unwrap();
        assert!(CellCache::open(&path).is_err());
    }

    /// A valid old archived root must be refused before interpreting its field layout.
    #[test]
    fn old_rkyv_is_rejected_before_layout_access() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cell_cache.rkyv");
        fixture_identity(&path);
        let source = shared::CellCache {
            version: shared::CELL_CACHE_VERSION,
            cells: vec![],
        };
        std::fs::write(&path, rkyv::to_bytes::<Error>(&source).unwrap()).unwrap();
        let error = CellCache::open(&path)
            .err()
            .expect("unenveloped cache was accepted");
        assert!(error.to_string().contains("converted again"), "{error}");
    }

    #[test]
    fn old_world_stamp_refuses_even_a_current_bound_cache() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cell_cache.rkyv");
        write_fixture(
            &path,
            shared::CellCache {
                version: shared::CELL_CACHE_VERSION,
                cells: vec![],
            },
        );
        let database =
            rusqlite::Connection::open(directory.path().join("skyrim_world.db")).unwrap();
        database
            .execute(
                "UPDATE schema_info SET version=?1",
                [shared::WORLD_DATABASE_SCHEMA_VERSION - 1],
            )
            .unwrap();
        let error = CellCache::open(&path)
            .err()
            .expect("old world must be converted again");
        assert!(format!("{error:#}").contains("convert again"), "{error:#}");
    }
}
