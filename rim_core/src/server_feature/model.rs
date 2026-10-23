use std::fmt;
use std::str::FromStr;

/// An optional module the operator of an instance can switch off with `RIM_FEATURES_DISABLED`.
/// Named on the wire, in configuration and in logs by the snake_case string of [`Self::as_str`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ServerFeature {
    /// File attachments on messages: the upload and download routes and the attachment
    /// queries and mutations.
    Attachments,
    /// Previews fetched from the web for links in messages.
    LinkPreviews,
    /// Outgoing email: password reset mail, invite mail and notification digests.
    Email,
    /// Workspace custom emoji.
    CustomEmoji,
    /// The workspace plugin registry and plugin bundle hosting.
    WorkspacePlugins,
    /// The surface an AI agent uses: command discovery, the agent-facing command metadata and
    /// commands or macros run with an agent initiator.
    AiCommands,
    /// Profile pictures: the avatar upload and download routes and the avatar URLs of users
    /// and members.
    Avatars,
    /// Server-side fetching of hash-pinned web plugin bundles.
    PluginLinkFetch,
}

impl ServerFeature {
    /// Every known feature, in display order.
    pub const ALL: [ServerFeature; 8] = [
        ServerFeature::Attachments,
        ServerFeature::LinkPreviews,
        ServerFeature::Email,
        ServerFeature::CustomEmoji,
        ServerFeature::WorkspacePlugins,
        ServerFeature::AiCommands,
        ServerFeature::Avatars,
        ServerFeature::PluginLinkFetch,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            ServerFeature::Attachments => "attachments",
            ServerFeature::LinkPreviews => "link_previews",
            ServerFeature::Email => "email",
            ServerFeature::CustomEmoji => "custom_emoji",
            ServerFeature::WorkspacePlugins => "workspace_plugins",
            ServerFeature::AiCommands => "ai_commands",
            ServerFeature::Avatars => "avatars",
            ServerFeature::PluginLinkFetch => "plugin_link_fetch",
        }
    }

    /// A comma-separated list of feature names, as `RIM_FEATURES_DISABLED` holds it. Spaces
    /// around names and empty entries are ignored; a name may repeat.
    ///
    /// # Errors
    /// The first name that is not a feature.
    pub fn parse_list(list: &str) -> Result<Vec<ServerFeature>, ParseServerFeatureError> {
        let mut features = Vec::new();
        for name in list.split(',').map(str::trim).filter(|n| !n.is_empty()) {
            let feature = name.parse::<ServerFeature>()?;
            if !features.contains(&feature) {
                features.push(feature);
            }
        }
        Ok(features)
    }
}

impl fmt::Display for ServerFeature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ServerFeature {
    type Err = ParseServerFeatureError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ServerFeature::ALL
            .into_iter()
            .find(|feature| feature.as_str() == s)
            .ok_or_else(|| ParseServerFeatureError::new(s))
    }
}

/// A name that is not a [`ServerFeature`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseServerFeatureError {
    input: String,
}

impl ParseServerFeatureError {
    pub fn new(input: impl Into<String>) -> Self {
        Self {
            input: input.into(),
        }
    }

    /// The name that did not parse.
    pub fn input(&self) -> &str {
        &self.input
    }
}

impl fmt::Display for ParseServerFeatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let known: Vec<&str> = ServerFeature::ALL.iter().map(|f| f.as_str()).collect();
        write!(
            f,
            "unknown feature: {} (known: {})",
            self.input,
            known.join(", ")
        )
    }
}

impl std::error::Error for ParseServerFeatureError {}

/// Whether a feature works on this instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeatureState {
    /// The feature works.
    Enabled,
    /// The operator switched it off.
    Disabled,
    /// The configuration lacks something the feature needs; `reason` says what.
    Unavailable { reason: String },
}

impl FeatureState {
    pub fn is_enabled(&self) -> bool {
        matches!(self, FeatureState::Enabled)
    }

    pub fn is_disabled(&self) -> bool {
        matches!(self, FeatureState::Disabled)
    }

    /// `enabled`, `disabled` or `unavailable`.
    pub fn as_str(&self) -> &'static str {
        match self {
            FeatureState::Enabled => "enabled",
            FeatureState::Disabled => "disabled",
            FeatureState::Unavailable { .. } => "unavailable",
        }
    }

    /// Why an unavailable feature is unavailable.
    pub fn reason(&self) -> Option<&str> {
        match self {
            FeatureState::Unavailable { reason } => Some(reason),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_str_round_trips_through_from_str() {
        for feature in ServerFeature::ALL {
            assert_eq!(feature.as_str().parse::<ServerFeature>(), Ok(feature));
            assert_eq!(feature.to_string(), feature.as_str());
        }
    }

    #[test]
    fn from_str_rejects_unknown_and_names_it() {
        let err = "billing".parse::<ServerFeature>().unwrap_err();
        assert_eq!(err.input(), "billing");
        assert!(err.to_string().starts_with("unknown feature: billing"));
        assert!(err.to_string().contains("custom_emoji"));
        assert!("Attachments".parse::<ServerFeature>().is_err());
    }

    #[test]
    fn parse_list_ignores_spaces_empty_entries_and_repeats() {
        assert_eq!(ServerFeature::parse_list(""), Ok(vec![]));
        assert_eq!(ServerFeature::parse_list(" , ,"), Ok(vec![]));
        assert_eq!(
            ServerFeature::parse_list(" attachments ,custom_emoji,, attachments "),
            Ok(vec![ServerFeature::Attachments, ServerFeature::CustomEmoji])
        );
    }

    #[test]
    fn parse_list_refuses_the_first_unknown_name() {
        let err = ServerFeature::parse_list("email, incidents, nope").unwrap_err();
        assert_eq!(err.input(), "incidents");
    }

    #[test]
    fn state_names_and_reason() {
        assert_eq!(FeatureState::Enabled.as_str(), "enabled");
        assert_eq!(FeatureState::Disabled.as_str(), "disabled");
        let unavailable = FeatureState::Unavailable {
            reason: "no SMTP".to_string(),
        };
        assert_eq!(unavailable.as_str(), "unavailable");
        assert_eq!(unavailable.reason(), Some("no SMTP"));
        assert_eq!(FeatureState::Disabled.reason(), None);
        assert!(FeatureState::Enabled.is_enabled());
        assert!(!unavailable.is_enabled() && !unavailable.is_disabled());
        assert!(FeatureState::Disabled.is_disabled());
    }
}
