pub use sea_orm_migration::prelude::*;

/// Channel-membership back-fill, exposed for the `rim_infra_db` db-tests.
pub use m20261005_005743_add_channel_privacy_and_archive::CHANNEL_MEMBERSHIP_BACKFILL_SQL;

/// Role data step (`guest` -> `member`), exposed for the `rim_infra_db` db-tests.
pub use m20261005_024730_add_member_role::GUEST_TO_MEMBER_SQL;

/// Notification data step (permanent mute -> `none` level), exposed for the
/// `rim_infra_db` db-tests.
pub use m20261005_045140_add_notification_rules::PERMANENT_MUTE_TO_NONE_SQL;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20251012_135522_create_user::Migration),
            Box::new(m20251029_222439_create_server::Migration),
            Box::new(m20251122_154540_create_conversation::Migration),
            Box::new(m20251123_105135_create_threads_message::Migration),
            Box::new(m20260122_120000_add_conversation_last_activity_at::Migration),
            Box::new(m20260212_000001_create_sessions_and_invites::Migration),
            Box::new(m20260212_000002_expand_user_password_hash::Migration),
            Box::new(m20260216_230500_fix_user_last_login_at_not_null::Migration),
            Box::new(m20260222_214356_add_conversation_main_thread_id::Migration),
            Box::new(m20260223_220000_make_session_revoked_last_seen_nullable::Migration),
            Box::new(m20260308_210831_add_conversation_owned_messages::Migration),
            Box::new(m20260308_213715_remove_legacy_thread_columns::Migration),
            Box::new(m20260308_215130_add_unique_thread_root_message::Migration),
            Box::new(m20260308_224258_add_membership_uniqueness::Migration),
            Box::new(m20260309_222347_add_conversation_title::Migration),
            Box::new(m20260310_220344_create_user_command::Migration),
            Box::new(m20260310_230745_add_safe_schema_hardening::Migration),
            Box::new(m20260310_235420_create_keybind::Migration),
            Box::new(m20260318_173750_add_missing_indexes::Migration),
            Box::new(m20260318_200000_make_user_last_login_at_nullable::Migration),
            Box::new(m20260318_210000_add_conversation_member_joined_at::Migration),
            Box::new(m20260320_000000_create_message_reaction::Migration),
            Box::new(m20260322_000000_add_unique_channel_title::Migration),
            Box::new(m20260331_create_conversation_read_state::Migration),
            Box::new(m20260401_add_conversation_topic::Migration),
            Box::new(m20260401_create_message_pin::Migration),
            Box::new(m20260402_add_message_forwarding::Migration),
            Box::new(m20260402_add_user_profile_fields::Migration),
            Box::new(m20260402_create_message_mention::Migration),
            Box::new(m20260402_create_notification_preference::Migration),
            Box::new(m20260404_000000_create_password_reset_token::Migration),
            Box::new(m20260404_000001_create_link_preview::Migration),
            Box::new(m20260404_000002_create_custom_emoji::Migration),
            Box::new(m20260404_000003_create_macro::Migration),
            Box::new(m20260407_000001_create_channel_lens::Migration),
            Box::new(m20260407_000002_create_incident::Migration),
            Box::new(m20260407_000003_create_user_skill_profile::Migration),
            Box::new(m20260408_122101_fix_fk_on_delete_policies::Migration),
            Box::new(m20260408_211135_create_workspace_state::Migration),
            Box::new(m20260409_000001_create_pipeline::Migration),
            Box::new(m20260424_120000_create_capability_registry::Migration),
            Box::new(m20260425_000001_create_command_audit_log::Migration),
            Box::new(m20260425_000002_audit_log_add_check_outcome_columns::Migration),
            Box::new(m20260426_180614_macro_add_ai_columns::Migration),
            Box::new(m20260426_200000_capability_registry_add_is_self_scoped::Migration),
            Box::new(m20260427_000001_audit_outcome_widen_check::Migration),
            Box::new(m20260428_182238_add_registration_invites::Migration),
            Box::new(m20260428_194718_add_user_email::Migration),
            Box::new(m20260708_220447_add_command_audit_log_parent_audit_id::Migration),
            Box::new(m20260708_225022_add_message_search_tsv::Migration),
            Box::new(m20260708_232445_create_workspace_ingest_token::Migration),
            Box::new(m20260708_233516_add_incident_alert_fields::Migration),
            Box::new(m20260710_094314_create_oncall_schedules::Migration),
            Box::new(m20260710_094331_add_user_timezone::Migration),
            Box::new(m20260710_111731_create_escalation_policies::Migration),
            Box::new(m20260710_111818_create_escalation_run::Migration),
            Box::new(m20260710_111818_add_incident_escalation_and_ack_fields::Migration),
            Box::new(m20261004_220805_add_workspace_feature::Migration),
            Box::new(m20261005_005743_add_channel_privacy_and_archive::Migration),
            Box::new(m20261005_024730_add_member_role::Migration),
            Box::new(m20261005_024731_add_moderation::Migration),
            Box::new(m20261005_045140_add_notification_rules::Migration),
            Box::new(m20261008_211159_add_message_change_feed::Migration),
            Box::new(m20261009_054837_add_key_identity::Migration),
            Box::new(m20261009_075648_key_identity_algorithm_agility::Migration),
            Box::new(m20261009_105834_key_only_signup::Migration),
            Box::new(m20261009_112441_add_device_link::Migration),
            Box::new(m20261009_133359_add_workspace_plugin::Migration),
            Box::new(m20261009_182741_add_e2ee_mls::Migration),
            Box::new(m20261009_214158_add_server_identity::Migration),
            Box::new(m20261009_220223_add_report_sender_device::Migration),
            Box::new(m20261009_230613_add_mls_readd_nonce::Migration),
            Box::new(m20261009_231431_drop_message_franking_key_fk::Migration),
            Box::new(m20261010_000142_add_message_franking_revision::Migration),
            Box::new(m20261010_002421_add_relay_delivery::Migration),
            Box::new(m20261010_010132_add_remote_workspaces::Migration),
            Box::new(m20261010_010907_add_relay_abuse::Migration),
            Box::new(m20261010_143053_make_instance_invite_creator_optional::Migration),
            Box::new(m20261010_153941_add_command_audit_log_created_at_index::Migration),
            Box::new(m20261010_184831_drop_incident_module::Migration),
            Box::new(m20261010_200459_drop_franking_key::Migration),
            Box::new(m20261010_205037_drop_redundant_indexes::Migration),
            Box::new(m20261010_205121_fold_bootstrap_tokens_into_instance_invites::Migration),
            Box::new(m20261011_141814_widen_message_text::Migration),
            Box::new(m20261012_142739_add_ip_block::Migration),
            Box::new(m20261012_140916_add_terms_acceptance_to_user::Migration),
            Box::new(m20261012_174304_add_account_deletion::Migration),
            Box::new(m20261012_173710_add_user_invite_chain::Migration),
            Box::new(m20261012_173711_add_blocked_email::Migration),
            Box::new(m20261013_055701_add_instance_operator_to_user::Migration),
            Box::new(m20261015_175228_add_conversation_member_pending::Migration),
            Box::new(m20261016_223549_add_workspace_dm_policy::Migration),
            Box::new(m20261016_223550_add_user_dm_settings::Migration),
            Box::new(m20261017_195236_add_reaction_notification_reason::Migration),
            Box::new(m20261017_195237_add_reaction_notification_fields::Migration),
            Box::new(m20261019_143809_add_user_username_lower_unique::Migration),
            Box::new(m20261019_182336_user_activity::Migration),
            Box::new(m20261019_194441_add_key_challenge_purpose::Migration),
            Box::new(m20261019_214438_add_relay_dm_request_host_user::Migration),
            Box::new(m20261019_225841_add_peer_delivery_failing_since::Migration),
            Box::new(m20261019_230105_add_peer_pin_forgotten::Migration),
            Box::new(m20261020_204601_add_instance_default_workspace::Migration),
            Box::new(m20261020_204805_add_minimum_age_confirmed_to_user::Migration),
            Box::new(m20261020_204850_add_user_handles::Migration),
            Box::new(m20261021_222259_add_relay_abuse_report_unique_request::Migration),
            Box::new(m20261022_062917_add_conversation_member_invited_by::Migration),
            Box::new(m20261023_191039_add_public_join_link::Migration),
            Box::new(m20261023_211807_add_remote_join_requests::Migration),
            Box::new(m20261024_110117_add_account_password::Migration),
            Box::new(m20261024_163804_drop_session_must_set_password::Migration),
            Box::new(m20261024_174153_remove_password_accounts::Migration),
            Box::new(m20261026_120000_add_conversation_posting::Migration),
            Box::new(m20261026_120342_add_user_profile_locked_until::Migration),
            Box::new(m20261029_084741_add_message_flag::Migration),
        ]
    }
}
mod m20251012_135522_create_user;
mod m20251029_222439_create_server;
mod m20251122_154540_create_conversation;
mod m20251123_105135_create_threads_message;
mod m20260122_120000_add_conversation_last_activity_at;
mod m20260212_000001_create_sessions_and_invites;
mod m20260212_000002_expand_user_password_hash;
mod m20260216_230500_fix_user_last_login_at_not_null;
mod m20260222_214356_add_conversation_main_thread_id;
mod m20260223_220000_make_session_revoked_last_seen_nullable;
mod m20260308_210831_add_conversation_owned_messages;
mod m20260308_213715_remove_legacy_thread_columns;
mod m20260308_215130_add_unique_thread_root_message;
mod m20260308_224258_add_membership_uniqueness;
mod m20260309_222347_add_conversation_title;
mod m20260310_220344_create_user_command;
mod m20260310_230745_add_safe_schema_hardening;
mod m20260310_235420_create_keybind;
mod m20260318_173750_add_missing_indexes;
mod m20260318_200000_make_user_last_login_at_nullable;
mod m20260318_210000_add_conversation_member_joined_at;
mod m20260320_000000_create_message_reaction;
mod m20260322_000000_add_unique_channel_title;
mod m20260331_create_conversation_read_state;
mod m20260401_add_conversation_topic;
mod m20260401_create_message_pin;
mod m20260402_add_message_forwarding;
mod m20260402_add_user_profile_fields;
mod m20260402_create_message_mention;
mod m20260402_create_notification_preference;
mod m20260404_000000_create_password_reset_token;
mod m20260404_000001_create_link_preview;
mod m20260404_000002_create_custom_emoji;
mod m20260404_000003_create_macro;
mod m20260407_000001_create_channel_lens;
mod m20260407_000002_create_incident;
mod m20260407_000003_create_user_skill_profile;
mod m20260408_122101_fix_fk_on_delete_policies;
mod m20260408_211135_create_workspace_state;
mod m20260409_000001_create_pipeline;
mod m20260424_120000_create_capability_registry;
mod m20260425_000001_create_command_audit_log;
mod m20260425_000002_audit_log_add_check_outcome_columns;
mod m20260426_180614_macro_add_ai_columns;
mod m20260426_200000_capability_registry_add_is_self_scoped;
mod m20260427_000001_audit_outcome_widen_check;
mod m20260428_182238_add_registration_invites;
mod m20260428_194718_add_user_email;
mod m20260708_220447_add_command_audit_log_parent_audit_id;
mod m20260708_225022_add_message_search_tsv;
mod m20260708_232445_create_workspace_ingest_token;
mod m20260708_233516_add_incident_alert_fields;
mod m20260710_094314_create_oncall_schedules;
mod m20260710_094331_add_user_timezone;
mod m20260710_111731_create_escalation_policies;
mod m20260710_111818_add_incident_escalation_and_ack_fields;
mod m20260710_111818_create_escalation_run;
mod m20261004_220805_add_workspace_feature;
mod m20261005_005743_add_channel_privacy_and_archive;
mod m20261005_024730_add_member_role;
mod m20261005_024731_add_moderation;
mod m20261005_045140_add_notification_rules;
mod m20261008_211159_add_message_change_feed;
mod m20261009_054837_add_key_identity;
mod m20261009_075648_key_identity_algorithm_agility;
mod m20261009_105834_key_only_signup;
mod m20261009_112441_add_device_link;
mod m20261009_133359_add_workspace_plugin;
mod m20261009_182741_add_e2ee_mls;
mod m20261009_214158_add_server_identity;
mod m20261009_220223_add_report_sender_device;
mod m20261009_230613_add_mls_readd_nonce;
mod m20261009_231431_drop_message_franking_key_fk;
mod m20261010_000142_add_message_franking_revision;
mod m20261010_002421_add_relay_delivery;
mod m20261010_010132_add_remote_workspaces;
mod m20261010_010907_add_relay_abuse;
mod m20261010_143053_make_instance_invite_creator_optional;
mod m20261010_153941_add_command_audit_log_created_at_index;
mod m20261010_184831_drop_incident_module;
mod m20261010_200459_drop_franking_key;
mod m20261010_205037_drop_redundant_indexes;
mod m20261010_205121_fold_bootstrap_tokens_into_instance_invites;
mod m20261011_141814_widen_message_text;
mod m20261012_140916_add_terms_acceptance_to_user;
mod m20261012_142739_add_ip_block;
mod m20261012_173710_add_user_invite_chain;
mod m20261012_173711_add_blocked_email;
mod m20261012_174304_add_account_deletion;
mod m20261013_055701_add_instance_operator_to_user;
mod m20261015_175228_add_conversation_member_pending;
mod m20261016_223549_add_workspace_dm_policy;
mod m20261016_223550_add_user_dm_settings;
mod m20261017_195236_add_reaction_notification_reason;
mod m20261017_195237_add_reaction_notification_fields;
mod m20261019_143809_add_user_username_lower_unique;
mod m20261019_182336_user_activity;
mod m20261019_194441_add_key_challenge_purpose;
mod m20261019_214438_add_relay_dm_request_host_user;
mod m20261019_225841_add_peer_delivery_failing_since;
mod m20261019_230105_add_peer_pin_forgotten;
mod m20261020_204601_add_instance_default_workspace;
mod m20261020_204805_add_minimum_age_confirmed_to_user;
mod m20261020_204850_add_user_handles;
mod m20261021_222259_add_relay_abuse_report_unique_request;
mod m20261022_062917_add_conversation_member_invited_by;
mod m20261023_191039_add_public_join_link;
mod m20261023_211807_add_remote_join_requests;
mod m20261024_110117_add_account_password;
mod m20261024_163804_drop_session_must_set_password;
mod m20261024_174153_remove_password_accounts;
mod m20261026_120000_add_conversation_posting;
mod m20261026_120342_add_user_profile_locked_until;
mod m20261029_084741_add_message_flag;
