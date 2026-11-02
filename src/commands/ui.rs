use anyhow::Result;
use axum::{
    extract::{Path, Query, Request, State},
    http::{header, HeaderMap, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::{sqlite::SqlitePool, Column, Row, TypeInfo};
use std::sync::Arc;
use tokio::net::TcpListener;

const HTML: &str = include_str!("../ui/index.html");

struct AppState {
    pool: SqlitePool,
    db_path: String,
}

pub async fn run(db_path: &str, port: u16, allowed: &[String]) -> Result<()> {
    let url = format!("sqlite:{db_path}");
    let pool = SqlitePool::connect(&url).await?;
    let state = Arc::new(AppState {
        pool,
        db_path: db_path.to_string(),
    });

    let app = Router::new()
        .route("/", get(index))
        .route("/api/tables", get(list_tables))
        .route("/api/tables/{name}", get(table_data))
        .route("/api/tables/{name}/schema", get(table_schema))
        .route("/api/stats", get(stats))
        .route("/api/query", post(query))
        .layer(middleware::from_fn_with_state(
            Arc::new(allowed.to_vec()),
            same_origin,
        ))
        .with_state(state);

    let addr = format!("127.0.0.1:{port}");
    println!("Wallet Inspector running at http://{addr}");
    let listener = TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

/// The inspector's browser gate (#411), the served doors' rule on its own
/// routes: a call is admitted when its `Origin` is the inspector's own
/// (`http://` and the `Host` it was asked by) or one `ALLOWED_ORIGINS`
/// names. The one exemption: a `GET` or `HEAD` with no `Origin`, the only
/// requests a browser sends without one (its own page's reads, and a
/// command line's); a page of another origin cannot read their answers, as
/// the inspector answers no CORS. Every call must name a loopback `Host`,
/// so a page rebinding its name to this port is refused before any read.
async fn same_origin(
    State(allowed): State<Arc<Vec<String>>>,
    request: Request,
    next: Next,
) -> Response {
    match inspector_word(request.method(), request.headers(), &allowed) {
        Ok(()) => next.run(request).await,
        Err(why) => (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": why })),
        )
            .into_response(),
    }
}

fn inspector_word(method: &Method, headers: &HeaderMap, allowed: &[String]) -> Result<(), String> {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let name = url::Url::parse(&format!("http://{host}"))
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_default();
    if !matches!(name.as_str(), "localhost" | "127.0.0.1" | "[::1]") {
        return Err(format!(
            "the inspector answers only a loopback Host, not {host:?}"
        ));
    }
    match headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        None if matches!(*method, Method::GET | Method::HEAD) => Ok(()),
        None => Err("Origin header is required".to_string()),
        Some(origin)
            if origin == format!("http://{host}") || allowed.iter().any(|o| o == origin) =>
        {
            Ok(())
        }
        Some(origin) => Err(format!(
            "origin {origin} is neither the inspector's own nor in ALLOWED_ORIGINS"
        )),
    }
}

async fn index() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], HTML)
}

// ─── List tables with row counts ───

#[derive(Serialize)]
struct TableInfo {
    name: String,
    count: i64,
}

async fn list_tables(State(state): State<Arc<AppState>>) -> Json<Vec<TableInfo>> {
    let rows = sqlx::query("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '_sqlx_%' ORDER BY name")
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();

    let mut tables = Vec::new();
    for row in rows {
        let name: String = row.get("name");
        let count_row = sqlx::query(&format!(
            "SELECT COUNT(*) as cnt FROM \"{}\"",
            name.replace('"', "")
        ))
        .fetch_one(&state.pool)
        .await;
        let count = count_row.map(|r| r.get::<i64, _>("cnt")).unwrap_or(0);
        tables.push(TableInfo { name, count });
    }
    Json(tables)
}

// ─── Table data with pagination ───

#[derive(Deserialize)]
struct TableQuery {
    page: Option<i64>,
    limit: Option<i64>,
    filter: Option<String>,
}

#[derive(Serialize)]
struct TableData {
    columns: Vec<String>,
    rows: Vec<serde_json::Map<String, serde_json::Value>>,
    total: i64,
}

async fn table_data(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(params): Query<TableQuery>,
) -> Result<Json<TableData>, StatusCode> {
    let safe_name = name.replace('"', "");
    let page = params.page.unwrap_or(1).max(1);
    let limit = params.limit.unwrap_or(50).min(1000);
    let offset = (page - 1) * limit;

    // Get total count
    let total = if let Some(filter) = &params.filter {
        if filter.is_empty() {
            count_table(&state.pool, &safe_name).await
        } else {
            // Filter: search all text columns for the filter string
            count_filtered(&state.pool, &safe_name, filter).await
        }
    } else {
        count_table(&state.pool, &safe_name).await
    };

    // Get rows
    let sql = if let Some(filter) = &params.filter {
        if filter.is_empty() {
            format!("SELECT * FROM \"{safe_name}\" LIMIT {limit} OFFSET {offset}")
        } else {
            let where_clause = build_filter_where(&state.pool, &safe_name, filter).await;
            format!("SELECT * FROM \"{safe_name}\" {where_clause} LIMIT {limit} OFFSET {offset}")
        }
    } else {
        format!("SELECT * FROM \"{safe_name}\" LIMIT {limit} OFFSET {offset}")
    };

    let rows = sqlx::query(&sql)
        .fetch_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let columns: Vec<String> = if let Some(first) = rows.first() {
        first
            .columns()
            .iter()
            .map(|c| c.name().to_string())
            .collect()
    } else {
        // No rows - get columns from schema
        get_column_names(&state.pool, &safe_name).await
    };

    let json_rows: Vec<serde_json::Map<String, serde_json::Value>> = rows
        .iter()
        .map(|row| {
            let mut map = serde_json::Map::new();
            for col in row.columns() {
                let name = col.name().to_string();
                let val = sqlite_value_to_json(row, col);
                map.insert(name, val);
            }
            map
        })
        .collect();

    Ok(Json(TableData {
        columns,
        rows: json_rows,
        total,
    }))
}

// ─── Table schema ───

#[derive(Serialize)]
struct ColumnInfo {
    name: String,
    col_type: String,
    pk: bool,
    notnull: bool,
    dflt_value: Option<String>,
}

async fn table_schema(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Json<Vec<ColumnInfo>> {
    let safe_name = name.replace('"', "");
    let rows = sqlx::query(&format!("PRAGMA table_info(\"{}\")", safe_name))
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();

    let columns: Vec<ColumnInfo> = rows
        .iter()
        .map(|r| ColumnInfo {
            name: r.get("name"),
            col_type: r.get("type"),
            pk: r.get::<bool, _>("pk"),
            notnull: r.get::<bool, _>("notnull"),
            dflt_value: r.get("dflt_value"),
        })
        .collect();

    Json(columns)
}

// ─── Stats ───

#[derive(Serialize)]
struct BasketStats {
    name: String,
    sats: i64,
    utxos: i64,
}

#[derive(Serialize)]
struct WalletStats {
    balance: i64,
    utxo_count: i64,
    total_balance: i64,
    total_utxo_count: i64,
    baskets: Vec<BasketStats>,
    tx_count: i64,
    proven_tx_count: i64,
    certificate_count: i64,
    chain: String,
    identity_key: String,
    db_path: String,
    db_size: String,
}

/// Common WHERE clause for spendable outputs with valid transaction status.
const SPENDABLE_WHERE: &str = r#"
    o.spendable = 1
    AND t.status IN ('completed', 'unproven', 'nosend', 'sending')
"#;

async fn stats(State(state): State<Arc<AppState>>) -> Json<WalletStats> {
    // "default" basket balance — matches SDK's list_outputs(basket: "default")
    let balance_sql = format!(
        r#"SELECT COALESCE(SUM(o.satoshis), 0) as v
        FROM outputs o
        JOIN transactions t ON o.transaction_id = t.transaction_id
        JOIN output_baskets b ON o.basket_id = b.basket_id
        WHERE {} AND b.name = 'default' AND b.is_deleted = 0"#,
        SPENDABLE_WHERE
    );
    // Total across ALL baskets (including basketless outputs)
    let total_sql = format!(
        r#"SELECT COALESCE(SUM(o.satoshis), 0) as v
        FROM outputs o
        JOIN transactions t ON o.transaction_id = t.transaction_id
        WHERE {}"#,
        SPENDABLE_WHERE
    );
    let total_utxo_sql = format!(
        r#"SELECT COUNT(*) as v
        FROM outputs o
        JOIN transactions t ON o.transaction_id = t.transaction_id
        WHERE {}"#,
        SPENDABLE_WHERE
    );
    // Per-basket breakdown
    let basket_sql = format!(
        r#"SELECT COALESCE(b.name, '(no basket)') as name,
                  COALESCE(SUM(o.satoshis), 0) as sats,
                  COUNT(*) as utxos
        FROM outputs o
        JOIN transactions t ON o.transaction_id = t.transaction_id
        LEFT JOIN output_baskets b ON o.basket_id = b.basket_id
        WHERE {}
        GROUP BY b.name
        ORDER BY sats DESC"#,
        SPENDABLE_WHERE
    );

    let balance = query_i64(&state.pool, &balance_sql).await;
    let total_balance = query_i64(&state.pool, &total_sql).await;
    let total_utxo_count = query_i64(&state.pool, &total_utxo_sql).await;
    let utxo_count = total_utxo_count; // header pill shows total
    let tx_count = query_i64(&state.pool, "SELECT COUNT(*) as v FROM transactions").await;
    let proven_tx_count = query_i64(&state.pool, "SELECT COUNT(*) as v FROM proven_txs").await;
    let certificate_count = query_i64(&state.pool, "SELECT COUNT(*) as v FROM certificates").await;

    let basket_rows = sqlx::query(&basket_sql)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();
    let baskets: Vec<BasketStats> = basket_rows
        .iter()
        .map(|r| BasketStats {
            name: r.get("name"),
            sats: r.get("sats"),
            utxos: r.get("utxos"),
        })
        .collect();

    let chain = query_string(&state.pool, "SELECT chain as v FROM settings LIMIT 1").await;
    let identity_key = query_string(
        &state.pool,
        "SELECT storage_identity_key as v FROM settings LIMIT 1",
    )
    .await;

    let db_size = std::fs::metadata(&state.db_path)
        .map(|m| format_bytes(m.len()))
        .unwrap_or_else(|_| "unknown".to_string());

    Json(WalletStats {
        balance,
        utxo_count,
        total_balance,
        total_utxo_count,
        baskets,
        tx_count,
        proven_tx_count,
        certificate_count,
        chain,
        identity_key,
        db_path: state.db_path.clone(),
        db_size,
    })
}

// ─── Custom SQL query ───

#[derive(Deserialize)]
struct QueryReq {
    sql: String,
}

#[derive(Serialize)]
struct QueryRes {
    columns: Vec<String>,
    rows: Vec<serde_json::Map<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn query(State(state): State<Arc<AppState>>, Json(req): Json<QueryReq>) -> Json<QueryRes> {
    let sql = req.sql.trim();

    // Safety: only allow SELECT and PRAGMA
    let upper = sql.to_uppercase();
    if !upper.starts_with("SELECT") && !upper.starts_with("PRAGMA") && !upper.starts_with("EXPLAIN")
    {
        return Json(QueryRes {
            columns: vec![],
            rows: vec![],
            error: Some("Only SELECT, PRAGMA, and EXPLAIN queries are allowed".to_string()),
        });
    }

    match sqlx::query(sql).fetch_all(&state.pool).await {
        Ok(rows) => {
            let columns: Vec<String> = rows
                .first()
                .map(|r| r.columns().iter().map(|c| c.name().to_string()).collect())
                .unwrap_or_default();

            let json_rows = rows
                .iter()
                .map(|row| {
                    let mut map = serde_json::Map::new();
                    for col in row.columns() {
                        map.insert(col.name().to_string(), sqlite_value_to_json(row, col));
                    }
                    map
                })
                .collect();

            Json(QueryRes {
                columns,
                rows: json_rows,
                error: None,
            })
        }
        Err(e) => Json(QueryRes {
            columns: vec![],
            rows: vec![],
            error: Some(e.to_string()),
        }),
    }
}

// ─── Helpers ───

fn sqlite_value_to_json(
    row: &sqlx::sqlite::SqliteRow,
    col: &sqlx::sqlite::SqliteColumn,
) -> serde_json::Value {
    let type_name = col.type_info().name();
    let idx = col.ordinal();

    // Try integer first
    if let Ok(v) = row.try_get::<i64, _>(idx) {
        return serde_json::Value::Number(v.into());
    }
    if let Ok(v) = row.try_get::<f64, _>(idx) {
        return serde_json::json!(v);
    }
    if let Ok(v) = row.try_get::<String, _>(idx) {
        return serde_json::Value::String(v);
    }
    if let Ok(v) = row.try_get::<Vec<u8>, _>(idx) {
        // Display blob as hex (truncated for readability)
        let hex = hex::encode(&v);
        if hex.len() > 64 {
            return serde_json::Value::String(format!("{}... ({} bytes)", &hex[..64], v.len()));
        }
        return serde_json::Value::String(hex);
    }
    if let Ok(v) = row.try_get::<bool, _>(idx) {
        return serde_json::Value::Bool(v);
    }

    // Check for NULL
    let _ = type_name;
    serde_json::Value::Null
}

async fn count_table(pool: &SqlitePool, table: &str) -> i64 {
    query_i64(pool, &format!("SELECT COUNT(*) as v FROM \"{}\"", table)).await
}

async fn count_filtered(pool: &SqlitePool, table: &str, filter: &str) -> i64 {
    let where_clause = build_filter_where(pool, table, filter).await;
    query_i64(
        pool,
        &format!("SELECT COUNT(*) as v FROM \"{table}\" {where_clause}"),
    )
    .await
}

async fn build_filter_where(pool: &SqlitePool, table: &str, filter: &str) -> String {
    let cols = get_column_names(pool, table).await;
    if cols.is_empty() {
        return String::new();
    }
    let conditions: Vec<String> = cols
        .iter()
        .map(|c| {
            format!(
                "CAST(\"{}\" AS TEXT) LIKE '%{}%'",
                c.replace('"', ""),
                filter.replace('\'', "''")
            )
        })
        .collect();
    format!("WHERE {}", conditions.join(" OR "))
}

async fn get_column_names(pool: &SqlitePool, table: &str) -> Vec<String> {
    sqlx::query(&format!("PRAGMA table_info(\"{}\")", table))
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| r.get("name"))
        .collect()
}

async fn query_i64(pool: &SqlitePool, sql: &str) -> i64 {
    sqlx::query(sql)
        .fetch_one(pool)
        .await
        .map(|r| r.get::<i64, _>("v"))
        .unwrap_or(0)
}

async fn query_string(pool: &SqlitePool, sql: &str) -> String {
    sqlx::query(sql)
        .fetch_one(pool)
        .await
        .map(|r| r.get::<String, _>("v"))
        .unwrap_or_default()
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(header::HeaderName, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(k.clone(), v.parse().unwrap());
        }
        h
    }

    /// #411: the inspector's own page and the listed origins are admitted;
    /// a GET without an Origin is its own page's read; anything else, or a
    /// Host that is not loopback, is refused.
    #[test]
    fn the_inspector_admits_its_own_origin_and_the_listed() {
        let listed = vec!["http://localhost:5173".to_string()];
        let host = (header::HOST, "localhost:9321");
        let ok = |m: Method, h: HeaderMap| inspector_word(&m, &h, &listed).is_ok();
        assert!(ok(Method::GET, headers(std::slice::from_ref(&host))));
        assert!(ok(
            Method::HEAD,
            headers(&[(header::HOST, "127.0.0.1:9321")])
        ));
        assert!(ok(
            Method::POST,
            headers(&[host.clone(), (header::ORIGIN, "http://localhost:9321")])
        ));
        assert!(ok(
            Method::POST,
            headers(&[host.clone(), (header::ORIGIN, "http://localhost:5173")])
        ));
        assert!(!ok(Method::POST, headers(std::slice::from_ref(&host))));
        assert!(!ok(
            Method::POST,
            headers(&[host.clone(), (header::ORIGIN, "http://evil.example")])
        ));
        assert!(!ok(
            Method::GET,
            headers(&[host.clone(), (header::ORIGIN, "http://127.0.0.1:9321")])
        ));
        assert!(!ok(
            Method::GET,
            headers(&[(header::HOST, "evil.example:9321")])
        ));
        assert!(!ok(Method::GET, headers(&[])));
    }
}
