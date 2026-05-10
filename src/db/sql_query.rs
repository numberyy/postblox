//! Read-only ad-hoc SQL surface for the agent-facing MCP tool.
//!
//! Defense-in-depth:
//! 1. Caller passes a separate read-only [`SqlitePool`] (opened via
//!    [`super::connect_readonly`]).
//! 2. Statement-level keyword scan rejects DDL/DML before submission.
//! 3. LIMIT cap is enforced by SQLite around the user query.

use base64::Engine;
use serde_json::{Map, Value};
use sqlx::{Column, Row, SqlitePool, TypeInfo, ValueRef};
use std::time::Duration;
use tokio::time::timeout;

/// Error returned by the agent-facing read-only SQL surface.
#[derive(Debug, thiserror::Error)]
pub enum SqlError {
    /// Query was rejected by the static safety filter before execution.
    #[error("query rejected: {reason}")]
    Rejected {
        /// Human-readable reason the statement was refused.
        reason: String,
    },
    /// Underlying SQLite or SQLx error during execution.
    #[error("sqlite error: {0}")]
    Sqlx(#[from] sqlx::Error),
}

/// Maximum rows returned by a single query.
pub const MAX_ROWS: usize = 1000;
/// Default cap when the caller doesn't pass a `limit`.
pub const DEFAULT_ROWS: usize = 200;
/// Maximum size of the submitted SQL string, in bytes.
pub const MAX_SQL_BYTES: usize = 16 * 1024;
/// Maximum raw bytes accepted for a single TEXT or BLOB cell.
pub const MAX_CELL_BYTES: usize = 64 * 1024;
/// Maximum approximate JSON payload bytes across returned cell values.
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// Best-effort timeout around query execution at the async future boundary.
const QUERY_TIMEOUT: Duration = Duration::from_secs(2);

/// Statement-level rejection list (case-insensitive tokens).
const FORBIDDEN_KEYWORDS: &[&str] = &[
    "INSERT",
    "UPDATE",
    "DELETE",
    "REPLACE",
    "MERGE",
    "CREATE",
    "DROP",
    "ALTER",
    "TRUNCATE",
    "ATTACH",
    "DETACH",
    "VACUUM",
    "REINDEX",
    "BEGIN",
    "COMMIT",
    "ROLLBACK",
    "SAVEPOINT",
    "RELEASE",
    // we expose schema via sql_schema instead
    "PRAGMA",
];

/// Reject any forbidden keyword that appears as a whole-word token
/// (case-insensitive). Comments and strings are NOT stripped — this is
/// defense in depth, not a parser. Combined with the read-only pool,
/// this is sufficient because even if a forbidden token slips through
/// in a string literal, SQLite refuses writes on a `mode=ro` connection.
///
/// # Errors
///
/// Returns [`SqlError::Rejected`] when `sql` is empty, doesn't start
/// with `SELECT` or `WITH`, or contains a forbidden keyword as a
/// standalone identifier-shaped token.
pub fn validate_query(sql: &str) -> Result<(), SqlError> {
    if sql.len() > MAX_SQL_BYTES {
        return Err(SqlError::Rejected {
            reason: format!(
                "sql string is {} bytes, exceeds {MAX_SQL_BYTES} byte limit",
                sql.len()
            ),
        });
    }

    let sql = trim_one_trailing_semicolon(sql);
    if sql.is_empty() {
        return Err(SqlError::Rejected {
            reason: "statement must not be empty".into(),
        });
    }

    let upper = sql.to_uppercase();
    for token in upper.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
        if token.is_empty() {
            continue;
        }
        for kw in FORBIDDEN_KEYWORDS {
            if token == *kw {
                return Err(SqlError::Rejected {
                    reason: format!("statement contains forbidden keyword: {kw}"),
                });
            }
        }
    }

    let first_token = sql
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .find(|token| !token.is_empty())
        .unwrap_or_default()
        .to_ascii_uppercase();
    if first_token != "SELECT" && first_token != "WITH" {
        return Err(SqlError::Rejected {
            reason: "statement must start with SELECT or WITH".into(),
        });
    }

    Ok(())
}

/// Execute `sql` on a read-only pool, returning at most `limit` rows
/// as JSON objects. Each row is `{ "column_name": <serde_json::Value> }`.
///
/// # Errors
///
/// - [`SqlError::Rejected`] if the statement isn't accepted as read-only.
/// - [`SqlError::Sqlx`] if SQLite refuses the query (e.g. an INSERT
///   that bypasses the keyword scan because it's hidden in a CTE
///   alias — the read-only mode rejects it at connect time).
pub async fn query(
    pool: &SqlitePool,
    sql: &str,
    limit: usize,
) -> Result<Vec<Map<String, Value>>, SqlError> {
    validate_query(sql)?;
    let sql = trim_one_trailing_semicolon(sql);
    let cap = limit.clamp(1, MAX_ROWS);
    let capped_sql = format!("SELECT * FROM ({sql}) AS postblox_agent_query LIMIT ?");
    let rows = timeout(
        QUERY_TIMEOUT,
        sqlx::query(&capped_sql).bind(cap as i64).fetch_all(pool),
    )
    .await
    .map_err(|_| SqlError::Rejected {
        reason: format!("query timed out after {} ms", QUERY_TIMEOUT.as_millis()),
    })??;
    let mut out = Vec::with_capacity(rows.len());
    let mut budget = ResponseBudget::default();
    for row in rows {
        let mut obj = Map::new();
        for (i, col) in row.columns().iter().enumerate() {
            let name = col.name().to_string();
            let value = sqlite_value_to_json(&row, i, &name)?;
            budget.account(json_value_bytes(&value))?;
            obj.insert(name, value);
        }
        out.push(obj);
    }
    Ok(out)
}

/// Dump every CREATE statement from `sqlite_master` (tables, views,
/// indexes, triggers, virtual tables). Returned as `[{ "type": ...,
/// "name": ..., "sql": ... }]`, ordered by type then name.
///
/// # Errors
///
/// Returns [`SqlError::Sqlx`] on connect/query failure.
pub async fn schema(pool: &SqlitePool) -> Result<Vec<Map<String, Value>>, SqlError> {
    let q = "SELECT type, name, sql FROM sqlite_master \
             WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' \
             ORDER BY type, name";
    let rows = sqlx::query(q).fetch_all(pool).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let mut obj = Map::new();
        for (i, col) in row.columns().iter().enumerate() {
            obj.insert(
                col.name().to_string(),
                sqlite_value_to_json(&row, i, col.name())?,
            );
        }
        out.push(obj);
    }
    Ok(out)
}

#[derive(Default)]
struct ResponseBudget {
    used_bytes: usize,
}

impl ResponseBudget {
    fn account(&mut self, cell_bytes: usize) -> Result<(), SqlError> {
        self.used_bytes =
            self.used_bytes
                .checked_add(cell_bytes)
                .ok_or_else(|| SqlError::Rejected {
                    reason: format!("query response exceeds {MAX_RESPONSE_BYTES} byte limit"),
                })?;
        if self.used_bytes > MAX_RESPONSE_BYTES {
            return Err(SqlError::Rejected {
                reason: format!(
                    "query response is {} bytes, exceeds {MAX_RESPONSE_BYTES} byte limit",
                    self.used_bytes
                ),
            });
        }
        Ok(())
    }
}

fn sqlite_value_to_json(
    row: &sqlx::sqlite::SqliteRow,
    idx: usize,
    column_name: &str,
) -> Result<Value, SqlError> {
    let raw = row.try_get_raw(idx)?;
    if raw.is_null() {
        return Ok(Value::Null);
    }
    let type_info = raw.type_info();
    let type_name = type_info.name();
    let v: Value = match type_name {
        "INTEGER" | "INT" | "BIGINT" | "BOOLEAN" => row
            .try_get::<Option<i64>, _>(idx)?
            .map_or(Value::Null, |i| Value::Number(i.into())),
        "REAL" | "FLOAT" | "DOUBLE" => row
            .try_get::<Option<f64>, _>(idx)?
            .and_then(|f| serde_json::Number::from_f64(f).map(Value::Number))
            .unwrap_or(Value::Null),
        "TEXT" => match row.try_get::<Option<&str>, _>(idx)? {
            Some(text) => {
                ensure_cell_bytes(column_name, "text", text.len())?;
                Value::String(text.to_owned())
            }
            None => Value::Null,
        },
        "BLOB" => match row.try_get::<Option<&[u8]>, _>(idx)? {
            Some(bytes) => {
                ensure_cell_bytes(column_name, "blob", bytes.len())?;
                Value::String(base64::engine::general_purpose::STANDARD.encode(bytes))
            }
            None => Value::Null,
        },
        // Unknown / NULL-typed columns: try generic serde via String fallback.
        _ => match row.try_get::<Option<&str>, _>(idx)? {
            Some(text) => {
                ensure_cell_bytes(column_name, "text", text.len())?;
                Value::String(text.to_owned())
            }
            None => Value::Null,
        },
    };
    Ok(v)
}

fn ensure_cell_bytes(column_name: &str, value_kind: &str, bytes: usize) -> Result<(), SqlError> {
    if bytes > MAX_CELL_BYTES {
        return Err(SqlError::Rejected {
            reason: format!(
                "{value_kind} cell '{column_name}' is {bytes} bytes, exceeds {MAX_CELL_BYTES} byte limit"
            ),
        });
    }
    Ok(())
}

fn json_value_bytes(value: &Value) -> usize {
    match value {
        Value::Null => 4,
        Value::Bool(true) => 4,
        Value::Bool(false) => 5,
        Value::Number(number) => number.to_string().len(),
        Value::String(text) => json_string_bytes(text),
        Value::Array(values) => values
            .iter()
            .map(json_value_bytes)
            .fold(2, |sum, bytes| sum.saturating_add(bytes).saturating_add(1)),
        Value::Object(map) => map.iter().fold(2, |sum, (key, value)| {
            sum.saturating_add(json_string_bytes(key))
                .saturating_add(json_value_bytes(value))
                .saturating_add(2)
        }),
    }
}

fn json_string_bytes(text: &str) -> usize {
    text.chars().fold(2, |sum, ch| {
        sum.saturating_add(match ch {
            '"' | '\\' => 2,
            '\u{08}' | '\u{0C}' | '\n' | '\r' | '\t' => 2,
            '\u{00}'..='\u{1F}' => 6,
            ch => ch.len_utf8(),
        })
    })
}

fn trim_one_trailing_semicolon(sql: &str) -> &str {
    let trimmed = sql.trim();
    trimmed
        .strip_suffix(';')
        .map_or(trimmed, |without_semicolon| without_semicolon.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_pool;

    #[test]
    fn test_validate_query_accepts_select() {
        assert!(validate_query("SELECT * FROM messages").is_ok());
        assert!(validate_query("select id, subject from messages where id = 1").is_ok());
        assert!(validate_query(" SELECT 1; ").is_ok());
        assert!(
            validate_query("WITH t AS (SELECT 1) SELECT * FROM t").is_ok(),
            "WITH ... SELECT must be allowed"
        );
    }

    #[test]
    fn test_validate_query_rejects_insert() {
        let err = validate_query("INSERT INTO messages(id) VALUES (1)").unwrap_err();
        assert!(
            matches!(err, SqlError::Rejected { ref reason } if reason.contains("INSERT")),
            "expected rejected/INSERT, got: {err:?}"
        );
    }

    #[test]
    fn test_validate_query_rejects_non_select_statement() {
        let err = validate_query("EXPLAIN SELECT * FROM messages").unwrap_err();
        assert!(matches!(err, SqlError::Rejected { reason } if reason.contains("SELECT")));
    }

    #[test]
    fn test_validate_query_rejects_update() {
        let err = validate_query("update messages set subject='x'").unwrap_err();
        assert!(matches!(err, SqlError::Rejected { reason } if reason.contains("UPDATE")));
    }

    #[test]
    fn test_validate_query_rejects_delete() {
        let err = validate_query("DELETE FROM messages").unwrap_err();
        assert!(matches!(err, SqlError::Rejected { reason } if reason.contains("DELETE")));
    }

    #[test]
    fn test_validate_query_rejects_drop() {
        let err = validate_query("drop table messages").unwrap_err();
        assert!(matches!(err, SqlError::Rejected { reason } if reason.contains("DROP")));
    }

    #[test]
    fn test_validate_query_rejects_pragma_write() {
        // PRAGMA is intentionally on the rejection list — schema/config
        // discovery goes through sql_schema, not arbitrary PRAGMAs.
        let err = validate_query("PRAGMA writable_schema = 1").unwrap_err();
        assert!(matches!(err, SqlError::Rejected { reason } if reason.contains("PRAGMA")));
    }

    #[test]
    fn test_validate_query_rejects_attach() {
        let err = validate_query("ATTACH DATABASE 'evil.db' AS evil").unwrap_err();
        assert!(matches!(err, SqlError::Rejected { reason } if reason.contains("ATTACH")));
    }

    #[test]
    fn test_validate_query_rejects_keyword_with_no_surrounding_spaces() {
        // The keyword scan must catch tokens regardless of whitespace
        // (e.g. "INSERT(", ";INSERT", or trailing newlines).
        assert!(validate_query("SELECT 1;DROP TABLE messages").is_err());
        assert!(validate_query("SELECT 1\nDELETE FROM messages").is_err());
    }

    #[test]
    fn test_validate_query_does_not_match_substring_inside_identifier() {
        // "INSERTED" contains "INSERT" as a substring but is a different
        // token, so it must NOT be rejected.
        assert!(validate_query("SELECT inserted_at FROM messages").is_ok());
        assert!(validate_query("SELECT * FROM dropbox_items").is_ok());
    }

    #[tokio::test]
    async fn test_query_returns_rows_as_json_objects() {
        // Use the rwc test_pool to seed; the keyword scan is what's
        // actually being tested here, not the RO connection.
        let pool = test_pool().await;
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO accounts \
             (id, email, display_name, auth_kind, secret_ref, \
              imap_host, imap_port, imap_use_tls, smtp_host, smtp_port, \
              smtp_use_tls, smtp_starttls, created_at) \
             VALUES (?, 'a@b.com', 'A', 'password', NULL, \
                     'imap.example', 993, 1, 'smtp.example', 587, 0, 1, '2026-01-01T00:00:00Z')",
        )
        .bind(&id)
        .execute(&pool)
        .await
        .unwrap();

        let rows = query(&pool, "SELECT id, email FROM accounts", 10)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], Value::String(id));
        assert_eq!(rows[0]["email"], Value::String("a@b.com".into()));
    }

    #[tokio::test]
    async fn test_query_overlong_sql_rejected() {
        let pool = test_pool().await;
        let sql = format!("SELECT 1 AS one{}", " ".repeat(MAX_SQL_BYTES + 1));
        let err = query(&pool, &sql, 10).await.unwrap_err();
        assert!(
            matches!(err, SqlError::Rejected { ref reason } if reason.contains("sql string")),
            "expected sql length rejection, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn test_query_oversized_text_rejected() {
        let pool = test_pool().await;
        let sql = format!(
            "SELECT hex(zeroblob({})) AS big_text",
            (MAX_CELL_BYTES / 2) + 1
        );
        let err = query(&pool, &sql, 10).await.unwrap_err();
        assert!(
            matches!(err, SqlError::Rejected { ref reason } if reason.contains("text cell 'big_text'")),
            "expected text cell rejection, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn test_query_oversized_blob_rejected() {
        let pool = test_pool().await;
        let sql = format!("SELECT zeroblob({}) AS big_blob", MAX_CELL_BYTES + 1);
        let err = query(&pool, &sql, 10).await.unwrap_err();
        assert!(
            matches!(err, SqlError::Rejected { ref reason } if reason.contains("blob cell 'big_blob'")),
            "expected blob cell rejection, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn test_query_total_response_cap_rejected() {
        let pool = test_pool().await;
        let rows_needed = (MAX_RESPONSE_BYTES / MAX_CELL_BYTES) + 1;
        let sql = format!(
            "WITH RECURSIVE n(x) AS ( \
                 VALUES(1) \
                 UNION ALL \
                 SELECT x + 1 FROM n WHERE x < {rows_needed} \
             ) \
             SELECT hex(zeroblob({})) AS chunk \
             FROM n",
            MAX_CELL_BYTES / 2
        );
        let err = query(&pool, &sql, rows_needed).await.unwrap_err();
        assert!(
            matches!(err, SqlError::Rejected { ref reason } if reason.contains("query response")),
            "expected total response rejection, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn test_query_small_values_encode() {
        let pool = test_pool().await;
        let rows = query(
            &pool,
            "SELECT 'hello' AS text_value, X'000102' AS blob_value, \
                    42 AS int_value, 1.5 AS real_value, NULL AS null_value",
            10,
        )
        .await
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["text_value"], Value::String("hello".into()));
        assert_eq!(rows[0]["blob_value"], Value::String("AAEC".into()));
        assert_eq!(rows[0]["int_value"], Value::Number(42.into()));
        assert_eq!(
            rows[0]["real_value"],
            Value::Number(serde_json::Number::from_f64(1.5).unwrap())
        );
        assert_eq!(rows[0]["null_value"], Value::Null);
    }

    #[tokio::test]
    async fn test_query_clamps_to_max_rows() {
        let pool = test_pool().await;
        for i in 0..50 {
            let id = uuid::Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO accounts \
                 (id, email, display_name, auth_kind, secret_ref, \
                  imap_host, imap_port, imap_use_tls, smtp_host, smtp_port, \
                  smtp_use_tls, smtp_starttls, created_at) \
                 VALUES (?, ?, 'A', 'password', NULL, \
                         'imap.example', 993, 1, 'smtp.example', 587, 0, 1, '2026-01-01T00:00:00Z')",
            )
            .bind(&id)
            .bind(format!("user{i}@b.com"))
            .execute(&pool)
            .await
            .unwrap();
        }
        let rows = query(&pool, "SELECT id FROM accounts", 10).await.unwrap();
        assert_eq!(rows.len(), 10, "limit=10 must clamp output to 10 rows");
    }

    #[tokio::test]
    async fn test_query_enforces_limit_in_sql_before_row_conversion() {
        let pool = test_pool().await;
        let rows = query(
            &pool,
            "WITH RECURSIVE n(x) AS ( \
                 VALUES(1) \
                 UNION ALL \
                 SELECT x + 1 FROM n WHERE x < 20 \
             ) \
             SELECT CASE \
                 WHEN x <= 10 THEN x \
                 ELSE abs(-9223372036854775808) \
             END AS value \
             FROM n",
            10,
        )
        .await
        .unwrap();
        assert_eq!(rows.len(), 10);
        assert_eq!(rows[0]["value"], Value::Number(1.into()));
        assert_eq!(rows[9]["value"], Value::Number(10.into()));
    }

    #[tokio::test]
    async fn test_query_clamps_to_default_when_zero_passed() {
        // limit=0 is meaningless; clamp to at least 1 (caller still
        // gets one row if any exist).
        let pool = test_pool().await;
        let rows = query(&pool, "SELECT 1 AS one", 0).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["one"], Value::Number(1.into()));
    }

    #[tokio::test]
    async fn test_query_returns_null_for_null_columns() {
        let pool = test_pool().await;
        let rows = query(&pool, "SELECT NULL AS nada, 7 AS seven", 10)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["nada"], Value::Null);
        assert_eq!(rows[0]["seven"], Value::Number(7.into()));
    }

    #[tokio::test]
    async fn test_query_encodes_blob_columns_as_base64() {
        let pool = test_pool().await;
        let rows = query(&pool, "SELECT X'000102FF' AS bytes", 10)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["bytes"], Value::String("AAEC/w==".into()));
    }

    #[tokio::test]
    async fn test_query_rejects_forbidden_keyword_before_submission() {
        let pool = test_pool().await;
        let err = query(&pool, "DELETE FROM accounts", 10).await.unwrap_err();
        assert!(matches!(err, SqlError::Rejected { .. }));
    }

    #[tokio::test]
    async fn test_schema_returns_create_statements_for_all_tables() {
        let pool = test_pool().await;
        let rows = schema(&pool).await.unwrap();
        let names: Vec<String> = rows
            .iter()
            .filter_map(|row| row.get("name").and_then(Value::as_str).map(String::from))
            .collect();
        for expected in ["accounts", "folders", "messages", "threads"] {
            assert!(
                names.iter().any(|n| n == expected),
                "schema dump missing table '{expected}', got: {names:?}"
            );
        }
        // Every row must carry a non-null SQL definition.
        for row in &rows {
            let sql = row.get("sql").and_then(Value::as_str).unwrap_or("");
            assert!(!sql.is_empty(), "row {row:?} had empty sql");
        }
    }
}
