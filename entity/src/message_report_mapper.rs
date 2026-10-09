use crate::{message_report, sea_orm_active_enums};
use rim_core::moderation::model::{FrankedReport, MessageReport, MessageReportStatus};

impl From<message_report::Model> for MessageReport {
    fn from(m: message_report::Model) -> Self {
        Self {
            id: m.id.into(),
            server_id: m.server_id.into(),
            conversation_id: m.conversation_id.into(),
            message_id: m.message_id.into(),
            reporter_user_id: m.reporter_user_id.into(),
            reason: m.reason,
            status: m.status.into(),
            resolved_by: m.resolved_by.map(Into::into),
            resolved_at: m.resolved_at.map(Into::into),
            created_at: m.created_at.into(),
            franked: m.franking_verified_at.map(|verified_at| FrankedReport {
                verified_at: verified_at.into(),
                content: m.franked_content,
                sender_device_id: m.franked_sender_device_id,
            }),
        }
    }
}

impl From<sea_orm_active_enums::MessageReportStatus> for MessageReportStatus {
    fn from(value: sea_orm_active_enums::MessageReportStatus) -> Self {
        match value {
            sea_orm_active_enums::MessageReportStatus::Open => MessageReportStatus::Open,
            sea_orm_active_enums::MessageReportStatus::Resolved => MessageReportStatus::Resolved,
            sea_orm_active_enums::MessageReportStatus::Dismissed => MessageReportStatus::Dismissed,
        }
    }
}

impl From<MessageReportStatus> for sea_orm_active_enums::MessageReportStatus {
    fn from(value: MessageReportStatus) -> Self {
        match value {
            MessageReportStatus::Open => sea_orm_active_enums::MessageReportStatus::Open,
            MessageReportStatus::Resolved => sea_orm_active_enums::MessageReportStatus::Resolved,
            MessageReportStatus::Dismissed => sea_orm_active_enums::MessageReportStatus::Dismissed,
        }
    }
}
