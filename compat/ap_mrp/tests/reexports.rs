#[test]
fn legacy_mrp_package_reexports_the_same_type() {
    assert_eq!(
        std::any::TypeId::of::<ap_mrp::messages::Device>(),
        std::any::TypeId::of::<mrp::messages::Device>()
    );
}
