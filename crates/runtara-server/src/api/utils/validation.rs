/// Validate if a string is a valid identifier (alphanumeric, hyphens, underscores)
/// Used for scenario names, connection names, etc.
pub fn is_valid_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }

    s.chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == ' ')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_valid_identifier() {
        assert!(is_valid_identifier("valid-name"));
        assert!(is_valid_identifier("valid_name"));
        assert!(is_valid_identifier("ValidName123"));
        assert!(is_valid_identifier("valid name"));
        assert!(is_valid_identifier("test-scenario-1"));

        assert!(!is_valid_identifier(""));
        assert!(!is_valid_identifier("invalid!name"));
        assert!(!is_valid_identifier("invalid@name"));
        assert!(!is_valid_identifier("invalid#name"));
    }
}
