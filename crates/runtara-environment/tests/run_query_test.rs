// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! `run_query` against a reference: runs with nested state (objects,
//! arrays, numbers and strings in one field, nulls, missing fields) are
//! queried in Postgres, and each answer is held to the same query computed
//! here, in Rust, from the runs as stored.
//!
//! Requires the explicit `db-integration-tests` feature and a live Postgres.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use runtara_environment::instance_repository::InstanceRepository;
use runtara_environment::run_query::{RunQuery, RunQueryResult};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgConnectOptions;
use uuid::Uuid;

fn url() -> String {
    std::env::var("TEST_ENVIRONMENT_DATABASE_URL")
        .expect("db-integration-tests requires TEST_ENVIRONMENT_DATABASE_URL")
}

async fn fresh() -> (PgPool, String) {
    let admin = PgPool::connect(&url()).await.unwrap();
    let name = format!("run_query_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&admin)
        .await
        .unwrap();
    let options: PgConnectOptions = url().parse().unwrap();
    let pool = PgPool::connect_with(options.database(&name)).await.unwrap();
    runtara_environment::migrations::run(&pool)
        .await
        .expect("migrations");
    (pool, name)
}

async fn drop_database(pool: PgPool, name: &str) {
    pool.close().await;
    let admin = PgPool::connect(&url()).await.unwrap();
    sqlx::query(&format!("DROP DATABASE {name}"))
        .execute(&admin)
        .await
        .unwrap();
}

/// A run as stored.
#[derive(Debug, Clone)]
struct Run {
    id: String,
    label: Option<String>,
    status: &'static str,
    version: i64,
    created: i64,
    updated: Option<i64>,
    state: Option<Value>,
}

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn pick<'a, T>(&mut self, of: &'a [T]) -> &'a T {
        &of[(self.next() as usize) % of.len()]
    }
}

fn runs() -> Vec<Run> {
    let mut rng = Lcg(7);
    let names = ["Anna", "Bogdan", "Marta", "Zoe", "anders", "Ólafur"];
    let countries = ["DE", "PL", "US", "FR"];
    let statuses = [
        "completed",
        "completed",
        "completed",
        "failed",
        "suspended",
        "running",
    ];
    let mut runs = Vec::new();
    for n in 0..600i64 {
        let label = if n % 97 == 0 {
            None
        } else {
            Some(format!("order-{:03}", n % 180))
        };
        let state = if n % 53 == 0 {
            None
        } else {
            let items: Vec<Value> = (0..(rng.next() % 4))
                .map(|_| {
                    json!({
                        "sku": format!("SKU-{}", rng.next() % 6),
                        "qty": rng.next() % 7,
                        "price": (rng.next() % 2000) as f64 / 100.0,
                    })
                })
                .collect();
            let total = match rng.next() % 10 {
                0 => json!(format!("{}", rng.next() % 300)),
                1 => Value::Null,
                2 => json!((rng.next() % 30000) as f64 / 100.0),
                _ => json!(rng.next() % 300),
            };
            let mut state = json!({
                "customer": {
                    "name": rng.pick(&names),
                    "country": rng.pick(&countries),
                    "vip": rng.next().is_multiple_of(3),
                },
                "total": total,
                "items": items,
                "tags": match rng.next() % 4 {
                    0 => json!([]),
                    1 => json!(["vip"]),
                    2 => json!(["vip", "rush"]),
                    _ => json!(["rush"]),
                },
            });
            match rng.next() % 4 {
                0 => {}
                1 => state["note"] = Value::Null,
                2 => state["note"] = json!(""),
                _ => state["note"] = json!(format!("call {}", rng.pick(&names))),
            }
            if n % 41 == 0 {
                // An array where others hold an object.
                state["customer"] = json!(["not", "an", "object"]);
            }
            Some(state)
        };
        runs.push(Run {
            id: format!("run-{n:04}"),
            label,
            status: rng.pick(&statuses),
            version: if n < 300 { 1 } else { 2 },
            created: n * 60,
            updated: state.as_ref().map(|_| n * 60 + (rng.next() % 5000) as i64),
            state,
        });
    }
    runs
}

async fn load(pool: &PgPool, tenant: &str, runs: &[Run]) {
    for version in [1, 2] {
        sqlx::query(
            "INSERT INTO images (image_id, tenant_id, name, binary_path) VALUES ($1, $2, $3, '/x')",
        )
        .bind(format!("{tenant}-{version}"))
        .bind(tenant)
        .bind(format!("orders:{version}"))
        .execute(pool)
        .await
        .unwrap();
    }
    let start = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z").unwrap();
    for run in runs {
        let created = start + chrono::Duration::seconds(run.created);
        sqlx::query(
            "INSERT INTO instances (instance_id, tenant_id, status, run_label, created_at) \
             VALUES ($1, $2, $3::instance_status, $4, $5)",
        )
        .bind(&run.id)
        .bind(tenant)
        .bind(run.status)
        .bind(&run.label)
        .bind(created)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO instance_images (instance_id, image_id, tenant_id) VALUES ($1, $2, $3)",
        )
        .bind(&run.id)
        .bind(format!("{tenant}-{}", run.version))
        .bind(tenant)
        .execute(pool)
        .await
        .unwrap();
        if let (Some(state), Some(updated)) = (&run.state, run.updated) {
            sqlx::query(
                "INSERT INTO instance_state (instance_id, state, state_updated_at) VALUES ($1, $2, $3)",
            )
            .bind(&run.id)
            .bind(state)
            .bind(start + chrono::Duration::seconds(updated))
            .execute(pool)
            .await
            .unwrap();
        }
    }
}

// ---- The reference -------------------------------------------------------

/// A row: a run, and with `unnest` one of its elements.
#[derive(Debug, Clone)]
struct Row<'a> {
    run: &'a Run,
    item: Option<Value>,
}

/// Each label's latest run: state written last, then created last, then
/// the greater id.
fn latest(runs: &[Run]) -> Vec<&Run> {
    let mut by: BTreeMap<&str, &Run> = BTreeMap::new();
    for run in runs {
        let Some(label) = &run.label else { continue };
        let rank = |r: &Run| (r.updated.unwrap_or(i64::MIN), r.created, r.id.clone());
        match by.get(label.as_str()) {
            Some(kept) if rank(kept) >= rank(run) => {}
            _ => {
                by.insert(label, run);
            }
        }
    }
    by.into_values().collect()
}

/// `value` along `path`, as Postgres' `#>` reads it.
fn follow(value: Option<&Value>, path: &[&str]) -> Option<Value> {
    let mut at = value?.clone();
    for segment in path {
        at = match at {
            Value::Object(mut object) => object.remove(*segment)?,
            Value::Array(mut array) => {
                let index: i64 = segment.parse().ok()?;
                let index = if index < 0 {
                    array.len() as i64 + index
                } else {
                    index
                };
                if index < 0 || index as usize >= array.len() {
                    return None;
                }
                array.swap_remove(index as usize)
            }
            _ => return None,
        };
    }
    Some(at)
}

/// What a row holds at `path` (no `[*]`); `None` where it is missing.
fn get(row: &Row, path: &str) -> Option<Value> {
    match path {
        "runId" => Some(json!(row.run.id)),
        "label" => row.run.label.clone().map(Value::String),
        "status" => Some(json!(row.run.status)),
        "version" => Some(json!(row.run.version)),
        _ => {
            let (root, rest) = path.split_once('.').map_or((path, ""), |(a, b)| (a, b));
            let segments: Vec<&str> = if rest.is_empty() {
                vec![]
            } else {
                rest.split('.').collect()
            };
            match root {
                "state" => follow(row.run.state.as_ref(), &segments),
                "item" => follow(row.item.as_ref(), &segments),
                _ => panic!("no path {path}"),
            }
        }
    }
}

fn number(value: &Value) -> Option<f64> {
    value.as_f64()
}

/// jsonb equality: numbers by value, the rest structurally.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            number(&Value::Number(x.clone())) == number(&Value::Number(y.clone()))
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same(a, b))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

fn empty(value: &Option<Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s.is_empty(),
        Some(Value::Array(a)) => a.is_empty(),
        Some(Value::Object(o)) => o.is_empty(),
        _ => false,
    }
}

fn compare(value: &Value, operand: &Value) -> Option<Ordering> {
    match (value, operand) {
        (Value::Number(_), Value::Number(_)) => number(value)?.partial_cmp(&number(operand)?),
        (Value::String(a), Value::String(b)) => Some(a.as_bytes().cmp(b.as_bytes())),
        (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
        _ => None,
    }
}

fn leaf(value: Option<Value>, op: &str, operand: Option<&Value>) -> bool {
    let defined = matches!(&value, Some(v) if !v.is_null());
    let operand = operand.cloned().unwrap_or(Value::Null);
    match op {
        "EQ" if operand.is_null() => !defined,
        "NE" if operand.is_null() => defined,
        "EQ" => value.is_some_and(|v| same(&v, &operand)),
        "NE" => defined && !same(value.as_ref().unwrap(), &operand),
        "GT" | "GTE" | "LT" | "LTE" => {
            value
                .and_then(|v| compare(&v, &operand))
                .is_some_and(|ordering| match op {
                    "GT" => ordering == Ordering::Greater,
                    "GTE" => ordering != Ordering::Less,
                    "LT" => ordering == Ordering::Less,
                    _ => ordering != Ordering::Greater,
                })
        }
        "IN" | "NOT_IN" => {
            let list = operand.as_array().unwrap();
            let hit = value
                .as_ref()
                .is_some_and(|v| list.iter().any(|o| !o.is_null() && same(v, o)))
                || (list.iter().any(Value::is_null) && !defined);
            if op == "IN" { hit } else { defined && !hit }
        }
        "IS_DEFINED" => defined != (operand == Value::Bool(false)),
        "IS_EMPTY" => empty(&value) != (operand == Value::Bool(false)),
        "IS_NOT_EMPTY" => !empty(&value) != (operand == Value::Bool(false)),
        "CONTAINS" => match value {
            Some(Value::Array(elements)) => elements.iter().any(|e| same(e, &operand)),
            Some(Value::String(text)) => operand.as_str().is_some_and(|o| text.contains(o)),
            _ => false,
        },
        "STARTS_WITH" => {
            matches!((&value, &operand), (Some(Value::String(t)), Value::String(o)) if t.starts_with(o.as_str()))
        }
        "ENDS_WITH" => {
            matches!((&value, &operand), (Some(Value::String(t)), Value::String(o)) if t.ends_with(o.as_str()))
        }
        _ => panic!("no op {op}"),
    }
}

fn holds(row: &Row, condition: &Value) -> bool {
    let op = condition["op"].as_str().unwrap();
    let arguments = condition["arguments"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    match op {
        "AND" => arguments.iter().all(|c| holds(row, c)),
        "OR" => arguments.iter().any(|c| holds(row, c)),
        "NOT" => !holds(row, &arguments[0]),
        _ => {
            let path = arguments[0].as_str().unwrap();
            let operand = arguments.get(1);
            match path.split_once("[*]") {
                None => leaf(get(row, path), op, operand),
                Some((lead, rest)) => {
                    let rest: Vec<&str> = rest
                        .trim_start_matches('.')
                        .split('.')
                        .filter(|s| !s.is_empty())
                        .collect();
                    match get(row, lead) {
                        Some(Value::Array(elements)) => elements
                            .iter()
                            .any(|e| leaf(follow(Some(e), &rest), op, operand)),
                        _ => false,
                    }
                }
            }
        }
    }
}

fn aggregate(rows: &[&Row], function: &str, path: Option<&str>) -> Value {
    let values: Vec<Value> = match path {
        None => return json!(rows.len()),
        Some(path) => rows
            .iter()
            .filter_map(|row| get(row, path))
            .filter(|v| !v.is_null())
            .collect(),
    };
    let numbers: Vec<f64> = values.iter().filter_map(Value::as_f64).collect();
    let strings: Vec<&str> = values.iter().filter_map(Value::as_str).collect();
    match function {
        "count" => json!(values.len()),
        "count_distinct" => {
            let mut distinct: Vec<&Value> = Vec::new();
            for v in &values {
                if !distinct.iter().any(|d| same(d, v)) {
                    distinct.push(v);
                }
            }
            json!(distinct.len())
        }
        "sum" if numbers.is_empty() => Value::Null,
        "sum" => json!(numbers.iter().sum::<f64>()),
        "avg" if numbers.is_empty() => Value::Null,
        "avg" => json!(numbers.iter().sum::<f64>() / numbers.len() as f64),
        "min" | "max" => {
            let pick_max = function == "max";
            if !numbers.is_empty() {
                let f = numbers.iter().cloned().fold(None, |a: Option<f64>, b| {
                    Some(a.map_or(b, |a| if pick_max { a.max(b) } else { a.min(b) }))
                });
                json!(f)
            } else if !strings.is_empty() {
                let s = if pick_max {
                    strings.iter().max_by(|a, b| a.as_bytes().cmp(b.as_bytes()))
                } else {
                    strings.iter().min_by(|a, b| a.as_bytes().cmp(b.as_bytes()))
                };
                json!(s)
            } else {
                Value::Null
            }
        }
        _ => panic!("no fn {function}"),
    }
}

/// The query's rows, computed here; unordered unless the query orders
/// them by label only.
fn reference(runs: &[Run], query: &Value) -> Vec<Vec<Value>> {
    let base: Vec<&Run> = if query["latestPerLabel"] == json!(true) {
        latest(runs)
    } else {
        runs.iter().collect()
    };
    let mut rows: Vec<Row> = Vec::new();
    for run in base {
        match query["unnest"].as_str() {
            None => rows.push(Row { run, item: None }),
            Some(path) => {
                if let Some(Value::Array(elements)) = get(&Row { run, item: None }, path) {
                    for element in elements {
                        rows.push(Row {
                            run,
                            item: Some(element),
                        });
                    }
                }
            }
        }
    }
    if !query["condition"].is_null() {
        rows.retain(|row| holds(row, &query["condition"]));
    }
    let group_by: Vec<&str> = query["groupBy"]
        .as_array()
        .map(|a| a.iter().map(|v| v.as_str().unwrap()).collect())
        .unwrap_or_default();
    let aggregates = query["aggregates"].as_array().cloned().unwrap_or_default();
    if group_by.is_empty() && aggregates.is_empty() {
        let select: Vec<&str> = query["select"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        return rows
            .iter()
            .map(|row| {
                select
                    .iter()
                    .map(|path| get(row, path).unwrap_or(Value::Null))
                    .collect()
            })
            .collect();
    }
    // Missing and JSON null group apart, as SQL NULL and jsonb null do.
    let mut groups: Vec<(Vec<Option<Value>>, Vec<&Row>)> = Vec::new();
    for row in &rows {
        let key: Vec<Option<Value>> = group_by.iter().map(|path| get(row, path)).collect();
        let same_key = |k: &Vec<Option<Value>>| {
            k.len() == key.len()
                && k.iter().zip(&key).all(|(a, b)| match (a, b) {
                    (None, None) => true,
                    (Some(a), Some(b)) => same(a, b),
                    _ => false,
                })
        };
        match groups.iter_mut().find(|(k, _)| same_key(k)) {
            Some((_, members)) => members.push(row),
            None => groups.push((key, vec![row])),
        }
    }
    if group_by.is_empty() && groups.is_empty() {
        groups.push((vec![], vec![]));
    }
    groups
        .into_iter()
        .map(|(key, members)| {
            key.into_iter()
                .map(|v| v.unwrap_or(Value::Null))
                .chain(
                    aggregates.iter().map(|a| {
                        aggregate(&members, a["fn"].as_str().unwrap(), a["path"].as_str())
                    }),
                )
                .collect()
        })
        .collect()
}

/// Numbers to six decimals, so exact decimals and floats compare.
fn normal(value: &Value) -> Value {
    match value {
        Value::Number(n) => json!(format!("{:.6}", n.as_f64().unwrap())),
        Value::Array(a) => Value::Array(a.iter().map(normal).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), normal(v))).collect()),
        other => other.clone(),
    }
}

fn sorted(rows: &[Vec<Value>]) -> Vec<String> {
    let mut rows: Vec<String> = rows
        .iter()
        .map(|r| normal(&Value::Array(r.clone())).to_string())
        .collect();
    rows.sort();
    rows
}

async fn ask(repository: &InstanceRepository, tenant: &str, query: &Value) -> RunQueryResult {
    let parsed: RunQuery = serde_json::from_value(query.clone()).unwrap();
    repository
        .query_runs(tenant, "orders", parsed)
        .await
        .unwrap_or_else(|error| panic!("{query}: {error}"))
}

fn c(op: &str, arguments: Value) -> Value {
    json!({ "op": op, "arguments": arguments })
}

#[tokio::test]
async fn queries_answer_as_the_reference_does() {
    let (pool, name) = fresh().await;
    let tenant = "query-tenant";
    let mut runs = runs();
    load(&pool, tenant, &runs).await;
    // Runs that change status after they were bound: a label's latest run
    // must report its status now, not the one it had then.
    for run in runs.iter_mut().filter(|run| run.id.ends_with('7')) {
        let status = if run.status == "completed" {
            "cancelled"
        } else {
            "completed"
        };
        sqlx::query("UPDATE instances SET status = $2::instance_status WHERE instance_id = $1")
            .bind(&run.id)
            .bind(status)
            .execute(&pool)
            .await
            .unwrap();
        run.status = status;
    }
    let repository = InstanceRepository::new(pool.clone());

    let select = json!([
        "runId",
        "label",
        "status",
        "version",
        "state.customer.country",
        "state.total",
        "state.tags"
    ]);
    let conditions = vec![
        c("EQ", json!(["state.customer.country", "DE"])),
        c("EQ", json!(["state.customer.vip", true])),
        c("EQ", json!(["state.total", 120])),
        c("EQ", json!(["state.note", null])),
        c("EQ", json!(["state.tags", ["vip", "rush"]])),
        c("NE", json!(["state.customer.country", "DE"])),
        c("NE", json!(["state.note", null])),
        c("GT", json!(["state.total", 150])),
        c("LTE", json!(["state.total", 99.5])),
        c("GTE", json!(["state.customer.name", "M"])),
        c("LT", json!(["state.total", "2"])),
        c("GT", json!(["state.customer.vip", false])),
        c("IN", json!(["state.customer.country", ["DE", "PL"]])),
        c("IN", json!(["state.total", [12, 150, "40", null]])),
        c("NOT_IN", json!(["state.customer.country", ["DE", "PL"]])),
        c("IS_DEFINED", json!(["state.note"])),
        c("IS_DEFINED", json!(["state.note", false])),
        c("IS_EMPTY", json!(["state.note"])),
        c("IS_NOT_EMPTY", json!(["state.tags"])),
        c("IS_EMPTY", json!(["state.items"])),
        c("CONTAINS", json!(["state.tags", "vip"])),
        c("CONTAINS", json!(["state.customer.name", "an"])),
        c("CONTAINS", json!(["state.note", "Zoe"])),
        c("STARTS_WITH", json!(["state.customer.name", "A"])),
        c("ENDS_WITH", json!(["state.note", "a"])),
        c("EQ", json!(["state.items[*].sku", "SKU-3"])),
        c("GT", json!(["state.items[*].qty", 4])),
        c("IN", json!(["state.items[*].sku", ["SKU-1", "SKU-5"]])),
        c("LT", json!(["state.items[*].price", 1.5])),
        c("EQ", json!(["state.items.0.sku", "SKU-2"])),
        c("EQ", json!(["state.customer.0", "not"])),
        c("EQ", json!(["state", {"total": 1}])),
        c("IN", json!(["status", ["failed", "suspended"]])),
        c("NOT_IN", json!(["status", ["completed"]])),
        c("EQ", json!(["version", 2])),
        c("GT", json!(["version", 1])),
        c("STARTS_WITH", json!(["label", "order-01"])),
        c("EQ", json!(["label", null])),
        c(
            "AND",
            json!([
                c("EQ", json!(["state.customer.country", "PL"])),
                c(
                    "OR",
                    json!([
                        c("GT", json!(["state.total", 200])),
                        c("CONTAINS", json!(["state.tags", "rush"]))
                    ])
                ),
            ]),
        ),
        c(
            "NOT",
            json!([c("EQ", json!(["state.customer.country", "DE"]))]),
        ),
        c("NOT", json!([c("GT", json!(["state.items[*].qty", 3]))])),
        c("OR", json!([])),
        c("AND", json!([])),
    ];

    let mut checked = 0;
    for latest in [false, true] {
        for condition in &conditions {
            let query = json!({
                "latestPerLabel": latest,
                "condition": condition,
                "select": select,
                "limit": 1000,
                "count": true,
            });
            let got = ask(&repository, tenant, &query).await;
            let want = reference(&runs, &query);
            assert_eq!(sorted(&got.rows), sorted(&want), "{query}");
            assert_eq!(got.total, Some(want.len() as u64), "{query}");
            assert!(!got.has_more);
            checked += 1;
        }

        let grouped = vec![
            json!({"groupBy": ["state.customer.country"], "aggregates": [
                {"fn": "count"}, {"fn": "sum", "path": "state.total"}, {"fn": "avg", "path": "state.total"},
                {"fn": "min", "path": "state.total"}, {"fn": "max", "path": "state.total"},
                {"fn": "count_distinct", "path": "state.customer.name"}, {"fn": "count", "path": "state.note"}]}),
            json!({"groupBy": ["status", "version"], "aggregates": [{"fn": "count"}, {"fn": "max", "path": "state.customer.name"}]}),
            json!({"groupBy": ["state.customer.vip"], "aggregates": [{"fn": "count"}]}),
            json!({"groupBy": ["state.tags"], "aggregates": [{"fn": "count"}]}),
            json!({"aggregates": [{"fn": "count"}, {"fn": "min", "path": "state.customer.name"}, {"fn": "sum", "path": "version"}]}),
            json!({"aggregates": [{"fn": "count"}], "condition": c("EQ", json!(["state.customer.country", "XX"]))}),
            json!({"groupBy": ["state.customer.country"], "aggregates": [{"fn": "count"}],
                   "condition": c("CONTAINS", json!(["state.tags", "vip"]))}),
            json!({"unnest": "state.items", "groupBy": ["item.sku"], "aggregates": [
                {"fn": "count"}, {"fn": "sum", "path": "item.qty"}, {"fn": "max", "path": "item.price"}]}),
            json!({"unnest": "state.items", "groupBy": ["state.customer.country", "item.sku"], "aggregates": [{"fn": "sum", "path": "item.qty"}],
                   "condition": c("GT", json!(["item.qty", 2]))}),
            json!({"unnest": "state.tags", "groupBy": ["item"], "aggregates": [{"fn": "count"}]}),
        ];
        for query in grouped {
            let mut query = query;
            query["latestPerLabel"] = json!(latest);
            query["limit"] = json!(1000);
            let got = ask(&repository, tenant, &query).await;
            let want = reference(&runs, &query);
            assert_eq!(sorted(&got.rows), sorted(&want), "{query}");
            checked += 1;
        }

        let listed = json!({"latestPerLabel": latest, "unnest": "state.items",
            "select": ["label", "item.sku", "item.qty"], "condition": c("EQ", json!(["item.sku", "SKU-4"])), "limit": 1000});
        let got = ask(&repository, tenant, &listed).await;
        assert_eq!(
            sorted(&got.rows),
            sorted(&reference(&runs, &listed)),
            "{listed}"
        );
        checked += 1;
    }

    // Order and pages: each label's latest, by label, ten at a time.
    let mut all = Vec::new();
    for page in 0..20 {
        let query = json!({"latestPerLabel": true, "select": ["label"], "orderBy": [{"path": "label"}],
                           "limit": 10, "offset": page * 10, "count": true});
        let got = ask(&repository, tenant, &query).await;
        assert_eq!(got.total, Some(latest(&runs).len() as u64));
        all.extend(got.rows.into_iter().map(|row| row[0].clone()));
        if !got.has_more {
            break;
        }
    }
    let want: Vec<Value> = latest(&runs).iter().map(|run| json!(run.label)).collect();
    assert_eq!(all, want);

    // Descending by a number, then by label: totals that are numbers come
    // first, in order.
    let got = ask(
        &repository,
        tenant,
        &json!({"latestPerLabel": true, "select": ["state.total"],
        "condition": c("GTE", json!(["state.total", 0])),
        "orderBy": [{"path": "state.total", "desc": true}], "limit": 5}),
    )
    .await;
    let mut totals: Vec<f64> = latest(&runs)
        .iter()
        .filter_map(|run| run.state.as_ref()?.get("total")?.as_f64())
        .collect();
    totals.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let got: Vec<f64> = got
        .rows
        .iter()
        .map(|row| row[0].as_f64().unwrap())
        .collect();
    assert_eq!(got, totals[..5].to_vec());

    // Groups ordered by an aggregate.
    let got = ask(
        &repository,
        tenant,
        &json!({"unnest": "state.items", "groupBy": ["item.sku"],
        "aggregates": [{"fn": "sum", "path": "item.qty", "as": "qty"}],
        "orderBy": [{"path": "qty", "desc": true}], "limit": 2}),
    )
    .await;
    let mut want = reference(
        &runs,
        &json!({"unnest": "state.items", "groupBy": ["item.sku"],
        "aggregates": [{"fn": "sum", "path": "item.qty"}]}),
    );
    want.sort_by(|a, b| b[1].as_f64().partial_cmp(&a[1].as_f64()).unwrap());
    assert_eq!(got.columns, vec!["item.sku", "qty"]);
    assert_eq!(sorted(&got.rows[..1]), sorted(&want[..1]));
    assert!(got.has_more);

    // Queries that cannot be answered are refused, not run.
    for bad in [
        json!({"condition": c("LIKE", json!(["state.a", "x"]))}),
        json!({"condition": c("GT", json!(["state.a", {"x": 1}]))}),
        json!({"groupBy": ["state.items[*].sku"]}),
        json!({"unnest": "label"}),
        json!({"select": ["item"]}),
    ] {
        let parsed: RunQuery = serde_json::from_value(bad.clone()).unwrap();
        assert!(
            repository
                .query_runs(tenant, "orders", parsed)
                .await
                .is_err(),
            "{bad}"
        );
    }

    eprintln!("{checked} queries matched the reference");
    drop_database(pool, &name).await;
}
