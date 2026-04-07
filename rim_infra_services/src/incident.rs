use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;

use rim_core::conversation::repository::ConversationRepository;
use rim_core::incident::model::{
    IncidentPhaseName, IncidentStatus, NewIncident, NewIncidentMessageTag,
};
use rim_core::incident::repository::{
    IncidentMessageTagRepository, IncidentPhaseRepository, IncidentRepository,
};
use rim_services::incident::command::{
    AdvancePhaseCmd, GeneratePostmortemCmd, GetActiveIncidentQuery, GetIncidentTimelineQuery,
    ResolveIncidentCmd, StartIncidentCmd, TagMessageCmd, UpdateIncidentStatusCmd,
};
use rim_services::incident::dto::{
    IncidentDto, IncidentMessageTagDto, IncidentPhaseDto, IncidentTimelineDto,
};
use rim_services::incident::service::IncidentService;
use rim_services::{ServiceError, ServiceResult, map_repo_error};

#[derive(Clone)]
pub struct IncidentRepos {
    pub incident_repo: Arc<dyn IncidentRepository>,
    pub incident_phase_repo: Arc<dyn IncidentPhaseRepository>,
    pub incident_message_tag_repo: Arc<dyn IncidentMessageTagRepository>,
    pub conversation_repo: Arc<dyn ConversationRepository>,
}

pub struct DefaultIncidentService {
    repos: IncidentRepos,
}

impl DefaultIncidentService {
    pub fn new(repos: IncidentRepos) -> Self {
        Self { repos }
    }
}

#[async_trait]
impl IncidentService for DefaultIncidentService {
    async fn start_incident(&self, cmd: StartIncidentCmd) -> ServiceResult<IncidentDto> {
        // Validate title
        if cmd.title.is_empty() || cmd.title.len() > 200 {
            return Err(ServiceError::Validation(
                "incident title must be between 1 and 200 characters".into(),
            ));
        }

        // Check no active incident on this conversation
        let existing = self
            .repos
            .incident_repo
            .find_active_by_conversation(cmd.conversation_id)
            .await
            .map_err(map_repo_error)?;

        if existing.is_some() {
            return Err(ServiceError::Conflict(
                "an active incident already exists on this conversation".into(),
            ));
        }

        // Get current conversation topic to preserve for restoration
        let conversation = self
            .repos
            .conversation_repo
            .get_by_id(cmd.conversation_id)
            .await
            .map_err(map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        let original_topic = conversation.topic.clone();

        // Create the incident
        let incident = self
            .repos
            .incident_repo
            .create(NewIncident {
                server_id: cmd.server_id,
                conversation_id: cmd.conversation_id,
                started_by: cmd.actor.user_id,
                severity: cmd.severity,
                title: cmd.title.clone(),
                original_topic,
            })
            .await
            .map_err(map_repo_error)?;

        // Create initial "detection" phase
        self.repos
            .incident_phase_repo
            .create(incident.id, IncidentPhaseName::Detection, None)
            .await
            .map_err(map_repo_error)?;

        // Update conversation topic to "[INCIDENT] {title}"
        let new_topic = format!("[INCIDENT] {}", cmd.title);
        self.repos
            .conversation_repo
            .update_topic(cmd.conversation_id, Some(new_topic))
            .await
            .map_err(map_repo_error)?;

        Ok(IncidentDto::from(incident))
    }

    async fn update_incident_status(
        &self,
        cmd: UpdateIncidentStatusCmd,
    ) -> ServiceResult<IncidentDto> {
        let incident = self
            .repos
            .incident_repo
            .find_by_id(cmd.incident_id)
            .await
            .map_err(map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        // Validate status transition
        match (&incident.status, &cmd.status) {
            (IncidentStatus::Active, IncidentStatus::Mitigated) => {}
            (IncidentStatus::Active, IncidentStatus::Resolved) => {}
            (IncidentStatus::Mitigated, IncidentStatus::Resolved) => {}
            (IncidentStatus::Mitigated, IncidentStatus::Active) => {}
            _ => {
                return Err(ServiceError::Validation(format!(
                    "cannot transition from {} to {}",
                    incident.status, cmd.status
                )));
            }
        }

        let now = Utc::now();
        let mitigated_at = match cmd.status {
            IncidentStatus::Mitigated => Some(now),
            _ => incident.mitigated_at,
        };
        let resolved_at = match cmd.status {
            IncidentStatus::Resolved => Some(now),
            _ => incident.resolved_at,
        };

        let updated = self
            .repos
            .incident_repo
            .update_status(cmd.incident_id, cmd.status, mitigated_at, resolved_at)
            .await
            .map_err(map_repo_error)?;

        Ok(IncidentDto::from(updated))
    }

    async fn advance_phase(&self, cmd: AdvancePhaseCmd) -> ServiceResult<IncidentPhaseDto> {
        let incident = self
            .repos
            .incident_repo
            .find_by_id(cmd.incident_id)
            .await
            .map_err(map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        if incident.status == IncidentStatus::Resolved {
            return Err(ServiceError::Validation(
                "cannot advance phase on a resolved incident".into(),
            ));
        }

        // End current phase
        let current_phase = self
            .repos
            .incident_phase_repo
            .find_current_phase(cmd.incident_id)
            .await
            .map_err(map_repo_error)?;

        if let Some(current) = current_phase {
            self.repos
                .incident_phase_repo
                .end_phase(current.id)
                .await
                .map_err(map_repo_error)?;
        }

        // Create next phase
        let new_phase = self
            .repos
            .incident_phase_repo
            .create(cmd.incident_id, cmd.next_phase, cmd.note)
            .await
            .map_err(map_repo_error)?;

        Ok(IncidentPhaseDto::from(new_phase))
    }

    async fn tag_message(&self, cmd: TagMessageCmd) -> ServiceResult<IncidentMessageTagDto> {
        let incident = self
            .repos
            .incident_repo
            .find_by_id(cmd.incident_id)
            .await
            .map_err(map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        // Determine current phase
        let current_phase = self
            .repos
            .incident_phase_repo
            .find_current_phase(incident.id)
            .await
            .map_err(map_repo_error)?
            .map(|p| p.phase)
            .unwrap_or(IncidentPhaseName::Detection);

        let tag = self
            .repos
            .incident_message_tag_repo
            .create(NewIncidentMessageTag {
                message_id: cmd.message_id,
                incident_id: cmd.incident_id,
                phase: current_phase,
                tag_type: cmd.tag_type,
            })
            .await
            .map_err(map_repo_error)?;

        Ok(IncidentMessageTagDto::from(tag))
    }

    async fn generate_postmortem(&self, cmd: GeneratePostmortemCmd) -> ServiceResult<IncidentDto> {
        let incident = self
            .repos
            .incident_repo
            .find_by_id(cmd.incident_id)
            .await
            .map_err(map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        let phases = self
            .repos
            .incident_phase_repo
            .list_by_incident(incident.id)
            .await
            .map_err(map_repo_error)?;

        let tags = self
            .repos
            .incident_message_tag_repo
            .list_by_incident(incident.id)
            .await
            .map_err(map_repo_error)?;

        // Build template-based postmortem markdown
        let mut md = String::new();
        md.push_str(&format!("# Incident Postmortem: {}\n\n", incident.title));

        if let Some(sev) = &incident.severity {
            md.push_str(&format!("**Severity:** {}\n", sev));
        }
        md.push_str(&format!("**Started:** {}\n", incident.started_at));
        if let Some(at) = incident.mitigated_at {
            md.push_str(&format!("**Mitigated:** {}\n", at));
        }
        if let Some(at) = incident.resolved_at {
            md.push_str(&format!("**Resolved:** {}\n", at));
        }
        md.push('\n');

        for phase in &phases {
            md.push_str(&format!("## Phase: {}\n\n", phase.phase));
            md.push_str(&format!("- **Started:** {}\n", phase.started_at));
            if let Some(ended) = phase.ended_at {
                md.push_str(&format!("- **Ended:** {}\n", ended));
            }
            if let Some(note) = &phase.note {
                md.push_str(&format!("- **Note:** {}\n", note));
            }
            md.push('\n');

            // List tagged messages for this phase
            let phase_tags: Vec<_> = tags.iter().filter(|t| t.phase == phase.phase).collect();
            if !phase_tags.is_empty() {
                md.push_str("### Tagged Messages\n\n");
                for tag in phase_tags {
                    let tag_label = tag
                        .tag_type
                        .as_ref()
                        .map(|t| t.as_str())
                        .unwrap_or("note");
                    md.push_str(&format!(
                        "- [{}] message_id: {}\n",
                        tag_label, tag.message_id
                    ));
                }
                md.push('\n');
            }
        }

        let updated = self
            .repos
            .incident_repo
            .update_postmortem_draft(incident.id, md)
            .await
            .map_err(map_repo_error)?;

        Ok(IncidentDto::from(updated))
    }

    async fn resolve_incident(&self, cmd: ResolveIncidentCmd) -> ServiceResult<IncidentDto> {
        let incident = self
            .repos
            .incident_repo
            .find_by_id(cmd.incident_id)
            .await
            .map_err(map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        if incident.status == IncidentStatus::Resolved {
            return Err(ServiceError::Validation(
                "incident is already resolved".into(),
            ));
        }

        // End current phase if any
        let current_phase = self
            .repos
            .incident_phase_repo
            .find_current_phase(incident.id)
            .await
            .map_err(map_repo_error)?;

        if let Some(current) = current_phase {
            self.repos
                .incident_phase_repo
                .end_phase(current.id)
                .await
                .map_err(map_repo_error)?;
        }

        // Set status to resolved
        let now = Utc::now();
        let updated = self
            .repos
            .incident_repo
            .update_status(
                incident.id,
                IncidentStatus::Resolved,
                incident.mitigated_at,
                Some(now),
            )
            .await
            .map_err(map_repo_error)?;

        // Restore original conversation topic
        self.repos
            .conversation_repo
            .update_topic(incident.conversation_id, incident.original_topic.clone())
            .await
            .map_err(map_repo_error)?;

        Ok(IncidentDto::from(updated))
    }

    async fn get_active_incident(
        &self,
        query: GetActiveIncidentQuery,
    ) -> ServiceResult<Option<IncidentDto>> {
        let incident = self
            .repos
            .incident_repo
            .find_active_by_conversation(query.conversation_id)
            .await
            .map_err(map_repo_error)?;

        Ok(incident.map(IncidentDto::from))
    }

    async fn get_incident_timeline(
        &self,
        query: GetIncidentTimelineQuery,
    ) -> ServiceResult<IncidentTimelineDto> {
        let incident = self
            .repos
            .incident_repo
            .find_by_id(query.incident_id)
            .await
            .map_err(map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        let phases = self
            .repos
            .incident_phase_repo
            .list_by_incident(incident.id)
            .await
            .map_err(map_repo_error)?;

        let tags = self
            .repos
            .incident_message_tag_repo
            .list_by_incident(incident.id)
            .await
            .map_err(map_repo_error)?;

        Ok(IncidentTimelineDto {
            incident: IncidentDto::from(incident),
            phases: phases.into_iter().map(IncidentPhaseDto::from).collect(),
            tagged_messages: tags.into_iter().map(IncidentMessageTagDto::from).collect(),
        })
    }
}
