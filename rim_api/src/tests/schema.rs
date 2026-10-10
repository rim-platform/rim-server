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

/// Key-only signup's contract with the frontend codegen: the mutation, its input, and the
/// payload it shares with `signupWithInvite`.
#[test]
fn sdl_declares_signup_with_key() {
    let sdl = export_sdl(
        test_service_provider(),
        InMemoryBroadcaster::new(16),
        test_presence_manager(),
        test_typing_tracker(),
        test_observers(),
    );
    assert!(sdl.contains("signupWithKey(input: SignupWithKeyInput!): SignupWithInvitePayloadGql!"));
    assert!(sdl.contains("input SignupWithKeyInput {"));
    for field in [
        "username: String!",
        "displayName: String",
        "keyLog: [SignedStatementInput!]!",
        "device: SignedStatementInput!",
        "proof: SignedStatementInput!",
        "bootstrapToken: String",
        "instanceInviteToken: String",
        "workspaceInviteToken: String",
    ] {
        assert!(sdl.contains(field), "{field}");
    }
}

/// The SDL with every `"""` description removed and whitespace collapsed to single spaces.
fn compact_sdl_without_descriptions(sdl: &str) -> String {
    let without: String = sdl
        .split("\"\"\"")
        .enumerate()
        .filter(|(i, _)| i % 2 == 0)
        .map(|(_, part)| part)
        .collect();
    without.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The device linking types and fields, with their exact signatures.
#[test]
fn device_link_sdl_matches_the_contract() {
    let sdl = compact_sdl_without_descriptions(&export_sdl(
        test_service_provider(),
        InMemoryBroadcaster::new(16),
        test_presence_manager(),
        test_typing_tracker(),
        test_observers(),
    ));
    for expected in [
        "input SealedMessageInput { nonce: String! ciphertext: String! }",
        "type SealedMessage { nonce: String! ciphertext: String! }",
        "enum DeviceLinkState { OPEN OFFERED ANSWERED CONSUMED CANCELLED EXPIRED }",
        "type StartedDeviceLink { linkId: String! expiresAt: DateTime! }",
        "type DeviceLink { state: DeviceLinkState! expiresAt: DateTime! offer: SealedMessage }",
        "deviceLink(linkId: String!): DeviceLink!",
        "deviceLinkAnswer(linkId: String!): SealedMessage ",
        "startDeviceLink: StartedDeviceLink!",
        "sendDeviceLinkOffer(linkId: String!, offer: SealedMessageInput!): Boolean!",
        "sendDeviceLinkAnswer(linkId: String!, answer: SealedMessageInput!): DeviceLink!",
        "completeDeviceLink(linkId: String!): Boolean!",
        "cancelDeviceLink(linkId: String!): Boolean!",
    ] {
        assert!(sdl.contains(expected), "SDL lacks `{expected}`");
    }
}

/// `sdl` without its descriptions, whitespace collapsed to single spaces.
fn compact_without_descriptions(sdl: &str) -> String {
    let mut kept = Vec::new();
    let mut in_block = false;
    for line in sdl.lines().map(str::trim) {
        let opens_or_closes_block = line.starts_with("\"\"\"");
        let one_line_block = opens_or_closes_block && line.len() >= 6 && line.ends_with("\"\"\"");
        if in_block {
            in_block = !opens_or_closes_block && !line.ends_with("\"\"\"");
            continue;
        }
        if one_line_block || (line.starts_with('"') && !opens_or_closes_block) {
            continue;
        }
        if opens_or_closes_block {
            in_block = true;
            continue;
        }
        kept.push(line);
    }
    kept.join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The workspace plugin registry API, field for field.
#[test]
fn workspace_plugin_sdl_has_the_registry_api() {
    let sdl = export_sdl(
        test_service_provider(),
        InMemoryBroadcaster::new(16),
        test_presence_manager(),
        test_typing_tracker(),
        test_observers(),
    );
    let compact = compact_without_descriptions(&sdl);
    for definition in [
        "enum PluginRuntimeKind { DESKTOP_LOCAL_PROCESS WEB_SANDBOXED }",
        "enum WorkspacePluginStatus { RECOMMENDED REQUIRED }",
        "enum PluginSourceKind { BUNDLE GIT }",
        "enum PluginSignatureStatus { VALID PUBLISHER_RECOVERED }",
        "type PluginSource { kind: PluginSourceKind! bundleUrl: String bundleSha256: String \
         bundleSize: Int gitUrl: String gitCommit: String gitTreeSha256: String }",
        "type WorkspacePlugin { id: ID! serverId: ID! pluginId: String! version: String! \
         displayName: String! runtimeKind: PluginRuntimeKind! status: WorkspacePluginStatus! \
         source: PluginSource! manifest: String! statement: SignedStatement! \
         publisherPrincipalId: String! publisherName: String! publisherKeySeq: Int! \
         publisherKeyLog: [SignedStatement!]! signatureStatus: PluginSignatureStatus! \
         approvedBy: User approvedAt: DateTime! updatedAt: DateTime! }",
        "input AddWorkspacePluginInput { serverId: ID! statement: SignedStatementInput! \
         status: WorkspacePluginStatus! bundleUrl: String \
         publisherKeyLog: [SignedStatementInput!] }",
        "input UpdateWorkspacePluginInput { serverId: ID! pluginId: String! \
         statement: SignedStatementInput status: WorkspacePluginStatus bundleUrl: String \
         publisherKeyLog: [SignedStatementInput!] }",
        "workspacePlugins(serverId: ID!): [WorkspacePlugin!]!",
        "addWorkspacePlugin(input: AddWorkspacePluginInput!): WorkspacePlugin!",
        "updateWorkspacePlugin(input: UpdateWorkspacePluginInput!): WorkspacePlugin!",
        "removeWorkspacePlugin(serverId: ID!, pluginId: String!): Boolean!",
    ] {
        assert!(compact.contains(definition), "SDL lacks `{definition}`");
    }
}

/// Verified reports of encrypted messages: the franking fields and the single-report query.
#[test]
fn message_report_sdl_has_the_franked_report_fields() {
    let sdl = export_sdl(
        test_service_provider(),
        InMemoryBroadcaster::new(16),
        test_presence_manager(),
        test_typing_tracker(),
        test_observers(),
    );
    let compact = compact_without_descriptions(&sdl);
    for definition in [
        "messageReadable: Boolean! messageText: String messageAuthorId: ID \
         messageSentAt: DateTime frankingVerifiedAt: DateTime frankedSenderDeviceId: ID \
         frankedContent: String frankedContentExpiresAt: DateTime }",
        "messageReport(serverId: ID!, reportId: ID!): MessageReport!",
        "reportEncryptedMessage(input: ReportEncryptedMessageInput!): Boolean!",
    ] {
        assert!(compact.contains(definition), "SDL lacks `{definition}`");
    }
}

/// `serverInfo` and its type, with their exact signatures.
#[test]
fn server_info_sdl_matches_the_contract() {
    let sdl = compact_sdl_without_descriptions(&export_sdl(
        test_service_provider(),
        InMemoryBroadcaster::new(16),
        test_presence_manager(),
        test_typing_tracker(),
        test_observers(),
    ));
    assert!(sdl.contains("serverInfo: ServerInfo!"));
    assert!(sdl.contains("type ServerInfo { version: String! apiVersion: Int! }"));
}

/// `serverInfo` answers without a session.
#[tokio::test]
async fn server_info_needs_no_session() {
    let schema = schema_with_user_mock(MockUserService::new());
    let res = schema
        .execute(Request::new("{ serverInfo { version apiVersion } }"))
        .await;
    assert_no_errors(&res);
    let data = res.data.into_json().expect("json");
    assert_eq!(data["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(data["serverInfo"]["apiVersion"], crate::types::API_VERSION);
}
