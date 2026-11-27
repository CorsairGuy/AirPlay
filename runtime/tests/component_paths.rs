//! Component interfaces are provided directly by their owning service crates.
#[test]
fn receiver_components_are_available_from_services() {
    use std::any::TypeId;
    let _ = TypeId::of::<sessions::session::Context>();
    let _ = TypeId::of::<runtime::lifecycle::receiver::Options>();
    let _ = TypeId::of::<runtime::relay::group::PtpGroup>();
    let _ = TypeId::of::<runtime::pipeline::pcm::Options>();
    let _ = TypeId::of::<runtime::routing::control::RelayControl>();
    let _ = TypeId::of::<runtime::routing::router::Intent>();
    let _ = TypeId::of::<media::rx::Media>();
    let _ = TypeId::of::<media::rx::Player>();
    let _ = TypeId::of::<media::control::Remote>();
    let _ = TypeId::of::<remote::card::Card>();
    let _ = TypeId::of::<remote::mrp_service::model::Profile>();
}
