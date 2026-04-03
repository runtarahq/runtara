//! SQL Identifier Sanitization Utilities
//!
//! Provides functions to safely sanitize and quote SQL identifiers to prevent SQL injection.

use regex::Regex;

/// PostgreSQL reserved keywords that cannot be used as unquoted identifiers
pub const POSTGRES_RESERVED_WORDS: &[&str] = &[
    "ALL",
    "ANALYSE",
    "ANALYZE",
    "AND",
    "ANY",
    "ARRAY",
    "AS",
    "ASC",
    "ASYMMETRIC",
    "BOTH",
    "CASE",
    "CAST",
    "CHECK",
    "COLLATE",
    "COLUMN",
    "CONSTRAINT",
    "CREATE",
    "CURRENT_CATALOG",
    "CURRENT_DATE",
    "CURRENT_ROLE",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "CURRENT_USER",
    "DEFAULT",
    "DEFERRABLE",
    "DESC",
    "DISTINCT",
    "DO",
    "ELSE",
    "END",
    "EXCEPT",
    "FALSE",
    "FETCH",
    "FOR",
    "FOREIGN",
    "FROM",
    "GRANT",
    "GROUP",
    "HAVING",
    "IN",
    "INITIALLY",
    "INTERSECT",
    "INTO",
    "LATERAL",
    "LEADING",
    "LIMIT",
    "LOCALTIME",
    "LOCALTIMESTAMP",
    "NOT",
    "NULL",
    "OFFSET",
    "ON",
    "ONLY",
    "OR",
    "ORDER",
    "PLACING",
    "PRIMARY",
    "REFERENCES",
    "RETURNING",
    "SELECT",
    "SESSION_USER",
    "SOME",
    "SYMMETRIC",
    "TABLE",
    "THEN",
    "TO",
    "TRAILING",
    "TRUE",
    "UNION",
    "UNIQUE",
    "USER",
    "USING",
    "VARIADIC",
    "WHEN",
    "WHERE",
    "WINDOW",
    "WITH",
];

/// Quote a SQL identifier to make it safe for use in queries
///
/// # Arguments
/// * `identifier` - The identifier to quote
///
/// # Returns
/// The identifier wrapped in double quotes with escaped internal quotes
///
/// # Example
/// ```
/// use runtara_object_store::sql::quote_identifier;
///
/// let quoted = quote_identifier("my_table");
/// assert_eq!(quoted, "\"my_table\"");
/// ```
pub fn quote_identifier(identifier: &str) -> String {
    // Escape any double quotes in the identifier by doubling them
    let escaped = identifier.replace('"', "\"\"");
    format!("\"{}\"", escaped)
}

/// Validate a table or column name
///
/// Rules:
/// - Must start with a letter (a-z)
/// - Can only contain lowercase letters, numbers, and underscores
/// - Cannot be a PostgreSQL reserved word
/// - Cannot be an auto-managed column name (for columns)
///
/// # Arguments
/// * `name` - The name to validate
/// * `reserved_columns` - List of reserved column names to check against (e.g., auto-managed columns)
///
/// # Returns
/// Ok(()) if valid, Err with message if invalid
///
/// # Example
/// ```
/// use runtara_object_store::sql::validate_identifier;
///
/// assert!(validate_identifier("products", &[]).is_ok());
/// assert!(validate_identifier("select", &[]).is_err()); // reserved keyword
/// assert!(validate_identifier("id", &["id", "created_at"]).is_err()); // reserved column
/// ```
pub fn validate_identifier(name: &str, reserved_columns: &[&str]) -> Result<(), String> {
    // Check empty
    if name.is_empty() {
        return Err("Identifier cannot be empty".to_string());
    }

    // Check pattern: must start with letter, only lowercase alphanumeric + underscore
    let re = Regex::new(r"^[a-z][a-z0-9_]*$").unwrap();
    if !re.is_match(name) {
        return Err(format!(
            "Identifier '{}' is invalid. Must start with a lowercase letter and contain only lowercase letters, numbers, and underscores.",
            name
        ));
    }

    // Check reserved keywords
    if POSTGRES_RESERVED_WORDS.contains(&name.to_uppercase().as_str()) {
        return Err(format!(
            "Identifier '{}' is a PostgreSQL reserved keyword and cannot be used.",
            name
        ));
    }

    // Check reserved columns
    if reserved_columns.contains(&name) {
        return Err(format!(
            "Column name '{}' is reserved and cannot be used.",
            name
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // quote_identifier Tests
    // =========================================================================

    #[test]
    fn test_quote_identifier_simple() {
        assert_eq!(quote_identifier("my_table"), "\"my_table\"");
        assert_eq!(quote_identifier("users"), "\"users\"");
        assert_eq!(quote_identifier("a"), "\"a\"");
    }

    #[test]
    fn test_quote_identifier_with_quotes() {
        assert_eq!(
            quote_identifier("table\"with\"quotes"),
            "\"table\"\"with\"\"quotes\""
        );
        assert_eq!(quote_identifier("\"quoted\""), "\"\"\"quoted\"\"\"");
    }

    #[test]
    fn test_quote_identifier_with_spaces() {
        assert_eq!(quote_identifier("my table"), "\"my table\"");
        assert_eq!(quote_identifier("column name"), "\"column name\"");
    }

    #[test]
    fn test_quote_identifier_with_special_chars() {
        assert_eq!(quote_identifier("my-table"), "\"my-table\"");
        assert_eq!(quote_identifier("my.table"), "\"my.table\"");
        assert_eq!(quote_identifier("my@table"), "\"my@table\"");
    }

    #[test]
    fn test_quote_identifier_unicode() {
        assert_eq!(quote_identifier("日本語"), "\"日本語\"");
        assert_eq!(quote_identifier("tëst"), "\"tëst\"");
    }

    #[test]
    fn test_quote_identifier_empty() {
        assert_eq!(quote_identifier(""), "\"\"");
    }

    #[test]
    fn test_quote_identifier_reserved_keyword() {
        // Even reserved keywords should be safely quoted
        assert_eq!(quote_identifier("select"), "\"select\"");
        assert_eq!(quote_identifier("table"), "\"table\"");
    }

    // =========================================================================
    // validate_identifier Valid Cases Tests
    // =========================================================================

    #[test]
    fn test_validate_identifier_valid_simple() {
        assert!(validate_identifier("products", &[]).is_ok());
        assert!(validate_identifier("users", &[]).is_ok());
        assert!(validate_identifier("items", &[]).is_ok());
    }

    #[test]
    fn test_validate_identifier_valid_with_numbers() {
        assert!(validate_identifier("table1", &[]).is_ok());
        assert!(validate_identifier("my_table_123", &[]).is_ok());
        assert!(validate_identifier("a1b2c3", &[]).is_ok());
    }

    #[test]
    fn test_validate_identifier_valid_with_underscores() {
        assert!(validate_identifier("my_table", &[]).is_ok());
        assert!(validate_identifier("my_long_table_name", &[]).is_ok());
        assert!(validate_identifier("a_b_c", &[]).is_ok());
    }

    #[test]
    fn test_validate_identifier_valid_single_char() {
        assert!(validate_identifier("a", &[]).is_ok());
        assert!(validate_identifier("x", &[]).is_ok());
        assert!(validate_identifier("z", &[]).is_ok());
    }

    // =========================================================================
    // validate_identifier Invalid Cases Tests
    // =========================================================================

    #[test]
    fn test_validate_identifier_empty() {
        let result = validate_identifier("", &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cannot be empty"));
    }

    #[test]
    fn test_validate_identifier_starts_with_number() {
        assert!(validate_identifier("1products", &[]).is_err());
        assert!(validate_identifier("123table", &[]).is_err());
        assert!(validate_identifier("0abc", &[]).is_err());
    }

    #[test]
    fn test_validate_identifier_starts_with_underscore() {
        assert!(validate_identifier("_products", &[]).is_err());
        assert!(validate_identifier("__table", &[]).is_err());
    }

    #[test]
    fn test_validate_identifier_uppercase() {
        assert!(validate_identifier("Products", &[]).is_err());
        assert!(validate_identifier("USERS", &[]).is_err());
        assert!(validate_identifier("myTable", &[]).is_err());
    }

    #[test]
    fn test_validate_identifier_hyphen() {
        assert!(validate_identifier("my-table", &[]).is_err());
        assert!(validate_identifier("kebab-case", &[]).is_err());
    }

    #[test]
    fn test_validate_identifier_dot() {
        assert!(validate_identifier("my.table", &[]).is_err());
        assert!(validate_identifier("schema.table", &[]).is_err());
    }

    #[test]
    fn test_validate_identifier_space() {
        assert!(validate_identifier("my table", &[]).is_err());
        assert!(validate_identifier(" table", &[]).is_err());
        assert!(validate_identifier("table ", &[]).is_err());
    }

    #[test]
    fn test_validate_identifier_special_chars() {
        assert!(validate_identifier("my@table", &[]).is_err());
        assert!(validate_identifier("my#table", &[]).is_err());
        assert!(validate_identifier("my$table", &[]).is_err());
        assert!(validate_identifier("my!table", &[]).is_err());
    }

    // =========================================================================
    // validate_identifier Reserved Keywords Tests
    // =========================================================================

    #[test]
    fn test_validate_identifier_reserved_select() {
        let result = validate_identifier("select", &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("reserved keyword"));
    }

    #[test]
    fn test_validate_identifier_reserved_table() {
        assert!(validate_identifier("table", &[]).is_err());
    }

    #[test]
    fn test_validate_identifier_reserved_various() {
        assert!(validate_identifier("user", &[]).is_err());
        assert!(validate_identifier("where", &[]).is_err());
        assert!(validate_identifier("from", &[]).is_err());
        assert!(validate_identifier("order", &[]).is_err());
        assert!(validate_identifier("group", &[]).is_err());
    }

    // =========================================================================
    // validate_identifier Reserved Columns Tests
    // =========================================================================

    #[test]
    fn test_validate_identifier_reserved_columns() {
        let reserved = &["id", "created_at", "updated_at", "deleted"];

        assert!(validate_identifier("id", reserved).is_err());
        assert!(validate_identifier("created_at", reserved).is_err());
        assert!(validate_identifier("updated_at", reserved).is_err());
        assert!(validate_identifier("deleted", reserved).is_err());
    }

    #[test]
    fn test_validate_identifier_not_reserved_when_empty() {
        // Same names should be OK when reserved list is empty
        assert!(validate_identifier("id", &[]).is_ok());
        assert!(validate_identifier("created_at", &[]).is_ok());
        assert!(validate_identifier("updated_at", &[]).is_ok());
        assert!(validate_identifier("deleted", &[]).is_ok());
    }

    #[test]
    fn test_validate_identifier_reserved_column_error_message() {
        let result = validate_identifier("id", &["id"]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("reserved"));
    }

    // =========================================================================
    // POSTGRES_RESERVED_WORDS Tests
    // =========================================================================

    #[test]
    fn test_reserved_words_contains_common_keywords() {
        assert!(POSTGRES_RESERVED_WORDS.contains(&"SELECT"));
        assert!(POSTGRES_RESERVED_WORDS.contains(&"FROM"));
        assert!(POSTGRES_RESERVED_WORDS.contains(&"WHERE"));
        assert!(POSTGRES_RESERVED_WORDS.contains(&"TABLE"));
        assert!(POSTGRES_RESERVED_WORDS.contains(&"CREATE"));
    }

    #[test]
    fn test_reserved_words_not_empty() {
        // POSTGRES_RESERVED_WORDS is a const, so we just validate it has sufficient entries
        assert!(POSTGRES_RESERVED_WORDS.len() > 50); // Should have many reserved words
    }
}
