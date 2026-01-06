#[macro_export]
macro_rules! impl_repository_for_entity {
    (
        $repo_ty:ty,
        $entity:ty,
        $model:ty,
        $model_id:ty
    ) => {
        #[async_trait]
        impl Repository for $repo_ty {
            type Model = $model;
            type Id = $model_id;

            async fn get_by_id(&self, id: Self::Id) -> anyhow::Result<Option<Self::Model>> {
                let user = <$entity>::find_by_id(id).one(&self.db).await?;
                Ok(user.map(Into::into))
            }

            async fn get_all(&self) -> anyhow::Result<Vec<Self::Model>> {
                let users = <$entity>::find().all(&self.db).await?;
                Ok(users.into_iter().map(Into::into).collect())
            }

            async fn delete_by_id(&self, id: Self::Id) -> anyhow::Result<()> {
                <$entity>::delete_by_id(id).exec(&self.db).await?;
                Ok(())
            }
        }
    };
}
