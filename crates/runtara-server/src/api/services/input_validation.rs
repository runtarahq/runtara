//! Compatibility re-exports for shared workflow start input validation.

pub use runtara_workflows::input_validation::{
    WorkflowInputValidationError as InputValidationError, is_empty_schema, validate_inputs,
    validate_workflow_inputs, validate_workflow_start_inputs,
};
