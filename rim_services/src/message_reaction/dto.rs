#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ReactionGroupDto {
    pub emoji: String,
    pub count: u32,
    pub reacted_by_me: bool,
}
