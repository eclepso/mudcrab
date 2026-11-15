//! Paired named catalog codecs authenticate their pack before resolving transient handles.
use shared::{
    actor_idle_catalog::{ActorIdleBindingFile, ActorIdleCatalogFile},
    actor_outfits::{ActorOutfitCatalogFile, ActorOutfitFile, OutfitPartFile},
    form_key::NamedFormKey,
    pack_identity::PackIdentity,
    plugin_table::{PluginFacts, PluginTable},
};
use std::collections::BTreeMap;

fn identity(count: usize, hash: u8) -> PackIdentity {
    let names: Vec<_> = (0..count)
        .map(|index| format!("Owner{index:03}.esm"))
        .collect();
    let table = PluginTable::fresh(
        names
            .iter()
            .map(|name| {
                (
                    name.clone(),
                    PluginFacts {
                        content_hash: Some([hash; 32]),
                        ..Default::default()
                    },
                )
            })
            .collect(),
    )
    .unwrap();
    let order = names.iter().map(|name| table.id(name).unwrap()).collect();
    PackIdentity::new(table, order).unwrap()
}
fn name(owner: usize, local: u32) -> NamedFormKey {
    NamedFormKey::new(&format!("Owner{owner:03}.esm"), local).unwrap()
}
fn outfit(owner: usize) -> ActorOutfitFile {
    ActorOutfitFile {
        npc_id: name(owner, 0x21),
        outfit_id: name(owner, 0x22),
        parts: vec![OutfitPartFile {
            armo_id: name(owner, 0x23),
            armature_id: name(owner, 0x24),
            biped_mask: 4,
            armature_race_id: name(owner, 0x25),
            scene: "meshes/clothes/body.glb".into(),
        }],
    }
}
fn idle(owner: usize) -> ActorIdleBindingFile {
    ActorIdleBindingFile {
        race_id: name(owner, 0x25),
        sex: "male".into(),
        body_scene: "meshes/actors/body.glb".into(),
        project: "meshes/actors/project.hkx".into(),
        skeleton: "meshes/actors/rig.hkx".into(),
        race_skeleton_nif: "meshes/actors/skeleton.nif".into(),
        bound_tracks: 1,
        moving_bound_tracks: 1,
        skipped_tracks: vec![],
        source: "meshes/actors/idle.hkx".into(),
        scene: "meshes/actor-idles/clips/a.glb".into(),
        clips: BTreeMap::new(),
    }
}

#[test]
fn outfit_wire_names_roundtrip_and_foreign_generations_are_refused() {
    let scope = identity(1, 3);
    let file = ActorOutfitCatalogFile {
        version: shared::actor_outfits::VERSION,
        pack_identity: scope.digest_hex(),
        outfits: vec![outfit(0)],
        skipped: BTreeMap::new(),
    };
    let json = serde_json::to_value(&file).unwrap();
    assert_eq!(json["outfits"][0]["npc_id"], "000021:Owner000.esm");
    let runtime = file.to_runtime(&scope).unwrap();
    assert_eq!(runtime.outfits[0].npc_id, 0x21);
    let restored = runtime.to_file(&scope).unwrap();
    assert_eq!(serde_json::to_value(restored).unwrap(), json);
    assert!(
        file.validate_for(&identity(1, 4))
            .unwrap_err()
            .to_string()
            .contains("convert again")
    );
    let mut old = json;
    old["outfits"][0]["npc_id"] = serde_json::json!(33);
    assert!(serde_json::from_value::<ActorOutfitCatalogFile>(old).is_err());
}

#[test]
fn idle_wire_names_roundtrip_and_old_numeric_ids_are_refused() {
    let scope = identity(1, 3);
    let file = ActorIdleCatalogFile {
        version: shared::actor_idle_catalog::VERSION,
        pack_identity: scope.digest_hex(),
        bindings: vec![idle(0)],
        ..Default::default()
    };
    let json = serde_json::to_value(&file).unwrap();
    assert_eq!(json["bindings"][0]["race_id"], "000025:Owner000.esm");
    let runtime = file.to_runtime(&scope).unwrap();
    assert_eq!(runtime.bindings[0].race_id, 0x25);
    assert_eq!(
        serde_json::to_value(runtime.to_file(&scope).unwrap()).unwrap(),
        json
    );
    assert!(file.validate_for(&identity(1, 9)).is_err());
    let mut old = json;
    old["bindings"][0]["race_id"] = serde_json::json!(37);
    assert!(serde_json::from_value::<ActorIdleCatalogFile>(old).is_err());
}

#[test]
fn cap_free_catalogs_preserve_names_and_runtime_declines_only_unrepresentable_rows() {
    let identity = identity(256, 3);
    let outfits = ActorOutfitCatalogFile {
        version: shared::actor_outfits::VERSION,
        pack_identity: identity.digest_hex(),
        outfits: vec![outfit(254), outfit(0)],
        skipped: BTreeMap::new(),
    };
    outfits.validate_for(&identity).unwrap();
    assert_eq!(outfits.outfits[0].npc_id.to_string(), "000021:Owner254.esm");
    let runtime = outfits.to_runtime(&identity).unwrap();
    assert_eq!(runtime.outfits.len(), 1);
    assert_eq!(runtime.outfits[0].npc_id, 0x21);
    assert_eq!(runtime.skipped["runtime_outfit_identity_unavailable"], 1);
    let idles = ActorIdleCatalogFile {
        version: shared::actor_idle_catalog::VERSION,
        pack_identity: identity.digest_hex(),
        bindings: vec![idle(254), idle(0)],
        ..Default::default()
    };
    idles.validate_for(&identity).unwrap();
    let runtime = idles.to_runtime(&identity).unwrap();
    assert_eq!(runtime.bindings.len(), 1);
    assert_eq!(runtime.bindings[0].race_id, 0x25);
    assert_eq!(runtime.discovery_issues.len(), 1);
}
