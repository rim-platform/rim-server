#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub id: String,
    pub title: String,
    pub description: String,
    pub capability_ids: Vec<String>,
}
