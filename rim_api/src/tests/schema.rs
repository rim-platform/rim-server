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
