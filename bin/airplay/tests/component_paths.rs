//! Legacy paths and the new component paths refer to the same types.
//!
//! A wrapper or a copied type would get its own `TypeId`, so these checks
//! also prove that the binary re-exports the service implementation instead
//! of pretending to move it.
#[test]
fn receiver_components_are_the_service_types() {
    use std::any::TypeId;
    assert_eq!(
        TypeId::of::<airplay::sessions::session::Context>(),
        TypeId::of::<sessions::session::Context>()
    );
    assert_eq!(
        TypeId::of::<airplay::media::rx::Media>(),
        TypeId::of::<media::rx::Media>()
    );
    assert_eq!(
        TypeId::of::<airplay::media::rx::Player>(),
        TypeId::of::<media::rx::Player>()
    );
    assert_eq!(
        TypeId::of::<airplay::remote::Command>(),
        TypeId::of::<media::control::Command>()
    );
    assert_eq!(
        TypeId::of::<airplay::remote::Remote>(),
        TypeId::of::<media::control::Remote>()
    );
    assert_eq!(
        TypeId::of::<airplay::card::Card>(),
        TypeId::of::<remote::card::Card>()
    );
    assert_eq!(
        TypeId::of::<airplay::continuous_control::SourceControl>(),
        TypeId::of::<remote::continuous_control::SourceControl>()
    );
    assert_eq!(
        TypeId::of::<airplay::output_control::OutputControl>(),
        TypeId::of::<remote::output_control::OutputControl>()
    );
    assert_eq!(
        TypeId::of::<airplay::coordinator::output_worker::Snapshot>(),
        TypeId::of::<sessions::output_worker::Snapshot>()
    );
}

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
