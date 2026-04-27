use std::fmt;
use std::str::FromStr;

/// Semantic verb classification for a command.
///
/// Stored as a text column in `command_audit_log.typed_verb`. Enables
/// audit queries to group by verb-class and `command.discover` to filter
/// by verb at query time without string parsing.
///
/// Variants are stable — adding a new one is fine, but renaming or removing
/// one is a breaking schema change (existing DB rows hold the `as_str()`
/// value). When adding a variant, add a matching arm in `as_str()`,
/// `from_str`, and the unit test round-trip list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TypedVerb {
    Read,
    Write,
    Delete,
    Send,
    Open,
    Close,
    Toggle,
    Navigate,
    Execute,
    Configure,
    Other,
}

impl TypedVerb {
    /// Stable lowercase string stored in the audit log.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Delete => "delete",
            Self::Send => "send",
            Self::Open => "open",
            Self::Close => "close",
            Self::Toggle => "toggle",
            Self::Navigate => "navigate",
            Self::Execute => "execute",
            Self::Configure => "configure",
            Self::Other => "other",
        }
    }
}

impl fmt::Display for TypedVerb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TypedVerb {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "read" => Ok(Self::Read),
            "write" => Ok(Self::Write),
            "delete" => Ok(Self::Delete),
            "send" => Ok(Self::Send),
            "open" => Ok(Self::Open),
            "close" => Ok(Self::Close),
            "toggle" => Ok(Self::Toggle),
            "navigate" => Ok(Self::Navigate),
            "execute" => Ok(Self::Execute),
            "configure" => Ok(Self::Configure),
            "other" => Ok(Self::Other),
            _ => Err(()),
        }
    }
}

impl From<&str> for TypedVerb {
    /// Lossy conversion — unknown strings map to `Other`.
    /// Use this at DB read boundaries. Use `FromStr` when strict parsing is needed.
    fn from(s: &str) -> Self {
        s.parse().unwrap_or(Self::Other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_variants_round_trip_through_as_str_and_from_str() {
        let variants = [
            TypedVerb::Read,
            TypedVerb::Write,
            TypedVerb::Delete,
            TypedVerb::Send,
            TypedVerb::Open,
            TypedVerb::Close,
            TypedVerb::Toggle,
            TypedVerb::Navigate,
            TypedVerb::Execute,
            TypedVerb::Configure,
            TypedVerb::Other,
        ];
        for v in variants {
            let s = v.as_str();
            let parsed: TypedVerb = s.parse().expect("as_str output must be parseable");
            assert_eq!(parsed, v, "round-trip failed for {v}");
        }
    }

    #[test]
    fn display_matches_as_str() {
        assert_eq!(TypedVerb::Write.to_string(), "write");
        assert_eq!(TypedVerb::Navigate.to_string(), "navigate");
    }

    #[test]
    fn from_str_rejects_unknown() {
        assert!("definitely_not_a_verb".parse::<TypedVerb>().is_err());
    }

    #[test]
    fn from_ref_str_maps_unknown_to_other() {
        assert_eq!(TypedVerb::from("definitely_not_a_verb"), TypedVerb::Other);
    }
}
