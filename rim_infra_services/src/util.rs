use rim_core::conversation::model::ConversationId;
use rim_core::server::model::ServerId;
use rim_core::server::repository::ServerRepository;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_services::{ServiceError, ServiceResult};

pub async fn ensure_server_member(
    server_member_repo: &dyn ServerMemberRepository,
    server_id: ServerId,
    user_id: rim_core::user::model::UserId,
) -> ServiceResult<()> {
    let is_member = server_member_repo
        .is_member(server_id, user_id)
        .await
        .map_err(rim_services::map_repo_error)?;

    if !is_member {
        return Err(ServiceError::PermissionDenied);
    }

    Ok(())
}

pub async fn ensure_server_owner(
    server_member_repo: &dyn ServerMemberRepository,
    server_id: ServerId,
    user_id: rim_core::user::model::UserId,
) -> ServiceResult<()> {
    let member = server_member_repo
        .get_member(server_id, user_id)
        .await
        .map_err(rim_services::map_repo_error)?
        .ok_or(ServiceError::PermissionDenied)?;

    if member.member_role != ServerMemberRole::Owner {
        return Err(ServiceError::PermissionDenied);
    }

    Ok(())
}

pub async fn ensure_server_exists(
    server_repo: &dyn ServerRepository,
    server_id: ServerId,
) -> ServiceResult<rim_core::server::model::Server> {
    let server = server_repo
        .get_by_id(server_id)
        .await
        .map_err(rim_services::map_repo_error)?
        .ok_or(ServiceError::NotFound)?;

    Ok(server)
}

pub async fn ensure_conversation_in_server(
    conversation_repo: &dyn rim_core::conversation::repository::ConversationRepository,
    conversation_id: ConversationId,
    expected_server_id: ServerId,
) -> ServiceResult<rim_core::conversation::model::Conversation> {
    let conv = conversation_repo
        .get_by_id(conversation_id)
        .await
        .map_err(rim_services::map_repo_error)?
        .ok_or(ServiceError::NotFound)?;

    if conv.server_id != expected_server_id {
        return Err(ServiceError::NotFound);
    }

    Ok(conv)
}
