use std::env;

use anyhow::{Context, Result};

#[derive(Clone, Debug)]
pub struct Config {
    pub database_encrypt_secret: [u8; 32],
    pub software_secret: [u8; 32],
    pub database_url: String,
    pub bind_addr: String,
    pub db_pool_size: u32,
    pub cors_origin: Option<String>,
    pub admin_allowed_subnet: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let database_encrypt_secret =
            decode_hex_env("DATABASEENCRYPTSECRET").context("DATABASEENCRYPTSECRET")?;
        let software_secret = decode_hex_env("SOFTWARESECRET").context("SOFTWARESECRET")?;

        Ok(Config {
            database_encrypt_secret,
            software_secret,
            database_url: env::var("DATABASE_URL").context("DATABASE_URL must be set")?,
            bind_addr: env::var("BIND_ADDR").context("BIND_ADDR must be set")?,
            db_pool_size: env::var("DB_POOL_SIZE")
                .context("DB_POOL_SIZE must be set")?
                .parse()
                .context("DB_POOL_SIZE must be a valid integer")?,
            cors_origin: env::var("CORS_ORIGIN").ok(),
            admin_allowed_subnet: env::var("ADMIN_ALLOWED_SUBNET")
                .context("ADMIN_ALLOWED_SUBNET must be set")?,
        })
    }
}

fn decode_hex_env(key: &str) -> Result<[u8; 32]> {
    let val = env::var(key).context(format!("{key} must be set"))?;
    let bytes = hex::decode(&val).context(format!("{key} must be valid hex"))?;
    if bytes.len() != 32 {
        anyhow::bail!("{key} must be exactly 32 bytes (64 hex chars)");
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(arr)
}
