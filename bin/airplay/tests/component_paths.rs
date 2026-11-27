//! Component interfaces are provided directly by their owning service crates.
#[test]
fn receiver_components_are_available_from_services() {
    use std::any::TypeId;
    let _ = TypeId::of::<sessions::session::Context>();
    let _ = TypeId::of::<sessions::serve::Options>();
    let _ = TypeId::of::<sessions::ptp_group::PtpGroup>();
    let _ = TypeId::of::<sessions::pcm_sender::Options>();
    let _ = TypeId::of::<sessions::send_control::RelayControl>();
    let _ = TypeId::of::<sessions::output_router::Intent>();
    let _ = TypeId::of::<media::rx::Media>();
    let _ = TypeId::of::<media::rx::Player>();
    let _ = TypeId::of::<media::control::Remote>();
    let _ = TypeId::of::<remote::card::Card>();
    let _ = TypeId::of::<remote::mrp_service::model::Profile>();
}
