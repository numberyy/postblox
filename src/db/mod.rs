use sqlx::{postgres::PgPoolOptions, PgPool};

pub mod api_keys;
pub mod approvals;
pub mod attachments;
pub mod audit;
pub mod bounces;
pub mod briefing;
pub mod domains;
pub mod drafts;
pub mod embeddings;
pub mod inboxes;
pub mod labels;
pub mod linked_accounts;
pub mod members;
pub mod messages;
pub mod notifications;
pub mod organizations;
pub mod permissions;
pub mod slop;
pub mod slop_feedback;
pub mod threads;
pub mod trust;
pub mod webhooks;

pub async fn connect(database_url: &str) -> anyhow::Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(std::time::Duration::from_secs(3))
        .connect(database_url)
        .await?;

    tracing::info!("connected to postgres");
    Ok(pool)
}

#[cfg(test)]
pub(crate) async fn test_pool() -> PgPool {
    let url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for integration tests");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("failed to connect to test database");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("failed to run migrations");
    pool
}
