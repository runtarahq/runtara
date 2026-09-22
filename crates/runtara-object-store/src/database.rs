//! Native implementation of the driver-independent SQL contract.
//! No Object Model operation names cross this boundary.
use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use futures_util::TryStreamExt;
use runtara_database_contract::*;
use sqlx::postgres::{PgArguments, PgColumn, PgRow, PgTypeInfo};
use sqlx::{
    Column as _, Connection, Either, Executor, Postgres, Row, Statement as _, Type, TypeInfo,
    ValueRef,
};

use crate::ObjectStore;

#[derive(Debug, Clone, Copy)]
pub struct DatabaseLimits {
    pub statement_timeout: Duration,
    pub operation_timeout: Duration,
    pub max_rows: usize,
    pub max_response_bytes: usize,
    pub max_batch_statements: usize,
}

impl Default for DatabaseLimits {
    fn default() -> Self {
        Self {
            statement_timeout: Duration::from_secs(60),
            operation_timeout: Duration::from_secs(60),
            max_rows: 10_000,
            max_response_bytes: MAX_RESPONSE_BYTES,
            max_batch_statements: MAX_BATCH_STATEMENTS,
        }
    }
}

impl DatabaseLimits {
    fn check(self) -> Result<(), DatabaseError> {
        if self.statement_timeout.is_zero()
            || self.operation_timeout.is_zero()
            || self.max_rows == 0
            || self.max_response_bytes == 0
            || self.max_batch_statements == 0
        {
            return Err(DatabaseError::invalid("Database limits must be positive"));
        }
        Ok(())
    }
    fn response_limit(self) -> usize {
        self.max_response_bytes.min(MAX_RESPONSE_BYTES)
    }
}

fn driver_error(error: sqlx::Error) -> DatabaseError {
    let sqlstate = error
        .as_database_error()
        .and_then(|e| e.code())
        .map(|s| s.into_owned());
    let retryable = sqlstate
        .as_deref()
        .is_some_and(|s| matches!(s, "40001" | "40P01"))
        || matches!(error, sqlx::Error::PoolTimedOut | sqlx::Error::Io(_));
    DatabaseError {
        code: "DATABASE_EXECUTION_FAILED".into(),
        message: "Database operation failed".into(),
        outcome: Outcome::Unknown,
        retryable,
        sqlstate,
        statement_index: None,
    }
}

fn result_too_large(message: impl Into<String>) -> DatabaseError {
    DatabaseError {
        code: "DATABASE_RESULT_TOO_LARGE".into(),
        ..DatabaseError::invalid(message)
    }
}

fn timeout_error(read_only: bool) -> DatabaseError {
    DatabaseError {
        code: "DATABASE_DEADLINE_EXCEEDED".into(),
        message: "Database operation deadline exceeded".into(),
        outcome: if read_only {
            Outcome::RolledBack
        } else {
            Outcome::Unknown
        },
        retryable: read_only,
        sqlstate: None,
        statement_index: None,
    }
}

fn check_request<T: serde::Serialize>(request: &T) -> Result<(), DatabaseError> {
    check_size(
        request,
        MAX_REQUEST_BYTES,
        "Database request exceeds the byte limit",
    )
}

fn check_size<T: serde::Serialize>(
    value: &T,
    limit: usize,
    message: &str,
) -> Result<(), DatabaseError> {
    // Count serialization without allocating a second complete payload.
    struct Counter {
        bytes: usize,
        limit: usize,
    }
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes = self.bytes.saturating_add(bytes.len());
            if self.bytes > self.limit {
                return Err(std::io::Error::other("size limit"));
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Counter { bytes: 0, limit }, value)
        .map_err(|_| DatabaseError::invalid(message))
}

fn sql_type(value: &SqlValue) -> SqlType {
    match value {
        SqlValue::Null(t) => t.clone(),
        SqlValue::Text(_) => SqlType::Text,
        SqlValue::Integer(_) => SqlType::Integer,
        SqlValue::Decimal(_) => SqlType::Decimal,
        SqlValue::Float(_) => SqlType::Float,
        SqlValue::Boolean(_) => SqlType::Boolean,
        SqlValue::Json(_) => SqlType::Json,
        SqlValue::Timestamp(_) => SqlType::Timestamp,
        SqlValue::TimestampTz(_) => SqlType::TimestampTz,
        SqlValue::Date(_) => SqlType::Date,
        SqlValue::Time(_) => SqlType::Time,
        SqlValue::Uuid(_) => SqlType::Uuid,
        SqlValue::Vector(_) => SqlType::Vector,
    }
}

fn type_info(kind: &SqlType) -> PgTypeInfo {
    match kind {
        SqlType::Text => <String as Type<Postgres>>::type_info(),
        SqlType::Integer => <i64 as Type<Postgres>>::type_info(),
        SqlType::Decimal => <sqlx::types::BigDecimal as Type<Postgres>>::type_info(),
        SqlType::Float => <f64 as Type<Postgres>>::type_info(),
        SqlType::Boolean => <bool as Type<Postgres>>::type_info(),
        SqlType::Json => <serde_json::Value as Type<Postgres>>::type_info(),
        SqlType::Timestamp => <NaiveDateTime as Type<Postgres>>::type_info(),
        SqlType::TimestampTz => <DateTime<Utc> as Type<Postgres>>::type_info(),
        SqlType::Date => <NaiveDate as Type<Postgres>>::type_info(),
        SqlType::Time => <NaiveTime as Type<Postgres>>::type_info(),
        SqlType::Uuid => <uuid::Uuid as Type<Postgres>>::type_info(),
        SqlType::Vector => <pgvector::Vector as Type<Postgres>>::type_info(),
    }
}

fn parse_decimal(value: &str) -> Result<sqlx::types::BigDecimal, DatabaseError> {
    // Bound expansion before SQLx encodes NUMERIC. Huge exponents must not cause
    // unbounded allocation in the driver even when the input string is tiny.
    if value.len() > 131_100 {
        return Err(DatabaseError::invalid(
            "Decimal exceeds PostgreSQL numeric limits",
        ));
    }
    let decimal = sqlx::types::BigDecimal::from_str(value)
        .map_err(|_| DatabaseError::invalid("Invalid decimal"))?;
    let (digits, scale) = decimal.as_bigint_and_exponent();
    let digits_len = digits.to_string().trim_start_matches('-').len() as i64;
    if !(-131_072..=16_383).contains(&scale) || digits_len.saturating_sub(scale) > 131_072 {
        return Err(DatabaseError::invalid(
            "Decimal exceeds PostgreSQL numeric limits",
        ));
    }
    Ok(decimal)
}

pub(crate) fn bind<'q>(
    mut query: sqlx::query::Query<'q, Postgres, PgArguments>,
    values: &[SqlValue],
) -> Result<sqlx::query::Query<'q, Postgres, PgArguments>, DatabaseError> {
    for (index, value) in values.iter().enumerate() {
        let invalid =
            || DatabaseError::invalid(format!("Invalid value for parameter ${}", index + 1));
        query = match value {
            SqlValue::Null(kind) => match kind {
                SqlType::Text => query.bind(None::<String>),
                SqlType::Integer => query.bind(None::<i64>),
                SqlType::Decimal => query.bind(None::<sqlx::types::BigDecimal>),
                SqlType::Float => query.bind(None::<f64>),
                SqlType::Boolean => query.bind(None::<bool>),
                SqlType::Json => query.bind(None::<serde_json::Value>),
                SqlType::Timestamp => query.bind(None::<NaiveDateTime>),
                SqlType::TimestampTz => query.bind(None::<DateTime<Utc>>),
                SqlType::Date => query.bind(None::<NaiveDate>),
                SqlType::Time => query.bind(None::<NaiveTime>),
                SqlType::Uuid => query.bind(None::<uuid::Uuid>),
                SqlType::Vector => query.bind(None::<pgvector::Vector>),
            },
            SqlValue::Text(v) => query.bind(v.clone()),
            SqlValue::Integer(v) => query.bind(v.parse::<i64>().map_err(|_| invalid())?),
            // Exact parser rejects values outside the supported decimal range.
            SqlValue::Decimal(v) => query.bind(parse_decimal(v).map_err(|_| invalid())?),
            SqlValue::Float(v) if v.is_finite() => query.bind(*v),
            SqlValue::Float(_) => return Err(invalid()),
            SqlValue::Boolean(v) => query.bind(*v),
            SqlValue::Json(v) => query.bind(v.clone()),
            SqlValue::Timestamp(v) => query.bind(
                NaiveDateTime::from_str(v)
                    .or_else(|_| NaiveDateTime::parse_from_str(v, "%Y-%m-%d %H:%M:%S%.f"))
                    .map_err(|_| invalid())?,
            ),
            SqlValue::TimestampTz(v) => query.bind(
                DateTime::parse_from_rfc3339(v)
                    .map_err(|_| invalid())?
                    .with_timezone(&Utc),
            ),
            SqlValue::Date(v) => query.bind(NaiveDate::from_str(v).map_err(|_| invalid())?),
            SqlValue::Time(v) => query.bind(NaiveTime::from_str(v).map_err(|_| invalid())?),
            SqlValue::Uuid(v) => query.bind(uuid::Uuid::parse_str(v).map_err(|_| invalid())?),
            SqlValue::Vector(v) => {
                if v.is_empty() || v.len() > 16_000 || v.iter().any(|f| !f.is_finite()) {
                    return Err(invalid());
                }
                query.bind(pgvector::Vector::from(v.clone()))
            }
        };
    }
    Ok(query)
}

fn infer_type(column: &PgColumn) -> Result<SqlType, DatabaseError> {
    Ok(match column.type_info().name() {
        "TEXT" | "VARCHAR" | "CHAR" | "BPCHAR" | "NAME" => SqlType::Text,
        "INT2" | "INT4" | "INT8" => SqlType::Integer,
        "NUMERIC" => SqlType::Decimal,
        "FLOAT4" | "FLOAT8" => SqlType::Float,
        "BOOL" => SqlType::Boolean,
        "JSON" | "JSONB" => SqlType::Json,
        "TIMESTAMP" => SqlType::Timestamp,
        "TIMESTAMPTZ" => SqlType::TimestampTz,
        "DATE" => SqlType::Date,
        "TIME" => SqlType::Time,
        "UUID" => SqlType::Uuid,
        name if name.eq_ignore_ascii_case("vector") => SqlType::Vector,
        _ => {
            return Err(DatabaseError::invalid(
                "Unsupported SQL result type; cast to a supported type",
            ));
        }
    })
}

struct Projection {
    index: usize,
    column: Column,
    nullable: bool,
}

fn projection(columns: &[PgColumn], spec: &ResultSpec) -> Result<Vec<Projection>, DatabaseError> {
    match spec {
        ResultSpec::Selected(names) => {
            let requested = names
                .iter()
                .map(|name| {
                    let column = columns
                        .iter()
                        .find(|column| column.name() == name)
                        .ok_or_else(|| {
                            DatabaseError::invalid("Result schema column was not returned")
                        })?;
                    Ok(ResultColumn {
                        name: name.clone(),
                        value_type: infer_type(column)?,
                        nullable: true,
                    })
                })
                .collect::<Result<Vec<_>, DatabaseError>>()?;
            projection(columns, &ResultSpec::Columns(requested))
        }
        ResultSpec::Raw => columns
            .iter()
            .enumerate()
            .map(|(index, column)| {
                Ok(Projection {
                    index,
                    column: Column {
                        name: column.name().into(),
                        database_type: column.type_info().name().into(),
                        value_type: infer_type(column)?,
                    },
                    nullable: true,
                })
            })
            .collect(),
        ResultSpec::Columns(requested) => {
            if requested.is_empty() {
                return Err(DatabaseError::invalid("Result schema cannot be empty"));
            }
            let mut names = std::collections::HashSet::new();
            requested
                .iter()
                .map(|field| {
                    if !names.insert(&field.name) {
                        return Err(DatabaseError::invalid("Duplicate result schema column"));
                    }
                    let mut matches = columns
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| c.name() == field.name);
                    let (index, column) = matches.next().ok_or_else(|| {
                        DatabaseError::invalid("Result schema column was not returned")
                    })?;
                    if matches.next().is_some() {
                        return Err(DatabaseError::invalid(
                            "Ambiguous result column; use SQL aliases",
                        ));
                    }
                    Ok(Projection {
                        index,
                        column: Column {
                            name: field.name.clone(),
                            database_type: column.type_info().name().into(),
                            value_type: field.value_type.clone(),
                        },
                        nullable: field.nullable,
                    })
                })
                .collect()
        }
    }
}

fn cell(row: &PgRow, projection: &Projection) -> Result<SqlValue, DatabaseError> {
    let index = projection.index;
    let invalid = || DatabaseError::invalid("SQL result cannot be decoded as the requested type");
    if row.try_get_raw(index).map_err(|_| invalid())?.is_null() {
        if !projection.nullable {
            return Err(DatabaseError::invalid("Non-nullable result column is NULL"));
        }
        return Ok(SqlValue::Null(projection.column.value_type.clone()));
    }
    macro_rules! get {
        ($t:ty) => {
            row.try_get::<$t, _>(index).map_err(|_| invalid())?
        };
    }
    Ok(match projection.column.value_type {
        SqlType::Text => SqlValue::Text(get!(String)),
        SqlType::Integer => SqlValue::Integer(match projection.column.database_type.as_str() {
            "INT2" => get!(i16).to_string(),
            "INT4" => get!(i32).to_string(),
            _ => get!(i64).to_string(),
        }),
        SqlType::Decimal => SqlValue::Decimal(get!(sqlx::types::BigDecimal).to_string()),
        SqlType::Float => {
            let value = if projection.column.database_type == "FLOAT4" {
                f64::from(get!(f32))
            } else {
                get!(f64)
            };
            if !value.is_finite() {
                return Err(DatabaseError::invalid(
                    "Non-finite SQL float is unsupported",
                ));
            }
            SqlValue::Float(value)
        }
        SqlType::Boolean => SqlValue::Boolean(get!(bool)),
        SqlType::Json => SqlValue::Json(get!(serde_json::Value)),
        SqlType::Timestamp => SqlValue::Timestamp(get!(NaiveDateTime).to_string()),
        SqlType::TimestampTz => SqlValue::TimestampTz(get!(DateTime<Utc>).to_rfc3339()),
        SqlType::Date => SqlValue::Date(get!(NaiveDate).to_string()),
        SqlType::Time => SqlValue::Time(get!(NaiveTime).to_string()),
        SqlType::Uuid => SqlValue::Uuid(get!(uuid::Uuid).to_string()),
        SqlType::Vector => SqlValue::Vector(get!(pgvector::Vector).to_vec()),
    })
}

async fn run_statement(
    connection: &mut sqlx::PgConnection,
    statement: &Statement,
    limits: DatabaseLimits,
) -> Result<ExecutionResult, DatabaseError> {
    let timeout_ms = limits
        .statement_timeout
        .as_millis()
        .max(1)
        .min(u64::MAX as u128);
    sqlx::query(&format!("SET LOCAL statement_timeout = {timeout_ms}"))
        .execute(&mut *connection)
        .await
        .map_err(driver_error)?;
    let params: Vec<_> = statement
        .params
        .iter()
        .map(|v| type_info(&sql_type(v)))
        .collect();
    let prepared = connection
        .prepare_with(&statement.sql, &params)
        .await
        .map_err(driver_error)?;
    let projected = statement
        .returning
        .as_ref()
        .map(|spec| projection(prepared.columns(), spec))
        .transpose()?;
    if statement.returning.is_none() && !prepared.columns().is_empty() {
        return Err(DatabaseError::invalid(
            "Statement returns rows; supply a result specification",
        ));
    }
    let mut result = ExecutionResult {
        rows_affected: 0,
        returned: projected.as_ref().map(|p| RowSet {
            columns: p.iter().map(|p| p.column.clone()).collect(),
            rows: vec![],
        }),
    };
    let query = bind(sqlx::query(&statement.sql), &statement.params)?;
    // One prepared statement, so fetch_many's stream has one command result.
    #[allow(deprecated)]
    let mut stream = query.fetch_many(connection);
    let mut row_bytes = 0usize;
    while let Some(item) = stream.try_next().await.map_err(driver_error)? {
        match item {
            Either::Left(done) => result.rows_affected = done.rows_affected(),
            Either::Right(row) => {
                if let (Some(projected), Some(rows)) = (&projected, &mut result.returned) {
                    if rows.rows.len() >= limits.max_rows {
                        return Err(result_too_large("SQL result exceeds the row limit"));
                    }
                    // Reject oversized driver values before allocating decoded
                    // JSON/strings/vectors or their serialized representations.
                    // SQLx has already received the protocol row at this point.
                    let raw_bytes = projected.iter().try_fold(0usize, |size, projection| {
                        let value = row.try_get_raw(projection.index).map_err(driver_error)?;
                        let length = if value.is_null() {
                            0
                        } else {
                            value
                                .as_bytes()
                                .map_err(|_| DatabaseError::invalid("Invalid SQL result encoding"))?
                                .len()
                        };
                        Ok::<_, DatabaseError>(size.saturating_add(length))
                    })?;
                    if raw_bytes > limits.response_limit().saturating_sub(row_bytes) {
                        return Err(result_too_large("SQL result exceeds the byte limit"));
                    }
                    let cells = projected
                        .iter()
                        .map(|p| cell(&row, p))
                        .collect::<Result<Vec<_>, _>>()?;
                    row_bytes = row_bytes.saturating_add(
                        serde_json::to_vec(&cells)
                            .map_err(|_| DatabaseError::invalid("Cannot encode SQL row"))?
                            .len()
                            + 1,
                    );
                    if row_bytes > limits.response_limit() {
                        return Err(result_too_large("SQL result exceeds the byte limit"));
                    }
                    rows.rows.push(cells);
                }
            }
        }
    }
    check_size(
        &result,
        limits.response_limit(),
        "SQL result exceeds the byte limit",
    )
    .map_err(|error| result_too_large(error.message))?;
    Ok(result)
}

impl ObjectStore {
    pub async fn database_query(
        &self,
        request: QueryRequest,
        limits: DatabaseLimits,
    ) -> Result<RowSet, DatabaseError> {
        limits.check()?;
        check_request(&request)?;
        let work = async {
            let mut tx = self.pool().begin().await.map_err(driver_error)?;
            sqlx::query("SET TRANSACTION READ ONLY")
                .execute(&mut *tx)
                .await
                .map_err(driver_error)?;
            let statement = Statement {
                sql: request.sql,
                params: request.params,
                returning: Some(request.result_schema),
            };
            let result = run_statement(&mut tx, &statement, limits).await;
            let rollback = tx.rollback().await;
            match result {
                Ok(result) => {
                    rollback.map_err(driver_error)?;
                    Ok(result.returned.unwrap_or_default())
                }
                Err(mut error) => {
                    error.outcome = Outcome::RolledBack;
                    Err(error)
                }
            }
        };
        tokio::time::timeout(limits.operation_timeout, work)
            .await
            .map_err(|_| timeout_error(true))?
    }

    pub async fn database_execute(
        &self,
        statement: Statement,
        limits: DatabaseLimits,
    ) -> Result<ExecutionResult, DatabaseError> {
        limits.check()?;
        check_request(&statement)?;
        tokio::time::timeout(
            limits.operation_timeout,
            self.database_execute_inner(&statement, limits),
        )
        .await
        .map_err(|_| timeout_error(false))?
    }

    async fn database_execute_inner(
        &self,
        statement: &Statement,
        limits: DatabaseLimits,
    ) -> Result<ExecutionResult, DatabaseError> {
        let mut connection = self.pool().acquire().await.map_err(|e| {
            let mut e = driver_error(e);
            e.outcome = Outcome::NotStarted;
            e
        })?;
        Self::execute_on_connection(&mut connection, statement, limits).await
    }

    async fn execute_on_connection(
        connection: &mut sqlx::PgConnection,
        statement: &Statement,
        limits: DatabaseLimits,
    ) -> Result<ExecutionResult, DatabaseError> {
        let mut tx = connection.begin().await.map_err(|e| {
            let mut e = driver_error(e);
            e.outcome = Outcome::NotStarted;
            e
        })?;
        match run_statement(&mut tx, statement, limits).await {
            Ok(result) => {
                tx.commit().await.map_err(|e| {
                    let mut e = driver_error(e);
                    e.retryable = false;
                    e
                })?;
                Ok(result)
            }
            Err(mut error) => {
                error.outcome = if tx.rollback().await.is_ok() {
                    Outcome::RolledBack
                } else {
                    Outcome::Unknown
                };
                if error.outcome == Outcome::Unknown {
                    error.retryable = false;
                }
                Err(error)
            }
        }
    }

    pub async fn database_execute_batch(
        &self,
        request: BatchRequest,
        limits: DatabaseLimits,
    ) -> Result<BatchResult, DatabaseError> {
        limits.check()?;
        check_request(&request)?;
        if request.statements.is_empty()
            || request.statements.len() > limits.max_batch_statements.min(MAX_BATCH_STATEMENTS)
        {
            return Err(DatabaseError::invalid(
                "Batch statement count is outside the allowed range",
            ));
        }
        match request.mode {
            BatchMode::Atomic => tokio::time::timeout(
                limits.operation_timeout,
                self.database_atomic(request, limits),
            )
            .await
            .map_err(|_| timeout_error(false))?,
            BatchMode::Independent => self.database_independent(request, limits).await,
        }
    }

    async fn database_atomic(
        &self,
        request: BatchRequest,
        limits: DatabaseLimits,
    ) -> Result<BatchResult, DatabaseError> {
        let mut tx = self.pool().begin().await.map_err(|e| {
            let mut e = driver_error(e);
            e.outcome = Outcome::NotStarted;
            e
        })?;
        let mut batch = BatchResult {
            mode: BatchMode::Atomic,
            results: vec![],
        };
        let mut returned_rows = 0usize;
        for (index, statement) in request.statements.iter().enumerate() {
            let item_limits = DatabaseLimits {
                max_rows: limits.max_rows.saturating_sub(returned_rows),
                ..limits
            };
            let result = run_statement(&mut tx, statement, item_limits).await;
            let validation = match result {
                Ok(result) => {
                    returned_rows += result.returned.as_ref().map_or(0, |rows| rows.rows.len());
                    batch.results.push(StatementResult {
                        index,
                        result: Ok(result),
                    });
                    check_size(
                        &batch,
                        limits.response_limit(),
                        "Batch result exceeds the byte limit",
                    )
                    .map_err(|error| result_too_large(error.message))
                }
                Err(error) => Err(error),
            };
            if let Err(mut error) = validation {
                error.statement_index = Some(index);
                error.outcome = if tx.rollback().await.is_ok() {
                    Outcome::RolledBack
                } else {
                    Outcome::Unknown
                };
                if error.outcome == Outcome::Unknown {
                    error.retryable = false;
                }
                return Err(error);
            }
        }
        tx.commit().await.map_err(|e| {
            let mut e = driver_error(e);
            e.retryable = false;
            e
        })?;
        Ok(batch)
    }

    async fn database_independent(
        &self,
        request: BatchRequest,
        limits: DatabaseLimits,
    ) -> Result<BatchResult, DatabaseError> {
        let deadline = tokio::time::Instant::now() + limits.operation_timeout;
        let mut batch = BatchResult {
            mode: BatchMode::Independent,
            results: vec![],
        };
        let mut stopped = false;
        // Reserve enough space for bounded errors/not-started results before any commits.
        let reserve = request.statements.len().saturating_mul(512);
        if reserve >= limits.response_limit() {
            return Err(DatabaseError::invalid("Batch exceeds result budget"));
        }
        let mut used = reserve;
        let mut returned_rows = 0usize;
        let mut connection = tokio::time::timeout_at(deadline, self.pool().acquire())
            .await
            .map_err(|_| {
                let mut error = timeout_error(false);
                error.outcome = Outcome::NotStarted;
                error.retryable = true;
                error
            })?
            .map_err(|e| {
                let mut error = driver_error(e);
                error.outcome = Outcome::NotStarted;
                error
            })?;
        for (index, statement) in request.statements.iter().enumerate() {
            let result = if stopped || tokio::time::Instant::now() >= deadline {
                let mut error =
                    DatabaseError::invalid("Statement was not started because the batch stopped");
                error.statement_index = Some(index);
                Err(error)
            } else {
                let remaining = limits.response_limit().saturating_sub(used);
                let item_limits = DatabaseLimits {
                    max_response_bytes: remaining,
                    max_rows: limits.max_rows.saturating_sub(returned_rows),
                    ..limits
                };
                let mut result = tokio::time::timeout_at(
                    deadline,
                    Self::execute_on_connection(&mut connection, statement, item_limits),
                )
                .await
                .unwrap_or_else(|_| Err(timeout_error(false)));
                if let Err(error) = &mut result {
                    error.statement_index = Some(index);
                    stopped = error.outcome == Outcome::Unknown
                        || error.code == "DATABASE_DEADLINE_EXCEEDED"
                        || error.code == "DATABASE_RESULT_TOO_LARGE"
                        || (error.code == "DATABASE_EXECUTION_FAILED" && error.sqlstate.is_none())
                        || error
                            .sqlstate
                            .as_deref()
                            .is_some_and(|s| s.starts_with("08") || s.starts_with("57"));
                }
                result
            };
            if let Ok(value) = &result {
                returned_rows += value.returned.as_ref().map_or(0, |rows| rows.rows.len());
                used = used.saturating_add(
                    serde_json::to_vec(value)
                        .map_err(|_| DatabaseError::invalid("Cannot encode result"))?
                        .len(),
                );
            }
            batch.results.push(StatementResult { index, result });
        }
        Ok(batch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_do_not_expose_driver_details() {
        let error = driver_error(sqlx::Error::Protocol(
            "private connection diagnostic".into(),
        ));
        assert!(!serde_json::to_string(&error).unwrap().contains("private"));
    }

    #[test]
    fn decimal_expansion_and_nonfinite_values_are_bounded() {
        let query = || sqlx::query("SELECT $1");
        assert!(bind(query(), &[SqlValue::Decimal("1e999999999".into())]).is_err());
        assert!(bind(query(), &[SqlValue::Float(f64::NAN)]).is_err());
        assert!(bind(query(), &[SqlValue::Integer(i64::MAX.to_string())]).is_ok());
    }
}
