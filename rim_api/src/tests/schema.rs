use super::*;

// Keep these IDs referenced so dead-code elimination doesn't hide imports in coverage builds.
#[test]
fn _keep_ids_referenced() {
    let _ = (MessageId::new(), ConversationId::new(), ServerId::new());
}

/// The SDL is non-empty and declares the root types.
#[test]
fn sdl_export_is_nonempty_and_has_core_types() {
    let sdl = test_sdl();

    assert!(!sdl.is_empty(), "the SDL must not be empty");
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
    let sdl = test_sdl();
    assert!(sdl.contains("signupWithKey(input: SignupWithKeyInput!): SignupWithInvitePayloadGql!"));
    assert!(sdl.contains("input SignupWithKeyInput {"));
    for field in [
        "username: String!",
        "displayName: String",
        "keyLog: [SignedStatementInput!]!",
        "device: SignedStatementInput!",
        "proof: SignedStatementInput!",
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
    let sdl = compact_sdl_without_descriptions(&test_sdl());
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
    let sdl = test_sdl();
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
    let sdl = test_sdl();
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
    let sdl = compact_sdl_without_descriptions(&test_sdl());
    assert!(sdl.contains("serverInfo: ServerInfo!"));
    assert!(sdl.contains(
        "type ServerInfo { version: String! apiVersion: Int! features: [ServerFeatureInfo!]! \
         auditRetentionDays: Int instanceNotice: String termsUrl: String limits: ServerLimits! \
         signupRequiresKeys: Boolean! dmInbound: DmInbound! dmOutbound: DmOutbound! \
         dmPartners: [String!]! defaultWorkspace: ID registrationFull: Boolean! \
         abuseContact: String minimumAge: Int }"
    ));
    assert!(sdl.contains("enum DmInbound { OPEN REQUESTS PARTNERS NONE }"));
    assert!(sdl.contains("enum DmOutbound { ALLOWED PARTNERS NONE }"));
    assert!(sdl.contains(
        "type ServerLimits { workspacesPerUser: Int membersPerWorkspace: Int \
         channelsPerWorkspace: Int devicesPerUser: Int dmRequestsPerDay: Int \
         macrosPerWorkspace: Int pipelinesPerWorkspace: Int pluginsPerWorkspace: Int \
         invitesPerUserPerDay: Int inviteMaxUses: Int messageBytes: Int probationHours: Int \
         pendingInvitesPerUser: Int accounts: Int }"
    ));
    assert!(sdl.contains(
        "type ServerFeatureInfo { key: String! state: ServerFeatureState! reason: String }"
    ));
    assert!(sdl.contains("enum ServerFeatureState { ENABLED DISABLED UNAVAILABLE }"));
}

/// `User` ends with the caller's probation end, pending deletion, operator role and handle.
#[test]
fn user_sdl_ends_with_the_probation_end_the_pending_deletion_the_operator_role_and_the_handle() {
    let sdl = compact_sdl_without_descriptions(&test_sdl());
    assert!(
        sdl.contains(
            "principalId: String probationEndsAt: DateTime deletionScheduledFor: DateTime \
             instanceOperator: Boolean! handle: String }"
        ),
        "User must end with probationEndsAt, deletionScheduledFor, instanceOperator, then handle"
    );
}

/// `serverInfo` answers without a session.
#[tokio::test]
async fn server_info_needs_no_session() {
    let schema = schema_with_user_mock(MockUserService::new());
    let res = schema
        .execute(Request::new(
            "{ serverInfo { version apiVersion auditRetentionDays instanceNotice termsUrl } }",
        ))
        .await;
    assert_no_errors(&res);
    let data = res.data.into_json().expect("json");
    assert_eq!(data["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(data["serverInfo"]["apiVersion"], crate::types::API_VERSION);
    assert!(data["serverInfo"]["auditRetentionDays"].is_null());
    assert!(data["serverInfo"]["instanceNotice"].is_null());
    assert!(data["serverInfo"]["termsUrl"].is_null());
}

/// `serverInfo.auditRetentionDays` reports the configured audit retention.
#[tokio::test]
async fn server_info_reports_the_audit_retention() {
    let schema = schema_with_user_mock(MockUserService::new());
    let res = schema
        .execute(
            Request::new("{ serverInfo { auditRetentionDays } }")
                .data(crate::types::AuditRetention(Some(90))),
        )
        .await;
    assert_no_errors(&res);
    let data = res.data.into_json().expect("json");
    assert_eq!(data["serverInfo"]["auditRetentionDays"], 90);
}

/// `serverInfo.instanceNotice`, `termsUrl`, `signupRequiresKeys`, `defaultWorkspace`,
/// `abuseContact` and `minimumAge` report the configured instance policy.
#[tokio::test]
async fn server_info_reports_the_instance_notice_and_terms() {
    let schema = schema_with_user_mock(MockUserService::new());
    let workspace = rim_core::server::model::ServerId::new();
    let res = schema
        .execute(
            Request::new(
                "{ serverInfo { instanceNotice termsUrl signupRequiresKeys defaultWorkspace \
                 abuseContact minimumAge } }",
            )
            .data(crate::types::InstancePolicy {
                notice: Some("Run by the Example club.".to_string()),
                terms_url: Some("https://rim.example.org/terms".to_string()),
                signup_requires_keys: true,
                default_workspace: Some(workspace),
                abuse_contact: Some("abuse@rim.example.org".to_string()),
                minimum_age: Some(16),
            }),
        )
        .await;
    assert_no_errors(&res);
    let data = res.data.into_json().expect("json");
    assert_eq!(
        data["serverInfo"],
        serde_json::json!({
            "instanceNotice": "Run by the Example club.",
            "termsUrl": "https://rim.example.org/terms",
            "signupRequiresKeys": true,
            "defaultWorkspace": workspace.to_string(),
            "abuseContact": "abuse@rim.example.org",
            "minimumAge": 16,
        })
    );
}

/// `serverInfo.defaultWorkspace`, `abuseContact` and `minimumAge` are null when the server
/// names none.
#[tokio::test]
async fn server_info_default_workspace_is_null_when_unset() {
    let schema = schema_with_user_mock(MockUserService::new());
    let res = schema
        .execute(
            Request::new("{ serverInfo { defaultWorkspace abuseContact minimumAge } }")
                .data(crate::types::InstancePolicy::default()),
        )
        .await;
    assert_no_errors(&res);
    let data = res.data.into_json().expect("json");
    assert!(data["serverInfo"]["defaultWorkspace"].is_null());
    assert!(data["serverInfo"]["abuseContact"].is_null());
    assert!(data["serverInfo"]["minimumAge"].is_null());
}

/// `serverInfo.limits` reports the configured caps, null where none is set.
#[tokio::test]
async fn server_info_reports_the_instance_limits() {
    let schema = schema_with_user_mock(MockUserService::new());
    let res = schema
        .execute(
            Request::new(
                "{ serverInfo { limits { workspacesPerUser membersPerWorkspace \
                 channelsPerWorkspace devicesPerUser dmRequestsPerDay macrosPerWorkspace \
                 pipelinesPerWorkspace pluginsPerWorkspace invitesPerUserPerDay inviteMaxUses \
                 messageBytes probationHours pendingInvitesPerUser accounts } } }",
            )
            .data(rim_services::limits::InstanceLimits {
                workspaces_per_user: Some(5),
                channels_per_workspace: Some(30),
                devices_per_user: Some(5),
                dm_requests_per_day: Some(20),
                message_bytes: Some(8192),
                probation_hours: Some(24),
                pending_invites_per_user: Some(50),
                accounts: Some(1000),
                ..Default::default()
            }),
        )
        .await;
    assert_no_errors(&res);
    let data = res.data.into_json().expect("json");
    assert_eq!(
        data["serverInfo"]["limits"],
        serde_json::json!({
            "workspacesPerUser": 5,
            "membersPerWorkspace": null,
            "channelsPerWorkspace": 30,
            "devicesPerUser": 5,
            "dmRequestsPerDay": 20,
            "macrosPerWorkspace": null,
            "pipelinesPerWorkspace": null,
            "pluginsPerWorkspace": null,
            "invitesPerUserPerDay": null,
            "inviteMaxUses": null,
            "messageBytes": 8192,
            "probationHours": 24,
            "pendingInvitesPerUser": 50,
            "accounts": 1000,
        })
    );
}

/// `serverInfo.registrationFull` is what the auth service answers, and false when it cannot
/// tell, so the rest of `serverInfo` still answers.
#[tokio::test]
async fn server_info_says_whether_registration_is_full() {
    for (answer, expected) in [(Ok(true), true), (Ok(false), false), (Err(()), false)] {
        let mut auth = rim_services::auth::service::MockAuthService::new();
        auth.expect_registration_full().times(1).returning(move || {
            Box::pin(async move { answer.map_err(|()| rim_services::ServiceError::Unavailable) })
        });
        let schema = super::schema_with_mocks(
            auth,
            rim_services::conversation::service::MockConversationService::new(),
            rim_services::message::service::MockMessageService::new(),
            rim_services::message_attachment::service::MockMessageAttachmentService::new(),
            rim_services::message_thread::service::MockThreadService::new(),
        );
        let res = schema
            .execute(Request::new("{ serverInfo { version registrationFull } }"))
            .await;
        assert_no_errors(&res);
        let data = res.data.into_json().expect("json");
        assert_eq!(data["serverInfo"]["registrationFull"], expected);
    }
}

/// `serverInfo` reports the operator's DM settings.
#[tokio::test]
async fn server_info_reports_the_dm_settings() {
    let schema = schema_with_user_mock(MockUserService::new());
    let res = schema
        .execute(
            Request::new("{ serverInfo { dmInbound dmOutbound dmPartners } }").data(
                rim_services::dm_policy::dto::DmOperatorPolicy {
                    inbound: rim_core::dm_policy::model::DmInbound::Partners,
                    outbound: rim_core::dm_policy::model::DmOutbound::None,
                    partners: vec!["rims1_partner".to_string()],
                },
            ),
        )
        .await;
    assert_no_errors(&res);
    let data = res.data.into_json().expect("json");
    assert_eq!(
        data["serverInfo"],
        serde_json::json!({
            "dmInbound": "PARTNERS",
            "dmOutbound": "NONE",
            "dmPartners": ["rims1_partner"],
        })
    );
}

/// The DM settings' types, fields and roots, with their exact signatures; `Server` ends with
/// its DM rules and `Conversation` with `dmRequestPending`.
#[test]
fn dm_policy_sdl_matches_the_contract() {
    let sdl = compact_sdl_without_descriptions(&test_sdl());
    for expected in [
        "dmOutsideFloor: DmOutsideFloor dmMemberDefault: DmMembers }",
        "predecessor: ConversationPredecessor dmRequestPending: Boolean }",
        "myDmSettings: MyDmSettings!",
        "setWorkspaceDmPolicy(workspaceId: ID!, outsideFloor: DmOutsideFloor!, \
         memberDefault: DmMembers!): WorkspaceDmPolicy!",
        "setMyDmSettings(outside: DmOutside!): MyDmSettings!",
        "setMyWorkspaceDmSetting(workspaceId: ID!, members: DmMembers!): MyDmSettings!",
        "type MyDmSettings { outside: DmOutside! workspaces: [WorkspaceDmSetting!]! \
         effectiveOutside: DmOutside! outsideFloorFrom: DmFloorSource }",
        "type WorkspaceDmSetting { workspaceId: ID! members: DmMembers! memberDefault: DmMembers! }",
        "type DmFloorSource { kind: DmFloorSourceKind! workspaceId: ID workspaceName: String }",
        "type WorkspaceDmPolicy { workspaceId: ID! outsideFloor: DmOutsideFloor! \
         memberDefault: DmMembers! }",
        "enum DmOutsideFloor { NONE REQUESTS CLOSED }",
        "enum DmMembers { DIRECT REQUESTS }",
        "enum DmOutside { REQUESTS NONE }",
        "enum DmFloorSourceKind { OPERATOR WORKSPACE }",
    ] {
        assert!(sdl.contains(expected), "the SDL lacks {expected}");
    }
}

/// Both signup inputs end with the optional `acceptedTermsUrl` and `confirmedMinimumAge`.
#[test]
fn signup_inputs_take_the_accepted_terms_last() {
    let sdl = compact_sdl_without_descriptions(&test_sdl());
    for expected in [
        "instanceInviteToken: String workspaceInviteToken: String acceptedTermsUrl: String \
         confirmedMinimumAge: Int }",
        "proofOfWork: String acceptedTermsUrl: String confirmedMinimumAge: Int }",
    ] {
        assert!(sdl.contains(expected), "SDL lacks `{expected}`");
    }
}

/// The instance moderation roots and types, with their exact signatures.
#[test]
fn instance_moderation_sdl_matches_the_contract() {
    let sdl = compact_sdl_without_descriptions(&test_sdl());
    for expected in [
        "instanceReports( openOnly: Boolean! = true ): [InstanceAbuseReport!]!",
        "instancePrincipalBlocks: [InstancePrincipalBlock!]!",
        "instanceIpBlocks: [InstanceIpBlock!]!",
        "instanceBlockPrincipal( principalId: String!, reason: String!, cascade: Boolean! = \
         false ): [InstancePrincipalBlockOutcome!]!",
        "instanceUnblockPrincipal( principalId: String! ): InstancePrincipalUnblockOutcome!",
        "instanceResolveReport( id: ID! ): InstanceAbuseReport!",
        "instanceBlockIp( cidr: String!, reason: String!, expiresInDays: Int ): \
         InstanceIpBlockOutcome!",
        "instanceUnblockIp( cidr: String! ): Boolean!",
        "type InstanceAbuseReport { id: ID! reporterUserId: ID reporterUsername: String \
         subjectPrincipalId: String subjectUsername: String subjectServerId: String requestId: \
         ID reason: String! createdAt: DateTime! resolvedAt: DateTime resolved: Boolean! }",
        "type InstancePrincipalBlock { principalId: String! username: String reason: String! \
         createdAt: DateTime! }",
        "type InstanceIpBlock { cidr: String! reason: String! expiresAt: DateTime createdAt: \
         DateTime! expired: Boolean! }",
        "type InstanceIpBlockOutcome { block: InstanceIpBlock! newlyBlocked: Boolean! }",
        "type InstanceAccount { userId: ID! username: String! principalId: String createdAt: \
         DateTime! }",
        "type InstancePrincipalBlockOutcome { principalId: String newlyBlocked: Boolean! \
         account: InstanceAccount reason: String! endedSessions: Int! ignoredRequests: Int! \
         revokedInvites: Int! emailBlocked: Boolean! invitees: [InstanceAccount!]! }",
        "type InstancePrincipalUnblockOutcome { lifted: Boolean! emailUnblocked: Boolean! }",
    ] {
        assert!(sdl.contains(expected), "SDL lacks `{expected}`");
    }
}

/// Every instance moderation type, field, root and argument carries a description.
#[test]
fn instance_moderation_sdl_describes_every_type_field_and_argument() {
    let sdl = test_sdl();
    let mut undocumented = Vec::new();
    let mut in_description = false;
    let mut described = false;
    let mut in_instance_type = false;
    let mut in_root = false;
    let mut in_instance_args = false;
    for line in sdl.lines().map(str::trim) {
        if line == "\"\"\"" {
            in_description = !in_description;
            described = !in_description;
            continue;
        }
        if in_description || line.is_empty() {
            continue;
        }
        let instance_root = in_root && !in_instance_args && line.starts_with("instance");
        let relevant = line.starts_with("type Instance")
            || instance_root
            || (in_instance_type && line != "}")
            || (in_instance_args && !line.starts_with(')'));
        // Arguments without descriptions are printed inline, on the root's own line.
        let inline_arguments = instance_root && line.contains('(') && !line.ends_with('(');
        if relevant && (!described || inline_arguments) {
            undocumented.push(line.to_string());
        }
        if line.starts_with("type ") {
            in_instance_type = line.starts_with("type Instance");
            in_root = line == "type Query {" || line == "type Mutation {";
        } else if line == "}" {
            in_instance_type = false;
            in_root = false;
        }
        if instance_root && line.ends_with('(') {
            in_instance_args = true;
        } else if line.starts_with(')') {
            in_instance_args = false;
        }
        described = false;
    }
    assert!(undocumented.is_empty(), "undocumented: {undocumented:?}");
}

/// Reaction notifications: the reason, the notification's reaction fields and the setting, each
/// appended after the existing members.
#[test]
fn notification_sdl_ends_with_the_reaction_fields() {
    let sdl = compact_sdl_without_descriptions(&test_sdl());
    for expected in [
        "enum NotificationReason { DIRECT MENTION KEYWORD THREAD_REPLY ALL REACTION }",
        "readAt: DateTime conversation: Conversation message: Message reactionEmoji: String \
         reactedBy: User }",
        "digest: DigestFrequency! reactions: Boolean! }",
        "dnd: DndWindowInput digest: DigestFrequency! reactions: Boolean }",
    ] {
        assert!(sdl.contains(expected), "SDL lacks `{expected}`");
    }
}

/// The workspace invite list and revocation, with their exact signatures, appended after the
/// existing roots; the listed invite carries no token.
#[test]
fn workspace_invite_sdl_matches_the_contract() {
    let sdl = compact_sdl_without_descriptions(&test_sdl());
    for expected in [
        "userCommands: [UserCommand!]! workspaceInvites(serverId: ID!, dir: PageDir, cursor: \
         WorkspaceInviteCursorInput, limit: Int): WorkspaceInvitePage! }",
        "setMyWorkspaceDmSetting(workspaceId: ID!, members: DmMembers!): MyDmSettings! \
         revokeInvite(inviteId: ID!): WorkspaceInvite! }",
        "type WorkspaceInvite { id: ID! serverId: ID! role: ServerMemberRoleGql! useCount: Int! \
         maxUses: Int! expiresAt: DateTime createdAt: DateTime! createdById: ID! revokedAt: \
         DateTime state: WorkspaceInviteState! createdBy: User }",
        "enum WorkspaceInviteState { ACTIVE EXPIRED USED_UP REVOKED }",
        "input WorkspaceInviteCursorInput { createdAt: DateTime! inviteId: ID! }",
        "type WorkspaceInviteCursorOutput { createdAt: DateTime! inviteId: ID! }",
        "type WorkspaceInvitePage { items: [WorkspaceInvite!]! next: \
         WorkspaceInviteCursorOutput prev: WorkspaceInviteCursorOutput pageInfo: PageInfo! }",
    ] {
        assert!(sdl.contains(expected), "SDL lacks `{expected}`");
    }
}
