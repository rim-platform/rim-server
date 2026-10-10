use rim_services::command::dto::{CommandContextDto, CommandDefinitionDto, CommandSurfaceDto};
use rim_services::command::request::ListCommandsQuery;
use rim_services::command::service::CommandService;
use rim_services::{Actor, ServiceError, ServiceResult};

use super::DefaultCommandService;

impl DefaultCommandService {
    pub(super) async fn command_discover_inner(
        &self,
        actor: &Actor,
        intent_phrase: Option<String>,
        limit: Option<u32>,
        workspace_id: Option<rim_core::server::model::ServerId>,
    ) -> ServiceResult<Vec<CommandDefinitionDto>> {
        const DISCOVER_RATE_LIMIT: u32 = 5;
        const DISCOVER_KEY: &str = "command.discover";

        let decision = self.rate_limiter.check_and_record(
            actor.user_id,
            DISCOVER_KEY,
            DISCOVER_RATE_LIMIT,
            chrono::Utc::now(),
        );
        if let rim_services::command::rate_limit::RateLimitDecision::Limited {
            retry_after_seconds,
        } = decision
        {
            tracing::info!(
                target: "audit",
                event = "discover_rate_limited",
                actor_id = %actor.user_id,
                retry_after = retry_after_seconds,
            );
            crate::metrics::rate_limited("command_discover");
            return Err(ServiceError::RateLimited {
                retry_after_seconds,
            });
        }

        let context = workspace_id.map(|server_id| CommandContextDto {
            workspace_id: Some(server_id),
            conversation_id: None,
            thread_id: None,
            selected_text: None,
            selected_message_id: None,
            selected_message_ids: vec![],
            focused_surface: None,
            active_panel: None,
        });

        let mut commands = self
            .list_commands(ListCommandsQuery {
                actor: *actor,
                context,
                surface: CommandSurfaceDto::Ai,
            })
            .await?;

        if let Some(phrase) = &intent_phrase {
            let phrase_lower = phrase.to_lowercase();
            let phrase_tokens: Vec<&str> = phrase_lower.split_whitespace().collect();

            let score = |cmd: &CommandDefinitionDto| -> i32 {
                let title_lower = cmd.title.to_lowercase();
                let desc_lower = cmd
                    .nl_description
                    .as_deref()
                    .unwrap_or(&cmd.description)
                    .to_lowercase();
                let samples_lower: Vec<String> = cmd
                    .intent_samples
                    .iter()
                    .map(|s| s.to_lowercase())
                    .collect();

                let mut s: i32 = 0;

                for sample in &samples_lower {
                    if sample.contains(phrase_lower.as_str()) {
                        s += 3;
                    }
                }
                if title_lower.contains(phrase_lower.as_str()) {
                    s += 2;
                }
                if desc_lower.contains(phrase_lower.as_str()) {
                    s += 1;
                }

                let cmd_text =
                    format!("{} {} {}", title_lower, desc_lower, samples_lower.join(" "));
                for token in &phrase_tokens {
                    if cmd_text.contains(token) {
                        s += 1;
                    }
                }

                s
            };

            commands.sort_by(|a, b| score(b).cmp(&score(a)).then_with(|| a.id.cmp(&b.id)));
        }

        let cap = limit.unwrap_or(50).min(100) as usize;
        commands.truncate(cap);

        Ok(commands)
    }
}
