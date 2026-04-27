use rim_services::{ServiceError, ServiceResult};

use rim_services::command::dto::CommandResultDto;

/// All command IDs that are registered as `DesktopLocal` execution targets.
/// These must never be dispatched on the server — the client runtime owns them.
/// Mirrors the full set from `registry/workspace.rs` (all 19 workspace.* IDs)
/// plus the 4 desktop-local chat.* IDs from `registry/chat_messages.rs`.
pub(super) fn is_desktop_local(command_id: &str) -> bool {
    matches!(
        command_id,
        // chat.* desktop-locals (registry/chat_messages.rs)
        "chat.copy_message_text"
            | "chat.open_thread"
            | "chat.scroll_to_bottom"
            | "chat.scroll_to_top"
            // workspace.* (registry/workspace.rs — all 19)
            | "workspace.open_command_palette"
            | "workspace.cycle_recent_activity"
            | "workspace.open_settings"
            | "workspace.open_search"
            | "workspace.search_in_channel"
            | "workspace.open_marketplace"
            | "workspace.open_agent"
            | "workspace.toggle_sidebar"
            | "workspace.focus_composer"
            | "workspace.focus_message_list"
            | "workspace.navigate_to_channel"
            | "workspace.navigate_to_dm"
            | "workspace.switch_workspace"
            | "workspace.previous_channel"
            | "workspace.next_channel"
            | "workspace.next_unread_channel"
            | "workspace.close_panel"
            | "workspace.reopen_last_thread"
            | "workspace.reload_commands"
    )
}

pub(super) fn desktop_local_rejected(_command_id: &str) -> ServiceResult<CommandResultDto> {
    Err(ServiceError::Validation(
        "desktop-local commands must run in the client runtime".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rim_services::ServiceError;
    use rim_services::command::dto::CommandExecutionTargetDto;

    /// Registry-walking drift check — bidirectional.
    ///
    /// Direction 1: every command in `builtin_commands()` whose
    /// `execution_target == DesktopLocal` must be recognized by
    /// `is_desktop_local()`. Catches the failure mode where a new desktop-local
    /// command is added to the registry without updating `is_desktop_local()` —
    /// it would silently fall through to `NotFound` at runtime.
    ///
    /// Direction 2: every ID that `is_desktop_local()` claims must actually
    /// be registered as `DesktopLocal` in the registry. Catches the inverse
    /// drift — a stale entry in `is_desktop_local()` whose registry row was
    /// changed to `Backend`.
    #[test]
    fn is_desktop_local_matches_registry_execution_target_bidirectionally() {
        let commands = crate::command::registry::builtin_commands();

        let registry_desktop_local_ids: Vec<&str> = commands
            .iter()
            .filter(|c| c.execution_target == CommandExecutionTargetDto::DesktopLocal)
            .map(|c| c.id.as_str())
            .collect();

        // Direction 1: registry DesktopLocal → is_desktop_local() must agree.
        for id in &registry_desktop_local_ids {
            assert!(
                is_desktop_local(id),
                "registry has `{id}` as DesktopLocal but is_desktop_local() returned false — \
                 add it to the matches! list in is_desktop_local()"
            );
        }

        // Direction 2: is_desktop_local() claims → must be DesktopLocal in registry.
        // Walk ALL commands; for any backend command, is_desktop_local() must return false.
        let registry_backend_ids: Vec<&str> = commands
            .iter()
            .filter(|c| c.execution_target != CommandExecutionTargetDto::DesktopLocal)
            .map(|c| c.id.as_str())
            .collect();

        for id in &registry_backend_ids {
            assert!(
                !is_desktop_local(id),
                "is_desktop_local() claims `{id}` is desktop-local but registry says Backend — \
                 remove it from the matches! list in is_desktop_local()"
            );
        }
    }

    /// `desktop_local_rejected` must return `Validation`, not `NotFound`, for
    /// every desktop-local command. Callers depend on this distinction to give
    /// useful error messages to clients.
    #[test]
    fn desktop_local_rejected_returns_validation_error() {
        let commands = crate::command::registry::builtin_commands();
        for cmd in commands
            .iter()
            .filter(|c| c.execution_target == CommandExecutionTargetDto::DesktopLocal)
        {
            let result = desktop_local_rejected(&cmd.id);
            assert!(
                matches!(result, Err(ServiceError::Validation(_))),
                "desktop_local_rejected returned non-Validation error for `{}`",
                cmd.id
            );
        }
    }
}
