#[cfg(test)]
mod pagination {
    use crate::repository::pagination::{Page, PageDir, SortOrder};

    #[test]
    fn sort_order_is_copy_and_comparable() {
        let a = SortOrder::Asc;
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    fn page_dir_is_copy_and_comparable() {
        let a = PageDir::Before;
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    fn page_new_sets_fields() {
        let page = Page::new(vec![1, 2, 3], Some("n"), Some("p"));
        assert_eq!(page.items, vec![1, 2, 3]);
        assert_eq!(page.next, Some("n"));
        assert_eq!(page.prev, Some("p"));
    }
}
