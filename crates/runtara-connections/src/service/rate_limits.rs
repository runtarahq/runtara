//! Rate Limit Analytics Service
//!
//! Business logic for rate limit status endpoints
//! Combines PostgreSQL configuration with Redis runtime state

use crate::repository::connections::ConnectionRepository;
use crate::types::*;
use redis::Commands;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;

pub struct RateLimitService {
    connection_repository: Arc<ConnectionRepository>,
    redis_url: Option<String>,
    db_pool: Option<PgPool>,
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum ServiceError {
    NotFound(String),
    DatabaseError(String),
    RedisError(String),
}

/// Parsed bucket state from Redis
struct ParsedBucketState {
    tokens: Option<f64>,
    last_refill: Option<i64>,
    calls_in_window: Option<u32>,
    total_calls: Option<u64>,
    window_start: Option<i64>,
}

impl RateLimitService {
    pub fn new(connection_repository: Arc<ConnectionRepository>) -> Self {
        Self {
            connection_repository,
            redis_url: None,
            db_pool: None,
        }
    }

    /// Create service with a specific Redis URL
    pub fn with_redis_url(
        connection_repository: Arc<ConnectionRepository>,
        redis_url: Option<String>,
    ) -> Self {
        Self {
            connection_repository,
            redis_url,
            db_pool: None,
        }
    }

    /// Create service with database pool for timeline queries
    pub fn with_db_pool(connection_repository: Arc<ConnectionRepository>, db_pool: PgPool) -> Self {
        Self {
            connection_repository,
            redis_url: None,
            db_pool: Some(db_pool),
        }
    }

    /// Create service with both Redis URL and database pool
    pub fn with_redis_url_and_db_pool(
        connection_repository: Arc<ConnectionRepository>,
        redis_url: Option<String>,
        db_pool: PgPool,
    ) -> Self {
        Self {
            connection_repository,
            redis_url,
            db_pool: Some(db_pool),
        }
    }

    /// Get rate limit status for a single connection
    pub async fn get_connection_rate_limit_status(
        &self,
        connection_id: &str,
        tenant_id: &str,
    ) -> Result<RateLimitStatusDto, ServiceError> {
        // Fetch connection from PostgreSQL
        let connection = self
            .connection_repository
            .get_by_id(connection_id, tenant_id)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?
            .ok_or_else(|| ServiceError::NotFound("Connection not found".to_string()))?;

        let config = connection.rate_limit_config;

        // Fetch Redis state
        let state = self.fetch_redis_state(connection_id);

        // Compute metrics
        let metrics = self.compute_metrics(&config, &state);

        Ok(RateLimitStatusDto {
            connection_id: connection.id,
            connection_title: connection.title,
            integration_id: connection.integration_id,
            config,
            state,
            metrics,
            period_stats: None,
        })
    }

    /// Get rate limit status for all tenant connections
    pub async fn list_all_rate_limits(
        &self,
        tenant_id: &str,
        interval: Option<&str>,
    ) -> Result<Vec<RateLimitStatusDto>, ServiceError> {
        // Validate interval if provided
        let interval = interval.unwrap_or("24h");
        Self::parse_interval(interval)?;

        // Fetch all connections from PostgreSQL
        let connections = self
            .connection_repository
            .list(tenant_id, None, None)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;

        if connections.is_empty() {
            return Ok(vec![]);
        }

        // Get connection IDs for batch Redis query
        let connection_ids: Vec<String> = connections.iter().map(|c| c.id.clone()).collect();

        // Batch fetch Redis states
        let states = self.fetch_redis_states_batch(&connection_ids);

        // Fetch period stats for all connections
        let period_stats = self
            .get_period_stats_for_connections(tenant_id, &connection_ids, interval)
            .await?;

        // Build response for each connection
        let results = connections
            .into_iter()
            .map(|conn| {
                let config = conn.rate_limit_config;
                let state = states.get(&conn.id).cloned().unwrap_or_default();
                let metrics = self.compute_metrics(&config, &state);
                let stats = period_stats.get(&conn.id).cloned();

                RateLimitStatusDto {
                    connection_id: conn.id,
                    connection_title: conn.title,
                    integration_id: conn.integration_id,
                    config,
                    state,
                    metrics,
                    period_stats: stats,
                }
            })
            .collect();

        Ok(results)
    }

    /// Fetch rate limit state from Redis for a single connection
    fn fetch_redis_state(&self, connection_id: &str) -> RateLimitStateDto {
        let Some(ref url) = self.redis_url else {
            return RateLimitStateDto::default();
        };

        let result = self.fetch_redis_state_internal(url, connection_id);
        result.unwrap_or_default()
    }

    /// Internal helper to fetch Redis state with error handling
    fn fetch_redis_state_internal(
        &self,
        url: &str,
        connection_id: &str,
    ) -> Result<RateLimitStateDto, redis::RedisError> {
        let client = redis::Client::open(url)?;
        let mut conn = client.get_connection()?;

        let key = format!("rate_limit:{}", connection_id);
        let learned_key = format!("rate_limit_learned:{}", connection_id);

        // Fetch token bucket state
        let bucket: HashMap<String, String> = conn.hgetall(&key)?;

        let current_tokens = bucket.get("tokens").and_then(|s| s.parse::<f64>().ok());

        let last_refill_ms = bucket
            .get("last_refill")
            .and_then(|s| s.parse::<i64>().ok());

        // Fetch call tracking fields
        let calls_in_window = bucket
            .get("calls_window")
            .and_then(|s| s.parse::<u32>().ok());

        let total_calls = bucket
            .get("calls_total")
            .and_then(|s| s.parse::<u64>().ok());

        let window_start_ms = bucket
            .get("window_start")
            .and_then(|s| s.parse::<i64>().ok());

        // Fetch learned limit
        let learned_limit: Option<u32> = conn.get(&learned_key).ok();

        Ok(RateLimitStateDto {
            available: true,
            current_tokens,
            last_refill_ms,
            learned_limit,
            calls_in_window,
            total_calls,
            window_start_ms,
        })
    }

    /// Batch fetch Redis states for multiple connections
    fn fetch_redis_states_batch(
        &self,
        connection_ids: &[String],
    ) -> HashMap<String, RateLimitStateDto> {
        let mut states = HashMap::new();

        let Some(ref url) = self.redis_url else {
            // Redis not configured - return default states
            for id in connection_ids {
                states.insert(id.clone(), RateLimitStateDto::default());
            }
            return states;
        };

        // Try to batch fetch, fall back to defaults on error
        match self.fetch_redis_states_batch_internal(url, connection_ids) {
            Ok(fetched) => fetched,
            Err(_) => {
                // Redis error - return unavailable states
                for id in connection_ids {
                    states.insert(id.clone(), RateLimitStateDto::default());
                }
                states
            }
        }
    }

    /// Internal helper for batch Redis fetch
    fn fetch_redis_states_batch_internal(
        &self,
        url: &str,
        connection_ids: &[String],
    ) -> Result<HashMap<String, RateLimitStateDto>, redis::RedisError> {
        let client = redis::Client::open(url)?;
        let mut conn = client.get_connection()?;

        let mut states = HashMap::new();

        // Build pipeline for efficient batch query
        let mut pipe = redis::pipe();

        for conn_id in connection_ids {
            let key = format!("rate_limit:{}", conn_id);
            let learned_key = format!("rate_limit_learned:{}", conn_id);

            pipe.hgetall(&key);
            pipe.get(&learned_key);
        }

        // Execute pipeline
        let results: Vec<redis::Value> = pipe.query(&mut conn)?;

        // Parse results (2 values per connection: hash + learned)
        for (idx, conn_id) in connection_ids.iter().enumerate() {
            let hash_idx = idx * 2;
            let learned_idx = hash_idx + 1;

            // Parse hash result
            let parsed = if hash_idx < results.len() {
                self.parse_bucket_hash(&results[hash_idx])
            } else {
                ParsedBucketState {
                    tokens: None,
                    last_refill: None,
                    calls_in_window: None,
                    total_calls: None,
                    window_start: None,
                }
            };

            // Parse learned limit
            let learned_limit = if learned_idx < results.len() {
                self.parse_learned_limit(&results[learned_idx])
            } else {
                None
            };

            states.insert(
                conn_id.clone(),
                RateLimitStateDto {
                    available: true,
                    current_tokens: parsed.tokens,
                    last_refill_ms: parsed.last_refill,
                    learned_limit,
                    calls_in_window: parsed.calls_in_window,
                    total_calls: parsed.total_calls,
                    window_start_ms: parsed.window_start,
                },
            );
        }

        Ok(states)
    }

    /// Parse token bucket hash from Redis value
    fn parse_bucket_hash(&self, value: &redis::Value) -> ParsedBucketState {
        use redis::Value;

        match value {
            Value::Array(items) => {
                let mut tokens = None;
                let mut last_refill = None;
                let mut calls_in_window = None;
                let mut total_calls = None;
                let mut window_start = None;

                // Hash is returned as [key1, val1, key2, val2, ...]
                let mut iter = items.iter();
                while let Some(key) = iter.next() {
                    let val = iter.next();
                    if let (Value::BulkString(key_bytes), Some(Value::BulkString(val_bytes))) =
                        (key, val)
                    {
                        let key_str = String::from_utf8_lossy(key_bytes);
                        let val_str = String::from_utf8_lossy(val_bytes);

                        match key_str.as_ref() {
                            "tokens" => tokens = val_str.parse().ok(),
                            "last_refill" => last_refill = val_str.parse().ok(),
                            "calls_window" => calls_in_window = val_str.parse().ok(),
                            "calls_total" => total_calls = val_str.parse().ok(),
                            "window_start" => window_start = val_str.parse().ok(),
                            _ => {}
                        }
                    }
                }

                ParsedBucketState {
                    tokens,
                    last_refill,
                    calls_in_window,
                    total_calls,
                    window_start,
                }
            }
            _ => ParsedBucketState {
                tokens: None,
                last_refill: None,
                calls_in_window: None,
                total_calls: None,
                window_start: None,
            },
        }
    }

    /// Parse learned limit from Redis value
    fn parse_learned_limit(&self, value: &redis::Value) -> Option<u32> {
        use redis::Value;

        match value {
            Value::BulkString(bytes) => String::from_utf8_lossy(bytes).parse().ok(),
            Value::Int(i) => Some(*i as u32),
            _ => None,
        }
    }

    /// Compute metrics from config and state
    fn compute_metrics(
        &self,
        config: &Option<RateLimitConfigDto>,
        state: &RateLimitStateDto,
    ) -> RateLimitMetricsDto {
        if !state.available {
            return RateLimitMetricsDto::default();
        }

        let Some(cfg) = config else {
            return RateLimitMetricsDto::default();
        };

        let Some(tokens) = state.current_tokens else {
            return RateLimitMetricsDto::default();
        };

        let capacity_percent = (tokens / cfg.burst_size as f64) * 100.0;
        let utilization_percent = 100.0 - capacity_percent;
        let is_rate_limited = tokens < 1.0;

        let retry_after_ms = if is_rate_limited && cfg.requests_per_second > 0 {
            let tokens_needed = 1.0 - tokens;
            let wait_secs = tokens_needed / cfg.requests_per_second as f64;
            Some((wait_secs * 1000.0).ceil() as u64)
        } else {
            None
        };

        RateLimitMetricsDto {
            capacity_percent: Some(capacity_percent.clamp(0.0, 100.0)),
            utilization_percent: Some(utilization_percent.clamp(0.0, 100.0)),
            is_rate_limited,
            retry_after_ms,
        }
    }

    // ========================================================================
    // Timeline Methods
    // ========================================================================

    /// Parse timeline granularity string to DATE_TRUNC argument
    fn parse_timeline_granularity(granularity: &str) -> Result<&'static str, ServiceError> {
        match granularity {
            "minute" => Ok("minute"),
            "hourly" => Ok("hour"),
            "daily" => Ok("day"),
            _ => Err(ServiceError::DatabaseError(format!(
                "Invalid granularity '{}'. Valid values: minute, hourly, daily",
                granularity
            ))),
        }
    }

    /// Get time-bucketed rate limit event counts for a connection
    pub async fn get_rate_limit_timeline(
        &self,
        connection_id: &str,
        tenant_id: &str,
        query: &RateLimitTimelineQuery,
    ) -> Result<RateLimitTimelineResponse, ServiceError> {
        let Some(ref pool) = self.db_pool else {
            return Err(ServiceError::DatabaseError(
                "Database pool not configured".to_string(),
            ));
        };

        let trunc_arg = Self::parse_timeline_granularity(&query.granularity)?;

        let end_time = query.end_time.unwrap_or_else(chrono::Utc::now);
        let start_time = query
            .start_time
            .unwrap_or_else(|| end_time - chrono::Duration::hours(1));

        let rows = sqlx::query_as::<_, TimelineBucketRow>(
            r#"
            SELECT
                DATE_TRUNC($1, created_at) AS bucket,
                COUNT(*) FILTER (WHERE event_type = 'request') AS request_count,
                COUNT(*) FILTER (WHERE event_type = 'rate_limited') AS rate_limited_count,
                COUNT(*) FILTER (WHERE event_type = 'retry') AS retry_count
            FROM rate_limit_events
            WHERE tenant_id = $2
              AND connection_id = $3
              AND created_at >= $4
              AND created_at < $5
              AND ($6::text IS NULL OR metadata->>'tag' = $6)
            GROUP BY bucket
            ORDER BY bucket ASC
            "#,
        )
        .bind(trunc_arg)
        .bind(tenant_id)
        .bind(connection_id)
        .bind(start_time)
        .bind(end_time)
        .bind(&query.tag)
        .fetch_all(pool)
        .await
        .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;

        let buckets: Vec<RateLimitTimelineBucket> = rows
            .into_iter()
            .map(|row| RateLimitTimelineBucket {
                bucket: row.bucket,
                request_count: row.request_count,
                rate_limited_count: row.rate_limited_count,
                retry_count: row.retry_count,
            })
            .collect();

        let bucket_count = buckets.len();

        Ok(RateLimitTimelineResponse {
            success: true,
            data: RateLimitTimelineData {
                connection_id: connection_id.to_string(),
                start_time,
                end_time,
                granularity: query.granularity.clone(),
                buckets,
            },
            bucket_count,
        })
    }

    // ========================================================================
    // Call Tracking Methods
    // ========================================================================

    /// Record a credential request (increments counters and logs event)
    ///
    /// Called when a workflow requests connection credentials.
    /// This tracks the request in both Redis (counters) and PostgreSQL (timeline).
    pub async fn record_credential_request(
        &self,
        connection_id: &str,
        tenant_id: &str,
        event_type: &RateLimitEventType,
        metadata: Option<serde_json::Value>,
    ) -> Result<(), ServiceError> {
        // Increment Redis counters
        if let Err(e) = self.increment_call_counters(connection_id) {
            tracing::warn!(
                connection_id = connection_id,
                error = %e,
                "Failed to increment rate limit counters in Redis"
            );
        }

        // Record event in PostgreSQL (if db pool available)
        if let Err(e) = self
            .insert_rate_limit_event(connection_id, tenant_id, event_type, metadata)
            .await
        {
            tracing::warn!(
                connection_id = connection_id,
                error = ?e,
                "Failed to insert rate limit event"
            );
        }

        Ok(())
    }

    /// Increment call counters in Redis
    fn increment_call_counters(&self, connection_id: &str) -> Result<(), redis::RedisError> {
        let Some(ref url) = self.redis_url else {
            return Ok(()); // Redis not configured, skip
        };

        let client = redis::Client::open(url.as_str())?;
        let mut conn = client.get_connection()?;

        let key = format!("rate_limit:{}", connection_id);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        // Use pipeline for atomic updates
        let mut pipe = redis::pipe();

        // Increment window calls
        pipe.hincr(&key, "calls_window", 1i64);

        // Increment total calls
        pipe.hincr(&key, "calls_total", 1i64);

        // Set window start if not already set (HSETNX)
        pipe.cmd("HSETNX").arg(&key).arg("window_start").arg(now_ms);

        let _: () = pipe.query(&mut conn)?;

        Ok(())
    }

    /// Insert a rate limit event into PostgreSQL
    async fn insert_rate_limit_event(
        &self,
        connection_id: &str,
        tenant_id: &str,
        event_type: &RateLimitEventType,
        metadata: Option<serde_json::Value>,
    ) -> Result<(), ServiceError> {
        let Some(ref pool) = self.db_pool else {
            return Ok(()); // DB pool not configured, skip
        };

        sqlx::query(
            r#"
            INSERT INTO rate_limit_events (tenant_id, connection_id, event_type, metadata)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(tenant_id)
        .bind(connection_id)
        .bind(event_type.to_string())
        .bind(metadata)
        .execute(pool)
        .await
        .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;

        Ok(())
    }

    /// Get rate limit history for a connection
    pub async fn get_rate_limit_history(
        &self,
        connection_id: &str,
        tenant_id: &str,
        query: &RateLimitHistoryQuery,
    ) -> Result<RateLimitHistoryResponse, ServiceError> {
        let Some(ref pool) = self.db_pool else {
            return Err(ServiceError::DatabaseError(
                "Database pool not configured".to_string(),
            ));
        };

        // Clamp limit to max 1000
        let limit = query.limit.clamp(1, 1000);
        let offset = query.offset.max(0);

        // Build query with optional filters
        let mut sql = String::from(
            r#"
            SELECT id, connection_id, event_type, created_at, metadata
            FROM rate_limit_events
            WHERE tenant_id = $1 AND connection_id = $2
            "#,
        );

        let mut param_idx = 3;

        if query.event_type.is_some() {
            sql.push_str(&format!(" AND event_type = ${}", param_idx));
            param_idx += 1;
        }

        if query.from.is_some() {
            sql.push_str(&format!(" AND created_at >= ${}", param_idx));
            param_idx += 1;
        }

        if query.to.is_some() {
            sql.push_str(&format!(" AND created_at <= ${}", param_idx));
            param_idx += 1;
        }

        sql.push_str(&format!(
            " ORDER BY created_at DESC LIMIT ${} OFFSET ${}",
            param_idx,
            param_idx + 1
        ));

        // Build and execute query
        let mut query_builder = sqlx::query_as::<_, RateLimitEventRow>(&sql)
            .bind(tenant_id)
            .bind(connection_id);

        if let Some(ref event_type) = query.event_type {
            query_builder = query_builder.bind(event_type);
        }

        if let Some(ref from) = query.from {
            query_builder = query_builder.bind(from);
        }

        if let Some(ref to) = query.to {
            query_builder = query_builder.bind(to);
        }

        query_builder = query_builder.bind(limit).bind(offset);

        let rows = query_builder
            .fetch_all(pool)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;

        // Get total count with same filters
        let total_count = self
            .get_rate_limit_event_count(pool, connection_id, tenant_id, query)
            .await?;

        let data = rows
            .into_iter()
            .map(|row| RateLimitEventDto {
                id: row.id,
                connection_id: row.connection_id,
                event_type: row.event_type,
                created_at: row.created_at,
                metadata: row.metadata,
            })
            .collect();

        Ok(RateLimitHistoryResponse {
            success: true,
            data,
            total_count,
            limit,
            offset,
        })
    }

    /// Get count of rate limit events matching filters
    async fn get_rate_limit_event_count(
        &self,
        pool: &PgPool,
        connection_id: &str,
        tenant_id: &str,
        query: &RateLimitHistoryQuery,
    ) -> Result<i64, ServiceError> {
        let mut sql = String::from(
            r#"
            SELECT COUNT(*) as count
            FROM rate_limit_events
            WHERE tenant_id = $1 AND connection_id = $2
            "#,
        );

        let mut param_idx = 3;

        if query.event_type.is_some() {
            sql.push_str(&format!(" AND event_type = ${}", param_idx));
            param_idx += 1;
        }

        if query.from.is_some() {
            sql.push_str(&format!(" AND created_at >= ${}", param_idx));
            param_idx += 1;
        }

        if query.to.is_some() {
            sql.push_str(&format!(" AND created_at <= ${}", param_idx));
        }

        let mut query_builder = sqlx::query_scalar::<_, i64>(&sql)
            .bind(tenant_id)
            .bind(connection_id);

        if let Some(ref event_type) = query.event_type {
            query_builder = query_builder.bind(event_type);
        }

        if let Some(ref from) = query.from {
            query_builder = query_builder.bind(from);
        }

        if let Some(ref to) = query.to {
            query_builder = query_builder.bind(to);
        }

        let count = query_builder
            .fetch_one(pool)
            .await
            .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;

        Ok(count)
    }

    /// Reset window counters (called when a new window starts)
    #[allow(dead_code)]
    pub fn reset_window_counters(&self, connection_id: &str) -> Result<(), redis::RedisError> {
        let Some(ref url) = self.redis_url else {
            return Ok(());
        };

        let client = redis::Client::open(url.as_str())?;
        let mut conn = client.get_connection()?;

        let key = format!("rate_limit:{}", connection_id);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        // Reset window counter and update window start
        let mut pipe = redis::pipe();
        pipe.hset(&key, "calls_window", 0i64);
        pipe.hset(&key, "window_start", now_ms);
        let _: () = pipe.query(&mut conn)?;

        Ok(())
    }

    /// Delete old rate limit events (for cleanup job)
    #[allow(dead_code)]
    pub async fn cleanup_old_events(&self, retention_days: i32) -> Result<i64, ServiceError> {
        let Some(ref pool) = self.db_pool else {
            return Err(ServiceError::DatabaseError(
                "Database pool not configured".to_string(),
            ));
        };

        let result = sqlx::query(
            r#"
            DELETE FROM rate_limit_events
            WHERE created_at < NOW() - ($1 || ' days')::INTERVAL
            "#,
        )
        .bind(retention_days)
        .execute(pool)
        .await
        .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;

        Ok(result.rows_affected() as i64)
    }

    /// Parse interval string to chrono::Duration
    /// Valid values: 1h, 24h, 7d, 30d
    pub fn parse_interval(interval: &str) -> Result<chrono::Duration, ServiceError> {
        match interval {
            "1h" => Ok(chrono::Duration::hours(1)),
            "24h" => Ok(chrono::Duration::hours(24)),
            "7d" => Ok(chrono::Duration::days(7)),
            "30d" => Ok(chrono::Duration::days(30)),
            _ => Err(ServiceError::DatabaseError(format!(
                "Invalid interval '{}'. Valid values: 1h, 24h, 7d, 30d",
                interval
            ))),
        }
    }

    /// Get aggregated event stats for all connections in a time period
    pub async fn get_period_stats_for_connections(
        &self,
        tenant_id: &str,
        connection_ids: &[String],
        interval: &str,
    ) -> Result<HashMap<String, PeriodStatsDto>, ServiceError> {
        let Some(ref pool) = self.db_pool else {
            // No db pool - return empty stats for all connections
            return Ok(connection_ids
                .iter()
                .map(|id| {
                    (
                        id.clone(),
                        PeriodStatsDto {
                            interval: interval.to_string(),
                            total_requests: 0,
                            rate_limited_count: 0,
                            retry_count: 0,
                            rate_limited_percent: 0.0,
                        },
                    )
                })
                .collect());
        };

        if connection_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let duration = Self::parse_interval(interval)?;
        let from_time = chrono::Utc::now() - duration;

        let rows = sqlx::query_as::<_, PeriodStatsRow>(
            r#"
            SELECT
                connection_id,
                COUNT(*) FILTER (WHERE event_type = 'request') as total_requests,
                COUNT(*) FILTER (WHERE event_type = 'rate_limited') as rate_limited_count,
                COUNT(*) FILTER (WHERE event_type = 'retry') as retry_count
            FROM rate_limit_events
            WHERE tenant_id = $1
              AND connection_id = ANY($2)
              AND created_at >= $3
            GROUP BY connection_id
            "#,
        )
        .bind(tenant_id)
        .bind(connection_ids)
        .bind(from_time)
        .fetch_all(pool)
        .await
        .map_err(|e| ServiceError::DatabaseError(e.to_string()))?;

        // Build result map from query results
        let mut result: HashMap<String, PeriodStatsDto> = rows
            .into_iter()
            .map(|row| {
                let rate_limited_percent = if row.total_requests > 0 {
                    (row.rate_limited_count as f64 / row.total_requests as f64) * 100.0
                } else {
                    0.0
                };
                (
                    row.connection_id.clone(),
                    PeriodStatsDto {
                        interval: interval.to_string(),
                        total_requests: row.total_requests,
                        rate_limited_count: row.rate_limited_count,
                        retry_count: row.retry_count,
                        rate_limited_percent,
                    },
                )
            })
            .collect();

        // Add empty stats for connections with no events in the period
        for conn_id in connection_ids {
            result
                .entry(conn_id.clone())
                .or_insert_with(|| PeriodStatsDto {
                    interval: interval.to_string(),
                    total_requests: 0,
                    rate_limited_count: 0,
                    retry_count: 0,
                    rate_limited_percent: 0.0,
                });
        }

        Ok(result)
    }
}

/// Row type for rate limit event queries
#[derive(sqlx::FromRow)]
struct RateLimitEventRow {
    id: i64,
    connection_id: String,
    event_type: String,
    created_at: chrono::DateTime<chrono::Utc>,
    metadata: Option<serde_json::Value>,
}

/// Row type for period stats aggregation query
#[derive(sqlx::FromRow)]
struct PeriodStatsRow {
    connection_id: String,
    total_requests: i64,
    rate_limited_count: i64,
    retry_count: i64,
}

/// Row type for timeline bucket query
#[derive(sqlx::FromRow)]
struct TimelineBucketRow {
    bucket: chrono::DateTime<chrono::Utc>,
    request_count: i64,
    rate_limited_count: i64,
    retry_count: i64,
}
