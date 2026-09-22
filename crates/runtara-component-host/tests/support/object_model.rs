//! SQL-boundary values shared by native fixture backends. No HTTP or database.
use runtara_database_contract::*;
use serde_json::{Value, json};

pub fn rows(objects: Vec<Value>) -> RowSet {
    let Some(first) = objects.first().and_then(Value::as_object) else {
        return RowSet::default();
    };
    let columns: Vec<_> = first
        .keys()
        .map(|name| Column {
            name: name.clone(),
            database_type: "JSONB".into(),
            value_type: SqlType::Json,
        })
        .collect();
    let rows = objects
        .iter()
        .map(|row| {
            columns
                .iter()
                .map(|column| SqlValue::Json(row[&column.name].clone()))
                .collect()
        })
        .collect();
    RowSet { columns, rows }
}

pub fn memory_schema() -> RowSet {
    rows(vec![
        json!({"id":"schema","name":"ai_conversation_memory","tableName":"ai_conversation_memory","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z","description":null,"columns":[{"name":"conversation_id","type":"string","nullable":false,"unique":true},{"name":"messages","type":"json","nullable":false},{"name":"message_count","type":"integer","nullable":false}],"indexes":[]}),
    ])
}

pub fn descriptor(connection: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({"connectionId":connection,"integrationId":"postgres","status":"ACTIVE","resources":[],"metadata":{"object_model":{"version":1,"metadata_table":"__schema","soft_delete":true,"auto_columns":{"id":true,"created_at":true,"updated_at":true},"bulk_request_limit":10000}}})).unwrap()
}
