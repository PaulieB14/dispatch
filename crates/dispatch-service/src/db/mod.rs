pub mod receipts;

use anyhow::Result;
use sqlx::{postgres::PgPoolOptions, PgPool};

pub type Pool = PgPool;

pub async fn connect(url: &str) -> Result<Pool> {
    let pool = PgPoolOptions::new().max_connections(5).connect(url).await?;
    Ok(pool)
}

pub async fn run_migrations(pool: &Pool) -> Result<()> {
    sqlx::migrate!().run(pool).await?;
    tracing::info!("database migrations applied");
    Ok(())
}
