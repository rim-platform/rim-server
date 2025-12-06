#[macro_export]
macro_rules! impl_repository_for_entity {
    (
        $repo_ty:ty,
        $model_ty:ty,
        $column_ty:ty,
        $id_ty:ty,
        $entity:ty
    ) => {
        #[async_trait]
        impl Repository<$model_ty, $id_ty> for $repo_ty {
            async fn get_by_id(&self, id: $id_ty) -> anyhow::Result<Option<$model_ty>> {
                let model = <$entity>::find_by_id(id)
                    .one(&self.db)
                    .await?;

                Ok(model)
            }

            async fn get_all(&self) -> anyhow::Result<Vec<$model_ty>> {
                let models = <$entity>::find()
                    .order_by_asc(<$column_ty>::Id)
                    .all(&self.db)
                    .await?;

                Ok(models)
            }

            async fn delete_by_id(&self, id: $id_ty) -> anyhow::Result<()> {
                <$entity>::delete_by_id(id)
                    .exec(&self.db)
                    .await?;
                Ok(())
            }
        }
    };
}