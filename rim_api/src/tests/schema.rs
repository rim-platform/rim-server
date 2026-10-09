use super::*;

// Keep these IDs referenced so dead-code elimination doesn't hide imports in coverage builds.
#[test]
fn _keep_ids_referenced() {
    let _ = (MessageId::new(), ConversationId::new(), ServerId::new());
}

/// Guards the `wire_common_schema_data` extraction (Track A item 3a,
/// 2026-07-08): `export_sdl` had no test before this, so the dedup between
/// it and `build_schema` was unverified. Asserts the SDL is non-empty and
/// contains a few well-known type markers.
#[test]
fn sdl_export_is_nonempty_and_has_core_types() {
    let sdl = export_sdl(
        test_service_provider(),
        InMemoryBroadcaster::new(16),
        test_presence_manager(),
        test_typing_tracker(),
        test_observers(),
    );

    assert!(
        !sdl.is_empty(),
        "export_sdl must return a non-empty SDL string"
    );
    assert!(
        sdl.contains("type Query"),
        "SDL must declare the Query type"
    );
    assert!(
        sdl.contains("type Mutation"),
        "SDL must declare the Mutation type"
    );
}

/// The device linking API exactly as `docs/specs/device-linking-v1.md` prints it.
#[test]
fn device_link_sdl_matches_the_spec() {
    let sdl = export_sdl(
        test_service_provider(),
        InMemoryBroadcaster::new(16),
        test_presence_manager(),
        test_typing_tracker(),
        test_observers(),
    );
    for field in [
        "deviceLink(linkId: String!): DeviceLink!",
        "deviceLinkAnswer(linkId: String!): SealedMessage",
        "startDeviceLink: StartedDeviceLink!",
        "sendDeviceLinkOffer(linkId: String!, offer: SealedMessageInput!): Boolean!",
        "sendDeviceLinkAnswer(linkId: String!, answer: SealedMessageInput!): DeviceLink!",
        "completeDeviceLink(linkId: String!): Boolean!",
        "cancelDeviceLink(linkId: String!): Boolean!",
        "linkId: String!",
        "state: DeviceLinkState!",
        "offer: SealedMessage\n",
    ] {
        assert!(sdl.contains(field), "SDL lacks `{field}`");
    }
    let compact: String = sdl.split_whitespace().collect::<Vec<_>>().join(" ");
    for definition in [
        "input SealedMessageInput { nonce: String! ciphertext: String! }",
        "type SealedMessage { nonce: String! ciphertext: String! }",
        "enum DeviceLinkState { OPEN OFFERED ANSWERED CONSUMED CANCELLED EXPIRED }",
    ] {
        assert!(compact.contains(definition), "SDL lacks `{definition}`");
    }
}
