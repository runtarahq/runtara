//! SQL utilities for Object Store
//!
//! Provides SQL generation, sanitization, and query building utilities.

pub mod aggregate;
pub mod condition;
pub mod ddl;
pub mod expr;
pub mod sanitize;

pub use aggregate::{
    AggregateFn, AggregateOrderBy, AggregateRequest, AggregateResult, AggregateSpec, AggregateSql,
    SortDirection, build_aggregate_query, build_aggregate_query_with_subqueries,
};
pub use condition::{
    build_condition_clause, build_condition_clause_with_subqueries, build_order_by_clause,
    collect_condition_subquery_schema_names,
};
pub use ddl::DdlGenerator;
pub use expr::{ExprNode, ExprOp, ExprOperation, ExprValue};
pub use sanitize::{POSTGRES_RESERVED_WORDS, quote_identifier, validate_identifier};
