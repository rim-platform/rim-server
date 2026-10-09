use rim_core::server::model::ServerId;

#[derive(Debug, Clone)]
pub struct ServerDto {
    pub id: ServerId,
    pub name: String,
}
