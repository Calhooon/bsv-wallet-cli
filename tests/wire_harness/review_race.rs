//! A named coin is taken after the host read, while headers are answered.
//! The toolbox must detect the competing txid under its write transaction.
use super::worlds::{puzzle_parent, PUZZLE};
use crate::wire_support::serve_local;
use axum::{extract::OriginalUri, Json, Router};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

pub(crate) async fn headers(base: String, pool: sqlx::SqlitePool) -> String {
    let taken = Arc::new(AtomicBool::new(false));
    let app = Router::new().fallback(move |OriginalUri(uri): OriginalUri| {
        let (base, pool, taken) = (base.clone(), pool.clone(), taken.clone());
        async move {
            if uri.path() == "/findHeaderHexForHeight"
                && uri.query() == Some(format!("height={}", PUZZLE.1).as_str())
                && !taken.swap(true, Ordering::SeqCst)
            {
                let now = chrono::Utc::now();
                let user: i64 = sqlx::query_scalar("SELECT user_id FROM users LIMIT 1")
                    .fetch_one(&pool).await.unwrap();
                let spender = sqlx::query("INSERT INTO transactions (user_id, status, reference, is_outgoing, satoshis, version, lock_time, description, txid, created_at, updated_at) VALUES (?, 'failed', 'review-competitor', 1, 0, 1, 0, 'concurrent spender', ?, ?, ?)")
                    .bind(user).bind("77".repeat(32)).bind(now).bind(now)
                    .execute(&pool).await.unwrap().last_insert_rowid();
                sqlx::query("UPDATE outputs SET spendable = 0, spent_by = ? WHERE txid = ?")
                    .bind(spender).bind(puzzle_parent().0.id()).execute(&pool).await.unwrap();
            }
            let value: serde_json::Value = reqwest::Client::builder().no_proxy().build().unwrap()
                .get(format!("{base}{uri}")).send().await.unwrap().json().await.unwrap();
            Json(value)
        }
    });
    serve_local(app).await
}
