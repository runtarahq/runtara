//! Public API DTO adapter for shared Object Model validation.
use crate::api::dto::object_model::{ColumnDefinition, IndexDefinition};
pub use runtara_object_store::validation::ValidationError;

pub struct SchemaValidator;
impl SchemaValidator {
    pub fn validate_schema(
        table_name: &str,
        columns: &[ColumnDefinition],
        indexes: &Option<Vec<IndexDefinition>>,
    ) -> Result<(), ValidationError> {
        let columns = columns.iter().cloned().map(Into::into).collect::<Vec<_>>();
        let indexes = indexes
            .as_ref()
            .map(|indexes| indexes.iter().cloned().map(Into::into).collect());
        runtara_object_store::validation::SchemaValidator::validate_schema(
            table_name, &columns, &indexes,
        )
    }
}
