use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "on_call_rotation_participant")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub layer_id: Uuid,
    pub user_id: Uuid,
    pub position: i32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::on_call_rotation_layer::Entity",
        from = "Column::LayerId",
        to = "super::on_call_rotation_layer::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    OnCallRotationLayer,
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::UserId",
        to = "super::user::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    User,
}

impl Related<super::on_call_rotation_layer::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::OnCallRotationLayer.def()
    }
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
