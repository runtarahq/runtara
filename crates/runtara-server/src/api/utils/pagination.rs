//! Page/size normalization for the paginated list endpoints.
//!
//! Pages are **0-based** across the whole runtime API: page 0 is the first page,
//! and the `number` field a page response carries back is the page that was
//! asked for, so `number + 1` fetches the next one.
//!
//! The workflow list used to be the exception — it read `page` as 1-based while
//! still reporting a 0-based `number`, so `number + 1` re-requested the page the
//! caller had just read and `?page=0` and `?page=1` both returned the first
//! page. Normalizing here keeps that from drifting apart again.

/// Validate and normalize a page number (0-based, minimum 0).
pub fn normalize_page(page: Option<i32>) -> i32 {
    page.unwrap_or(0).max(0)
}

/// Validate and normalize page size (between 1 and 100)
pub fn normalize_page_size(size: Option<i32>) -> i32 {
    size.unwrap_or(20).clamp(1, 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_page() {
        assert_eq!(normalize_page(None), 0); // Default is the first page
        assert_eq!(normalize_page(Some(0)), 0);
        assert_eq!(normalize_page(Some(1)), 1); // The *second* page
        assert_eq!(normalize_page(Some(5)), 5);
        assert_eq!(normalize_page(Some(-5)), 0); // Minimum 0
    }

    #[test]
    fn test_normalize_page_size() {
        assert_eq!(normalize_page_size(None), 20); // Default
        assert_eq!(normalize_page_size(Some(10)), 10);
        assert_eq!(normalize_page_size(Some(0)), 1); // Minimum 1
        assert_eq!(normalize_page_size(Some(-5)), 1); // Minimum 1
        assert_eq!(normalize_page_size(Some(150)), 100); // Maximum 100
    }
}
