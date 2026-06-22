use diesel::connection::SimpleConnection;
use diesel::r2d2::{ConnectionManager, CustomizeConnection};
use diesel::sqlite::SqliteConnection;
use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};

use crate::error::{AppError, AppResult};

pub type DbPool = r2d2::Pool<ConnectionManager<SqliteConnection>>;
pub type DbConn = r2d2::PooledConnection<ConnectionManager<SqliteConnection>>;

pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

#[derive(Debug)]
struct ConnectionPragmas;

impl CustomizeConnection<SqliteConnection, diesel::r2d2::Error> for ConnectionPragmas {
    fn on_acquire(&self, conn: &mut SqliteConnection) -> Result<(), diesel::r2d2::Error> {
        conn.batch_execute("PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON;")
            .map_err(diesel::r2d2::Error::QueryError)
    }
}

pub fn init_pool(database_url: &str, pool_size: u32) -> AppResult<DbPool> {
    let manager = ConnectionManager::<SqliteConnection>::new(database_url);
    let pool = r2d2::Pool::builder()
        .max_size(pool_size)
        .connection_customizer(Box::new(ConnectionPragmas))
        .build(manager)?;
    Ok(pool)
}

pub fn run_migrations(pool: &DbPool) -> AppResult<()> {
    let mut conn = pool.get()?;
    conn.run_pending_migrations(MIGRATIONS)
        .map_err(|e| AppError::Internal(format!("migration failed: {e}")))?;
    tracing::info!("database migrations applied successfully");
    Ok(())
}
