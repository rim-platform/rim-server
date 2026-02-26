#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SortOrder {
    Asc,
    Desc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PageDir {
    Before,
    After,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T, C> {
    pub items: Vec<T>,
    pub next: Option<C>,
    pub prev: Option<C>,
}

impl<T, C> Page<T, C> {
    pub fn new(items: Vec<T>, next: Option<C>, prev: Option<C>) -> Self {
        Self { items, next, prev }
    }
}
