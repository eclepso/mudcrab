//! An ESL extension controls light ownership even without the header flag.
use shared::{
    cache_identity::{owner_order, required},
    pack_identity::PackIdentity,
    plugin_table::{PluginFacts, PluginTable},
};

#[test]
fn extension_only_light_owners_sort_after_full_owners() {
    let table = PluginTable::fresh(vec![
        (
            "Early.esl".into(),
            PluginFacts {
                content_hash: Some([0x19; 32]),
                ..Default::default()
            },
        ),
        (
            "Late.esm".into(),
            PluginFacts {
                content_hash: Some([0x27; 32]),
                ..Default::default()
            },
        ),
    ])
    .unwrap();
    let order = vec![
        table.id("Early.esl").unwrap(),
        table.id("Late.esm").unwrap(),
    ];
    let identity = PackIdentity::new(table, order).unwrap();
    let light = required(&identity, 0x0100_0011).unwrap();
    let full = required(&identity, 0x0200_0011).unwrap();
    assert_eq!(identity.runtime_id(light).unwrap(), 0xfe00_0011);
    assert_eq!(identity.runtime_id(full).unwrap(), 0x0000_0011);
    assert!(
        owner_order(&identity, full) < owner_order(&identity, light),
        "full owners must precede extension-only light owners"
    );
}
