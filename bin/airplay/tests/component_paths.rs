//! Legacy paths and the new component paths refer to the same types.
#[test]
fn legacy_and_component_paths_share_types() {
    use std::any::TypeId;
    assert_eq!(
        TypeId::of::<airplay::discovery_catalog::Advertisement>(),
        TypeId::of::<airplay::discovery::catalog::Advertisement>()
    );
    assert_eq!(
        TypeId::of::<airplay::session::Context>(),
        TypeId::of::<airplay::sessions::session::Context>()
    );
    assert_eq!(
        TypeId::of::<airplay::sender_store::SenderStore>(),
        TypeId::of::<airplay::identity::sender_store::SenderStore>()
    );
    assert_eq!(
        TypeId::of::<airplay::media::Media>(),
        TypeId::of::<airplay::media::rx::Media>()
    );
    assert_eq!(
        TypeId::of::<airplay::send::Options>(),
        TypeId::of::<airplay::media::tx::Options>()
    );
    assert_eq!(
        TypeId::of::<airplay::mrp_service::model::Profile>(),
        TypeId::of::<airplay::remote::mrp_service::model::Profile>()
    );
}
