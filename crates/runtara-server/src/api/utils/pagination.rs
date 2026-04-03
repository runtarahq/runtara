/// Validate and normalize page number (minimum 1)
pub fn normalize_page(page: Option<i32>) -> i32 {
    page.unwrap_or(1).max(1)
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
        assert_eq!(normalize_page(None), 1);
        assert_eq!(normalize_page(Some(1)), 1);
        assert_eq!(normalize_page(Some(5)), 5);
        assert_eq!(normalize_page(Some(0)), 1); // Minimum 1
        assert_eq!(normalize_page(Some(-5)), 1); // Minimum 1
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
