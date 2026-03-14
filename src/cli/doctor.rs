use std::path::PathBuf;

use clap::Args;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DoctorError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

#[derive(Args)]
pub struct DoctorArgs {
    /// Auto-fix: re-run migrations, test connections
    #[arg(long)]
    pub fix: bool,

    /// Config file path
    #[arg(long, default_value = "postblox.toml")]
    pub config_path: PathBuf,
}

struct CheckResult {
    name: &'static str,
    passed: bool,
    detail: String,
}

impl CheckResult {
    fn ok(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            passed: true,
            detail: detail.into(),
        }
    }

    fn fail(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            passed: false,
            detail: detail.into(),
        }
    }
}

pub async fn run(args: DoctorArgs) -> Result<(), DoctorError> {
    println!("\n  postblox doctor\n");

    let mut results = Vec::new();

    // 1. Check config file
    let config = check_config(&args.config_path, &mut results);

    if let Some(config) = config {
        // 2. Test database
        let pool = check_database(&config.database_url, &mut results).await;

        // 3. Check migrations
        if let Some(pool) = &pool {
            check_migrations(pool, args.fix, &mut results).await;
        }

        // 4. Check Stalwart
        if let (Some(url), Some(_token)) = (&config.stalwart_url, &config.stalwart_admin_token) {
            check_stalwart(url, &mut results).await;
        }

        // 5. Check embedding provider
        if let Some(url) = &config.embedding_url {
            check_embedding(url, &mut results).await;
        }

        // 6. Check config file permissions
        #[cfg(unix)]
        check_permissions(&args.config_path, &mut results);

        if let Some(pool) = pool {
            pool.close().await;
        }
    }

    // Print results
    let mut failures = 0;
    for r in &results {
        let icon = if r.passed { "+" } else { "x" };
        println!("  [{icon}] {}: {}", r.name, r.detail);
        if !r.passed {
            failures += 1;
        }
    }

    println!();
    if failures == 0 {
        println!("  All checks passed.\n");
        Ok(())
    } else {
        println!("  {failures} check(s) failed.\n");
        Err(DoctorError::Other(format!("{failures} check(s) failed")))
    }
}

fn check_config(path: &PathBuf, results: &mut Vec<CheckResult>) -> Option<crate::config::Config> {
    if !path.exists() {
        results.push(CheckResult::fail(
            "Config file",
            format!(
                "{} not found. Run `postblox init` to create it.",
                path.display()
            ),
        ));
        return None;
    }

    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            results.push(CheckResult::fail("Config file", format!("read error: {e}")));
            return None;
        }
    };

    match toml::from_str::<crate::config::Config>(&contents) {
        Ok(config) => {
            results.push(CheckResult::ok(
                "Config file",
                format!("{} is valid TOML", path.display()),
            ));
            Some(config)
        }
        Err(e) => {
            results.push(CheckResult::fail(
                "Config file",
                format!("invalid TOML: {e}"),
            ));
            None
        }
    }
}

async fn check_database(url: &str, results: &mut Vec<CheckResult>) -> Option<sqlx::PgPool> {
    use sqlx::postgres::PgPoolOptions;

    let pool = match PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(url)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            results.push(CheckResult::fail(
                "Database",
                format!("connection failed: {e}. Check DATABASE_URL or database_url in config."),
            ));
            return None;
        }
    };

    match sqlx::query("SELECT 1").execute(&pool).await {
        Ok(_) => {
            results.push(CheckResult::ok("Database", "connected and responding"));
            Some(pool)
        }
        Err(e) => {
            results.push(CheckResult::fail("Database", format!("query failed: {e}")));
            None
        }
    }
}

async fn check_migrations(pool: &sqlx::PgPool, fix: bool, results: &mut Vec<CheckResult>) {
    let applied: Result<Vec<(i64,)>, _> =
        sqlx::query_as("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(pool)
            .await;

    let applied_count = match applied {
        Ok(rows) => rows.len(),
        Err(e) => {
            let is_missing = e.to_string().contains("does not exist");
            if !is_missing {
                results.push(CheckResult::fail(
                    "Migrations",
                    format!("query failed: {e}"),
                ));
                return;
            }
            if fix {
                match sqlx::migrate!("./migrations").run(pool).await {
                    Ok(_) => {
                        results.push(CheckResult::ok(
                            "Migrations",
                            "table missing, ran all migrations successfully",
                        ));
                        return;
                    }
                    Err(e) => {
                        results.push(CheckResult::fail(
                            "Migrations",
                            format!("failed to run: {e}"),
                        ));
                        return;
                    }
                }
            }
            results.push(CheckResult::fail(
                "Migrations",
                "migration table not found. Run `postblox doctor --fix` or `postblox init`.",
            ));
            return;
        }
    };

    let available = sqlx::migrate!("./migrations");
    let available_count = available.migrations.len();

    if applied_count >= available_count {
        results.push(CheckResult::ok(
            "Migrations",
            format!("{applied_count}/{available_count} applied"),
        ));
    } else if fix {
        match available.run(pool).await {
            Ok(_) => {
                results.push(CheckResult::ok(
                    "Migrations",
                    format!(
                        "was {applied_count}/{available_count}, ran pending migrations successfully"
                    ),
                ));
            }
            Err(e) => {
                results.push(CheckResult::fail(
                    "Migrations",
                    format!("failed to run pending: {e}"),
                ));
            }
        }
    } else {
        results.push(CheckResult::fail(
            "Migrations",
            format!(
                "{applied_count}/{available_count} applied. Run `postblox doctor --fix` to apply pending."
            ),
        ));
    }
}

async fn check_stalwart(url: &str, results: &mut Vec<CheckResult>) {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            results.push(CheckResult::fail(
                "Stalwart",
                format!("http client error: {e}"),
            ));
            return;
        }
    };

    match client.get(url).send().await {
        Ok(resp) => {
            results.push(CheckResult::ok(
                "Stalwart",
                format!("reachable (HTTP {})", resp.status().as_u16()),
            ));
        }
        Err(e) => {
            results.push(CheckResult::fail(
                "Stalwart",
                format!("unreachable: {e}. Check stalwart_url in config."),
            ));
        }
    }
}

async fn check_embedding(url: &str, results: &mut Vec<CheckResult>) {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            results.push(CheckResult::fail(
                "Embedding",
                format!("http client error: {e}"),
            ));
            return;
        }
    };

    let base = url.trim_end_matches("/v1/embeddings").trim_end_matches('/');

    match client.get(base).send().await {
        Ok(resp) => {
            results.push(CheckResult::ok(
                "Embedding",
                format!("reachable (HTTP {})", resp.status().as_u16()),
            ));
        }
        Err(e) => {
            results.push(CheckResult::fail(
                "Embedding",
                format!("unreachable: {e}. Check embedding_url in config."),
            ));
        }
    }
}

#[cfg(unix)]
fn check_permissions(path: &PathBuf, results: &mut Vec<CheckResult>) {
    use std::os::unix::fs::PermissionsExt;

    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            results.push(CheckResult::fail(
                "File permissions",
                format!("could not read metadata for {}: {e}", path.display()),
            ));
            return;
        }
    };

    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 == 0 {
        results.push(CheckResult::ok(
            "File permissions",
            format!("{} has mode {:o}", path.display(), mode),
        ));
    } else {
        results.push(CheckResult::fail(
            "File permissions",
            format!(
                "{} is world/group-readable (mode {:o}). Run: chmod 600 {}",
                path.display(),
                mode,
                path.display()
            ),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_result_ok() {
        let r = CheckResult::ok("Test", "all good");
        assert!(r.passed);
        assert_eq!(r.name, "Test");
        assert_eq!(r.detail, "all good");
    }

    #[test]
    fn test_check_result_fail() {
        let r = CheckResult::fail("Test", "broken");
        assert!(!r.passed);
        assert_eq!(r.name, "Test");
        assert_eq!(r.detail, "broken");
    }

    #[test]
    fn test_check_config_missing_file() {
        let mut results = Vec::new();
        let config = check_config(&PathBuf::from("/nonexistent/postblox.toml"), &mut results);
        assert!(config.is_none());
        assert_eq!(results.len(), 1);
        assert!(!results[0].passed);
        assert!(results[0].detail.contains("not found"));
    }

    #[test]
    fn test_check_config_valid_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("postblox.toml");
        std::fs::write(&path, "database_url = \"postgres://localhost/pb\"\n").unwrap();

        let mut results = Vec::new();
        let config = check_config(&path, &mut results);
        assert!(config.is_some());
        assert_eq!(results.len(), 1);
        assert!(results[0].passed);
    }

    #[test]
    fn test_check_config_invalid_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("postblox.toml");
        std::fs::write(&path, "this is not valid toml [[[").unwrap();

        let mut results = Vec::new();
        let config = check_config(&path, &mut results);
        assert!(config.is_none());
        assert_eq!(results.len(), 1);
        assert!(!results[0].passed);
        assert!(results[0].detail.contains("invalid TOML"));
    }

    #[test]
    fn test_check_config_missing_required_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("postblox.toml");
        std::fs::write(&path, "host = \"127.0.0.1\"\n").unwrap();

        let mut results = Vec::new();
        let config = check_config(&path, &mut results);
        assert!(config.is_none());
        assert_eq!(results.len(), 1);
        assert!(!results[0].passed);
    }

    #[cfg(unix)]
    #[test]
    fn test_check_permissions_secure() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("postblox.toml");
        std::fs::write(&path, "test").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        let mut results = Vec::new();
        check_permissions(&path, &mut results);
        assert_eq!(results.len(), 1);
        assert!(results[0].passed);
    }

    #[cfg(unix)]
    #[test]
    fn test_check_permissions_world_readable() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("postblox.toml");
        std::fs::write(&path, "test").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let mut results = Vec::new();
        check_permissions(&path, &mut results);
        assert_eq!(results.len(), 1);
        assert!(!results[0].passed);
        assert!(results[0].detail.contains("world/group-readable"));
        assert!(results[0].detail.contains("chmod 600"));
    }

    #[test]
    fn test_doctor_error_display() {
        let err = DoctorError::Other("something went wrong".into());
        assert_eq!(err.to_string(), "something went wrong");
    }
}
