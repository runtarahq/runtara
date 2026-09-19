//! Capability adapters. Operations below are local agent functions, not host APIs.
use crate::{
    AgentError,
    model::ObjectModel,
    sql_client::{HostSqlClient, SqlClient},
};
use runtara_database_contract::*;
use runtara_object_model_core::{ColumnType, Condition, FilterRequest};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Clone, Copy)]
pub enum Operation {
    Create,
    Query,
    Exists,
    CreateIfMissing,
    Update,
    Delete,
    BulkCreate,
    BulkUpdate,
    BulkDelete,
    Aggregate,
    GetSchema,
    CreateSchema,
    QuerySql,
    ExecuteSql,
}

fn invalid(message: impl std::fmt::Display) -> DatabaseError {
    DatabaseError::invalid(message.to_string())
}
fn required<'a>(request: &'a Value, key: &str) -> Result<&'a str, DatabaseError> {
    request[key]
        .as_str()
        .ok_or_else(|| invalid(format!("Missing {key}")))
}
fn parse<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, DatabaseError> {
    serde_json::from_value(value).map_err(invalid)
}

fn condition(request: &Value, filters_key: &str) -> Result<Option<Condition>, DatabaseError> {
    if !request["condition"].is_null() {
        return parse(request["condition"].clone()).map(Some);
    }
    let Some(filters) = request[filters_key].as_object() else {
        return Ok(None);
    };
    if filters.is_empty() {
        return Ok(None);
    }
    let parts = filters
        .iter()
        .map(|(field, value)| Condition::eq(field, value.clone()))
        .collect();
    Ok(Some(Condition::and(parts)))
}

fn filter(request: &Value) -> Result<FilterRequest, DatabaseError> {
    Ok(FilterRequest {
        condition: condition(request, "filters")?,
        offset: request["offset"].as_i64().unwrap_or(0),
        limit: request["limit"].as_i64().unwrap_or(100),
        sort_by: parse(request["sortBy"].clone())?,
        sort_order: parse(request["sortOrder"].clone())?,
        score_expression: parse(request["scoreExpression"].clone())?,
        order_by: parse(request["orderBy"].clone())?,
        projection: None,
    })
}

pub fn agent_error(error: DatabaseError, read_only: bool) -> AgentError {
    let retryable = error.retryable
        && (read_only || matches!(error.outcome, Outcome::NotStarted | Outcome::RolledBack));
    let code = match error.code.as_str() {
        "ENTITLEMENT_REQUIRED" => "OBJECT_MODEL_UNAUTHORIZED",
        "DATABASE_DEADLINE_EXCEEDED" => "OBJECT_MODEL_HOST_ERROR",
        "DATABASE_INVALID_RESPONSE" => "OBJECT_MODEL_PARSE_ERROR",
        "DATABASE_RESULT_TOO_LARGE" => "OBJECT_MODEL_PAYLOAD_TOO_LARGE",
        _ if error.message.contains("byte limit") => "OBJECT_MODEL_PAYLOAD_TOO_LARGE",
        _ if retryable => "OBJECT_MODEL_UPSTREAM_ERROR",
        _ => "OBJECT_MODEL_REQUEST_FAILED",
    };
    let mut result = if retryable {
        AgentError::transient(code, error.message)
    } else {
        AgentError::permanent(code, error.message)
    };
    result = result
        .with_attr("integration", "OBJECT_MODEL")
        .with_attr("database_code", error.code)
        .with_attr(
            "outcome",
            serde_json::to_value(error.outcome)
                .unwrap()
                .as_str()
                .unwrap(),
        );
    if let Some(sqlstate) = error.sqlstate {
        result = result.with_attr("sqlstate", sqlstate);
    }
    if let Some(index) = error.statement_index {
        result = result.with_attr("statement_index", index.to_string());
    }
    result
}

pub async fn call(
    operation: Operation,
    request: Value,
    connection: &str,
) -> Result<Value, AgentError> {
    let result = invoke(&HostSqlClient, operation, request, connection).await;
    match result {
        Ok(result) => Ok(result),
        // Preserve the existing CRUD/bulk success:false envelope for domain
        // failures. Execution uncertainty and authority failures remain errors.
        Err(error)
            if !matches!(operation, Operation::QuerySql | Operation::ExecuteSql)
                && error.code != "ENTITLEMENT_REQUIRED"
                && error.code != "DATABASE_CONNECTION_UNAVAILABLE"
                && !error.retryable
                && matches!(error.outcome, Outcome::NotStarted | Outcome::RolledBack)
                && error.code != "DATABASE_DEADLINE_EXCEEDED" =>
        {
            Ok(json!({"success":false,"error":error.message}))
        }
        Err(error) => Err(agent_error(
            error,
            matches!(
                operation,
                Operation::Query
                    | Operation::Exists
                    | Operation::Aggregate
                    | Operation::GetSchema
                    | Operation::QuerySql
            ),
        )),
    }
}

#[derive(Deserialize)]
struct LegacyParam {
    #[serde(flatten)]
    column_type: ColumnType,
    value: Value,
}
#[derive(Deserialize)]
struct LegacyColumn {
    name: String,
    #[serde(flatten)]
    column_type: ColumnType,
    #[serde(default = "yes")]
    nullable: bool,
}
fn yes() -> bool {
    true
}

pub async fn invoke<C: SqlClient>(
    client: &C,
    operation: Operation,
    request: Value,
    connection: &str,
) -> Result<Value, DatabaseError> {
    if matches!(operation, Operation::QuerySql | Operation::ExecuteSql) {
        let sql = required(&request, "sql")?.to_owned();
        let params: Vec<LegacyParam> = parse(request.get("params").cloned().unwrap_or(json!([])))?;
        let params = params
            .iter()
            .map(|param| {
                runtara_object_model_core::mapping::legacy_sql_param(
                    &param.column_type,
                    &param.value,
                )
                .map_err(invalid)
            })
            .collect::<Result<Vec<_>, _>>()?;
        if matches!(operation, Operation::ExecuteSql) {
            let result = client
                .execute(
                    connection,
                    Statement {
                        sql,
                        params,
                        returning: None,
                    },
                )
                .await?;
            return Ok(json!({"success":true,"rowsAffected":result.rows_affected}));
        }
        let expected: Option<Vec<LegacyColumn>> = if request["resultSchema"].is_null() {
            None
        } else {
            Some(parse(request["resultSchema"].clone())?)
        };
        let result_schema = expected
            .as_ref()
            .map(|columns| {
                ResultSpec::Selected(columns.iter().map(|column| column.name.clone()).collect())
            })
            .unwrap_or(ResultSpec::Raw);
        let rows = client
            .query(
                connection,
                QueryRequest {
                    sql,
                    params,
                    result_schema,
                },
            )
            .await?;
        if let Some(expected) = expected {
            for row in &rows.rows {
                if row.len() != expected.len() {
                    return Err(invalid("Invalid SQL row width"));
                }
                for (field, value) in expected.iter().zip(row) {
                    runtara_object_model_core::mapping::validate_sql_result(
                        &field.column_type,
                        value,
                        field.nullable,
                    )
                    .map_err(invalid)?;
                }
            }
        }
        let rows = runtara_object_model_core::mapping::row_objects(rows).map_err(invalid)?;
        return Ok(json!({"success":true,"rowCount":rows.len(),"rows":rows}));
    }
    let model = ObjectModel::new(client, connection).await?;
    match operation {
        Operation::Create => {
            model
                .create(
                    required(&request, "schema_name")?,
                    request["properties"].clone(),
                )
                .await
        }
        Operation::Query => {
            model
                .query(required(&request, "schema_name")?, filter(&request)?)
                .await
        }
        Operation::Update => {
            model
                .update(
                    required(&request, "schema_name")?,
                    required(&request, "instance_id")?,
                    request["data"].clone(),
                )
                .await
        }
        Operation::Delete => {
            model
                .delete(
                    required(&request, "schema_name")?,
                    required(&request, "instance_id")?,
                )
                .await
        }
        Operation::BulkCreate => model.bulk_create(parse(request)?).await,
        Operation::BulkUpdate => model.bulk_update(parse(request)?).await,
        Operation::BulkDelete => model.bulk_delete(parse(request)?).await,
        Operation::Aggregate => {
            model
                .aggregate(required(&request, "schema_name")?, parse(request.clone())?)
                .await
        }
        Operation::GetSchema => match model.schema(required(&request, "name")?).await? {
            Some(schema) => Ok(json!({"success":true,"schema":schema})),
            None => Ok(json!({"success":false,"schema":null,"error":"Schema not found"})),
        },
        Operation::CreateSchema => model.create_schema(parse(request)?).await,
        Operation::Exists | Operation::CreateIfMissing => {
            let schema_name = required(&request, "schema_name")?;
            let is_create = matches!(operation, Operation::CreateIfMissing);
            let filter = FilterRequest {
                limit: 1,
                condition: condition(
                    &request,
                    if is_create {
                        "match_filters"
                    } else {
                        "filters"
                    },
                )?,
                ..Default::default()
            };
            let result = model.query(schema_name, filter).await?;
            let instance = result["instances"].as_array().and_then(|rows| rows.first());
            if is_create {
                match instance {
                    Some(instance) => Ok(
                        json!({"success":true,"created":false,"already_existed":true,"instance_id":instance["id"]}),
                    ),
                    None => {
                        let created = model.create(schema_name, request["data"].clone()).await?;
                        Ok(
                            json!({"success":true,"created":true,"already_existed":false,"instance_id":created["instance_id"]}),
                        )
                    }
                }
            } else {
                Ok(
                    json!({"exists":instance.is_some(),"instance_id":instance.map(|v| &v["id"]),"instance":instance}),
                )
            }
        }
        Operation::QuerySql | Operation::ExecuteSql => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_mutations_never_become_retryable() {
        let error = DatabaseError {
            code: "DATABASE_EXECUTION_FAILED".into(),
            message: "Database operation failed".into(),
            outcome: Outcome::Unknown,
            retryable: true,
            sqlstate: None,
            statement_index: None,
        };
        assert_eq!(agent_error(error.clone(), false).category, "permanent");
        assert_eq!(agent_error(error, true).category, "transient");
    }
    #[test]
    fn rolled_back_serialization_failure_can_be_retried() {
        let error = DatabaseError {
            code: "DATABASE_EXECUTION_FAILED".into(),
            message: "Database operation failed".into(),
            outcome: Outcome::RolledBack,
            retryable: true,
            sqlstate: Some("40001".into()),
            statement_index: Some(2),
        };
        let error = agent_error(error, false);
        assert_eq!(error.category, "transient");
        assert_eq!(error.attributes["statement_index"], "2");
    }
    #[test]
    fn empty_filters_mean_an_unfiltered_query() {
        for request in [json!({}), json!({"filters": {}}), json!({"filters": null})] {
            assert!(filter(&request).unwrap().condition.is_none());
        }
    }
    #[test]
    fn query_adapter_preserves_score_order_and_conditions() {
        let filter = filter(&json!({"filters":{"name":"value"},"scoreExpression":{"alias":"score","expression":{"valueType":"reference","value":"count"}},"orderBy":[{"expression":{"kind":"alias","name":"score"},"direction":"DESC"}]})).unwrap();
        assert!(filter.condition.is_some());
        assert_eq!(filter.score_expression.unwrap().alias, "score");
        assert_eq!(filter.order_by.unwrap().len(), 1);
    }
}
