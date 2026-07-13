use async_graphql::Enum;

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum PageDir {
    Before,
    After,
}

impl From<PageDir> for rim_core::repository::pagination::PageDir {
    fn from(value: PageDir) -> Self {
        match value {
            PageDir::Before => rim_core::repository::pagination::PageDir::Before,
            PageDir::After => rim_core::repository::pagination::PageDir::After,
        }
    }
}
