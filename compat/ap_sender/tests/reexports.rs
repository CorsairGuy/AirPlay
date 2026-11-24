#[test]
fn legacy_sender_package_reexports_the_same_type() {
    assert_eq!(
        std::any::TypeId::of::<ap_sender::SessionConfig>(),
        std::any::TypeId::of::<::media::tx::SessionConfig>()
    );
    assert_eq!(
        std::any::TypeId::of::<ap_sender::group::Coordinator>(),
        std::any::TypeId::of::<coordinator::group::Coordinator>()
    );
    assert_eq!(
        std::any::TypeId::of::<ap_sender::outputs::OutputCatalog>(),
        std::any::TypeId::of::<coordinator::outputs::OutputCatalog>()
    );
}
