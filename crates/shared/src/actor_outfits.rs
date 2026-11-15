//! Optional, typed worn outfits for `NPC_` bases, derived from their default outfit.
//!
//! The catalog is one sidecar file beside the world database. It holds, per `NPC_` base, the
//! armature models the default outfit (`NPC_/DOFT` -> `OTFT` -> `ARMO` -> `ARMA`) puts on the
//! actor, already matched to the actor's race and sex and already named as converted scene paths.
//! The converter resolves it once; the runtime only reads rows and never decodes records.
//! A pack without the file spawns actors exactly as before.

use crate::{form_key::NamedFormKey, pack_identity::PackIdentity};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Where the catalog lives, relative to the converted-assets root.
pub const PATH: &str = "actor-outfits.json";
/// Catalog format version. A different version is rejected as a whole.
pub const VERSION: u32 = 2;

/// Published named identities are portable; the digest binds this derived catalog to its pack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorOutfitCatalogFile {
    pub version: u32,
    pub pack_identity: String,
    pub outfits: Vec<ActorOutfitFile>,
    #[serde(default)]
    pub skipped: BTreeMap<String, usize>,
}

/// A stored outfit never exposes a pack-local integer or transient runtime handle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorOutfitFile {
    pub npc_id: NamedFormKey,
    pub outfit_id: NamedFormKey,
    pub parts: Vec<OutfitPartFile>,
}

/// Model and identity source facts for one published worn part.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutfitPartFile {
    pub armo_id: NamedFormKey,
    pub armature_id: NamedFormKey,
    pub biped_mask: u32,
    pub armature_race_id: NamedFormKey,
    pub scene: String,
}

/// Convert a required transient handle only at an explicit runtime boundary.
fn named_runtime(identity: &PackIdentity, value: u32) -> color_eyre::Result<NamedFormKey> {
    let key = identity
        .runtime_key(value)?
        .ok_or_else(|| color_eyre::eyre::eyre!("missing required outfit identity"))?;
    Ok(identity.table().named(key)?)
}

/// Authenticate a required portable key before producing a transient gameplay handle.
fn runtime_named(identity: &PackIdentity, value: &NamedFormKey) -> color_eyre::Result<u32> {
    let key = identity
        .table()
        .intern(value)?
        .ok_or_else(|| color_eyre::eyre::eyre!("missing required outfit identity"))?;
    identity.runtime_id(key)
}

impl ActorOutfitCatalogFile {
    /// Check the generation before resolving any named rows or scene paths.
    pub fn validate_for(&self, identity: &PackIdentity) -> color_eyre::Result<()> {
        color_eyre::eyre::ensure!(
            self.version == VERSION,
            "unsupported actor outfit catalog; convert again"
        );
        color_eyre::eyre::ensure!(
            self.pack_identity == identity.digest_hex(),
            "actor outfit catalog belongs to another conversion; convert again"
        );
        let mut seen = BTreeSet::new();
        for row in &self.outfits {
            let key = identity
                .table()
                .intern(&row.npc_id)?
                .ok_or_else(|| color_eyre::eyre::eyre!("outfit has no NPC identity"))?;
            color_eyre::eyre::ensure!(
                seen.insert(key) && !row.parts.is_empty(),
                "empty or duplicate actor outfit"
            );
            color_eyre::eyre::ensure!(
                identity.table().intern(&row.outfit_id)?.is_some(),
                "outfit has no source identity"
            );
            for part in &row.parts {
                color_eyre::eyre::ensure!(
                    part.biped_mask != 0
                        && crate::actor_idle_catalog::canonical_mesh_key(&part.scene, ".glb"),
                    "invalid actor outfit part"
                );
                for id in [&part.armo_id, &part.armature_id, &part.armature_race_id] {
                    color_eyre::eyre::ensure!(
                        identity.table().intern(id)?.is_some(),
                        "outfit part has no identity"
                    );
                }
            }
        }
        Ok(())
    }

    /// Preserve usable neighboring outfits while declining handles unavailable before step 4.
    pub fn to_runtime(&self, identity: &PackIdentity) -> color_eyre::Result<ActorOutfitCatalog> {
        self.validate_for(identity)?;
        let mut catalog = ActorOutfitCatalog {
            version: VERSION,
            outfits: Vec::new(),
            skipped: self.skipped.clone(),
        };
        for row in &self.outfits {
            let mapped = (|| -> color_eyre::Result<ActorOutfit> {
                let npc_id = runtime_named(identity, &row.npc_id)?;
                let outfit_id = runtime_named(identity, &row.outfit_id)?;
                let mut parts = Vec::new();
                for part in &row.parts {
                    let mapped = (|| -> color_eyre::Result<OutfitPart> {
                        Ok(OutfitPart {
                            armo_id: runtime_named(identity, &part.armo_id)?,
                            armature_id: runtime_named(identity, &part.armature_id)?,
                            armature_race_id: runtime_named(identity, &part.armature_race_id)?,
                            biped_mask: part.biped_mask,
                            scene: part.scene.clone(),
                        })
                    })();
                    match mapped {
                        Ok(part) => parts.push(part),
                        Err(_) => {
                            *catalog
                                .skipped
                                .entry("runtime_part_identity_unavailable".into())
                                .or_default() += 1
                        }
                    }
                }
                Ok(ActorOutfit {
                    npc_id,
                    outfit_id,
                    parts,
                })
            })();
            match mapped {
                Ok(row) if !row.parts.is_empty() => catalog.outfits.push(row),
                _ => {
                    *catalog
                        .skipped
                        .entry("runtime_outfit_identity_unavailable".into())
                        .or_default() += 1
                }
            }
        }
        catalog.outfits.sort_by_key(|row| row.npc_id);
        Ok(catalog)
    }
}

/// `ARMA/BOD2` biped slot bits. The slot number minus 30.
pub const BIPED_BODY: u32 = 1 << 2;
pub const BIPED_HANDS: u32 = 1 << 3;
pub const BIPED_FEET: u32 = 1 << 7;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ActorOutfitCatalog {
    pub version: u32,
    /// One row per `NPC_` base with at least one worn part, sorted by `npc_id`.
    pub outfits: Vec<ActorOutfit>,
    /// Why items did not become parts, as `reason -> item count`. Never fatal.
    #[serde(default)]
    pub skipped: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActorOutfit {
    /// The `NPC_` FormID the outfit is worn by.
    pub npc_id: u32,
    /// The `OTFT` FormID the parts came from.
    pub outfit_id: u32,
    /// Parts in outfit order, then armature order.
    pub parts: Vec<OutfitPart>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutfitPart {
    /// The worn `ARMO` FormID.
    pub armo_id: u32,
    /// The selected `ARMA` FormID.
    pub armature_id: u32,
    /// `ARMA/BOD2` biped slot mask: the body regions this part covers.
    pub biped_mask: u32,
    /// The armature's own `RNAM`: the actor's race, or `DefaultRace`.
    pub armature_race_id: u32,
    /// The converted scene, a canonical `meshes/**.glb` key.
    pub scene: String,
}

impl OutfitPart {
    pub fn covers_body(&self) -> bool {
        self.biped_mask & BIPED_BODY != 0
    }

    pub fn covers_hands(&self) -> bool {
        self.biped_mask & BIPED_HANDS != 0
    }

    pub fn covers_feet(&self) -> bool {
        self.biped_mask & BIPED_FEET != 0
    }
}

impl ActorOutfitCatalog {
    /// Serialize only named identities, bound to the authenticated producing generation.
    pub fn to_file(&self, identity: &PackIdentity) -> color_eyre::Result<ActorOutfitCatalogFile> {
        let file = ActorOutfitCatalogFile {
            version: VERSION,
            pack_identity: identity.digest_hex(),
            outfits: self
                .outfits
                .iter()
                .map(|row| {
                    Ok(ActorOutfitFile {
                        npc_id: named_runtime(identity, row.npc_id)?,
                        outfit_id: named_runtime(identity, row.outfit_id)?,
                        parts: row
                            .parts
                            .iter()
                            .map(|part| {
                                Ok(OutfitPartFile {
                                    armo_id: named_runtime(identity, part.armo_id)?,
                                    armature_id: named_runtime(identity, part.armature_id)?,
                                    armature_race_id: named_runtime(
                                        identity,
                                        part.armature_race_id,
                                    )?,
                                    biped_mask: part.biped_mask,
                                    scene: part.scene.clone(),
                                })
                            })
                            .collect::<color_eyre::Result<_>>()?,
                    })
                })
                .collect::<color_eyre::Result<_>>()?,
            skipped: self.skipped.clone(),
        };
        file.validate_for(identity)?;
        Ok(file)
    }

    /// Reject the whole catalog if a key or identity is ambiguous. An optional sidecar must not
    /// turn a malformed row into an unrelated actor's clothing.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != VERSION {
            return Err("unsupported actor outfit catalog version");
        }
        let mut seen = BTreeSet::new();
        for outfit in &self.outfits {
            if outfit.parts.is_empty() || !seen.insert(outfit.npc_id) {
                return Err("empty or duplicate actor outfit");
            }
            for part in &outfit.parts {
                if part.biped_mask == 0
                    || !crate::actor_idle_catalog::canonical_mesh_key(&part.scene, ".glb")
                {
                    return Err("invalid actor outfit part");
                }
            }
        }
        Ok(())
    }

    pub fn find(&self, npc_id: u32) -> Option<&ActorOutfit> {
        self.outfits
            .binary_search_by_key(&npc_id, |outfit| outfit.npc_id)
            .ok()
            .map(|index| &self.outfits[index])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(mask: u32, scene: &str) -> OutfitPart {
        OutfitPart {
            armo_id: 1,
            armature_id: 2,
            biped_mask: mask,
            armature_race_id: 25,
            scene: scene.into(),
        }
    }

    fn catalog(parts: Vec<OutfitPart>) -> ActorOutfitCatalog {
        ActorOutfitCatalog {
            version: VERSION,
            outfits: vec![ActorOutfit {
                npc_id: 7,
                outfit_id: 9,
                parts,
            }],
            skipped: BTreeMap::new(),
        }
    }

    #[test]
    fn a_well_formed_catalog_validates_and_is_found_by_npc() {
        let catalog = catalog(vec![part(BIPED_BODY, "meshes/clothes/a/torso_1.glb")]);
        assert_eq!(catalog.validate(), Ok(()));
        assert!(catalog.find(7).is_some());
        assert!(catalog.find(8).is_none());
    }

    #[test]
    fn a_malformed_part_rejects_the_catalog() {
        for bad in [
            part(0, "meshes/a.glb"),
            part(BIPED_BODY, "Meshes/A.glb"),
            part(BIPED_BODY, "meshes/../a.glb"),
            part(BIPED_BODY, "meshes/a.nif"),
        ] {
            assert!(catalog(vec![bad]).validate().is_err());
        }
        let mut wrong_version = catalog(vec![part(BIPED_BODY, "meshes/a.glb")]);
        wrong_version.version = VERSION + 1;
        assert!(wrong_version.validate().is_err());
        let mut duplicate = catalog(vec![part(BIPED_BODY, "meshes/a.glb")]);
        duplicate.outfits.push(duplicate.outfits[0].clone());
        assert!(duplicate.validate().is_err());
    }

    #[test]
    fn the_mask_names_the_regions_a_part_covers() {
        let part = part(BIPED_BODY | BIPED_FEET, "meshes/a.glb");
        assert!(part.covers_body() && part.covers_feet() && !part.covers_hands());
    }
}
