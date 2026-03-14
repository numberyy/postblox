use std::path::PathBuf;

use clap::Args;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum InitError {
    #[error("database connection failed: {0}")]
    Database(String),
    #[error("stalwart unreachable: {0}")]
    Stalwart(String),
    #[error("embedding provider unreachable: {0}")]
    Embedding(String),
    #[error("config file already exists: {0}")]
    ConfigExists(PathBuf),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("prompt error: {0}")]
    Prompt(#[from] dialoguer::Error),
    #[error("{0}")]
    Other(String),
}

#[derive(Args)]
pub struct InitArgs {
    /// Run without interactive prompts (CI/Docker mode)
    #[arg(long)]
    pub non_interactive: bool,

    /// PostgreSQL connection URL
    #[arg(long, env = "DATABASE_URL")]
    pub database_url: Option<String>,

    /// Server bind host
    #[arg(long, env = "POSTBLOX_HOST")]
    pub host: Option<String>,

    /// Server bind port
    #[arg(long, env = "POSTBLOX_PORT")]
    pub port: Option<u16>,

    /// Stalwart mail server URL
    #[arg(long, env = "STALWART_URL")]
    pub stalwart_url: Option<String>,

    /// Stalwart admin username
    #[arg(long, env = "STALWART_ADMIN_USER")]
    pub stalwart_admin_user: Option<String>,

    /// Stalwart admin password/token
    #[arg(long, env = "STALWART_ADMIN_TOKEN")]
    pub stalwart_admin_token: Option<String>,

    /// Stalwart inbound webhook token
    #[arg(long, env = "STALWART_INBOUND_TOKEN")]
    pub stalwart_inbound_token: Option<String>,

    /// Embedding provider URL
    #[arg(long, env = "EMBEDDING_URL")]
    pub embedding_url: Option<String>,

    /// Embedding model name
    #[arg(long, env = "EMBEDDING_MODEL")]
    pub embedding_model: Option<String>,

    /// Embedding API key
    #[arg(long, env = "EMBEDDING_API_KEY")]
    pub embedding_api_key: Option<String>,

    /// Organization name for initial setup
    #[arg(long)]
    pub org_name: Option<String>,

    /// Config file output path
    #[arg(long, default_value = "postblox.toml")]
    pub config_path: PathBuf,

    /// Overwrite existing config file
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug)]
struct CollectedConfig {
    database_url: String,
    host: String,
    port: u16,
    stalwart_url: Option<String>,
    stalwart_admin_user: Option<String>,
    stalwart_admin_token: Option<String>,
    stalwart_inbound_token: Option<String>,
    embedding_url: Option<String>,
    embedding_model: Option<String>,
    embedding_api_key: Option<String>,
    org_name: String,
}

pub async fn run(args: InitArgs) -> Result<(), InitError> {
    println!("\n  postblox init\n");

    if args.config_path.exists() && !args.force {
        return Err(InitError::ConfigExists(args.config_path));
    }

    let config = if args.non_interactive {
        collect_non_interactive(&args)?
    } else {
        collect_interactive(&args)?
    };

    // Test database connection
    print!("  Testing database connection... ");
    let pool = test_db_connection(&config.database_url).await?;
    println!("ok");

    // Run migrations
    print!("  Running migrations... ");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .map_err(|e| InitError::Database(e.to_string()))?;
    println!("ok");

    // Test Stalwart if configured
    if let Some(url) = &config.stalwart_url {
        print!("  Testing Stalwart connection... ");
        test_stalwart(url).await?;
        println!("ok");
    }

    // Test embedding provider if configured
    if let Some(url) = &config.embedding_url {
        print!("  Testing embedding provider... ");
        test_embedding(url).await?;
        println!("ok");
    }

    // Generate config file
    let toml_content = generate_toml(&config);
    write_config(&args.config_path, &toml_content)?;
    println!("  Config written to {}", args.config_path.display());

    // Create first organization + API key
    print!("  Creating organization '{}'... ", config.org_name);
    let api_key = create_org_and_key(&pool, &config.org_name).await?;
    println!("ok\n");

    println!("  Your API key (save this — it won't be shown again):");
    println!("  {api_key}\n");

    pool.close().await;
    println!("  Setup complete. Start with: postblox\n");

    Ok(())
}

fn collect_non_interactive(args: &InitArgs) -> Result<CollectedConfig, InitError> {
    let database_url = args.database_url.clone().ok_or_else(|| {
        InitError::Other("--database-url is required in non-interactive mode".into())
    })?;

    let org_name = args.org_name.clone().unwrap_or_else(|| "default".into());

    Ok(CollectedConfig {
        database_url,
        host: args.host.clone().unwrap_or_else(|| "0.0.0.0".into()),
        port: args.port.unwrap_or(3000),
        stalwart_url: args.stalwart_url.clone(),
        stalwart_admin_user: args.stalwart_admin_user.clone(),
        stalwart_admin_token: args.stalwart_admin_token.clone(),
        stalwart_inbound_token: args.stalwart_inbound_token.clone(),
        embedding_url: args.embedding_url.clone(),
        embedding_model: args.embedding_model.clone(),
        embedding_api_key: args.embedding_api_key.clone(),
        org_name,
    })
}

fn collect_interactive(args: &InitArgs) -> Result<CollectedConfig, InitError> {
    use dialoguer::{Confirm, Input, Password, Select};

    let mode = Select::new()
        .with_prompt("Setup mode")
        .items(&[
            "QuickStart (Docker Compose, minimal config)",
            "Advanced (bare metal, full config)",
        ])
        .default(0)
        .interact()?;

    let is_quickstart = mode == 0;

    let default_db = if is_quickstart {
        "postgres://postblox:postblox@localhost:5432/postblox"
    } else {
        "postgres://user:pass@localhost:5432/postblox"
    };

    let database_url: String = if let Some(url) = &args.database_url {
        url.clone()
    } else {
        Input::new()
            .with_prompt("Database URL")
            .default(default_db.into())
            .interact_text()?
    };

    let (host, port) = if is_quickstart {
        ("0.0.0.0".into(), 3000)
    } else {
        let h: String = Input::new()
            .with_prompt("Bind host")
            .default("0.0.0.0".into())
            .interact_text()?;
        let p: u16 = Input::new()
            .with_prompt("Bind port")
            .default(3000)
            .interact()?;
        (h, p)
    };

    let (stalwart_url, stalwart_admin_user, stalwart_admin_token, stalwart_inbound_token) =
        if Confirm::new()
            .with_prompt("Configure email delivery (Stalwart)?")
            .default(is_quickstart)
            .interact()?
        {
            let default_stalwart = if is_quickstart {
                "http://stalwart:8080"
            } else {
                "http://localhost:8080"
            };
            let url: String = Input::new()
                .with_prompt("Stalwart URL")
                .default(default_stalwart.into())
                .interact_text()?;
            let user: String = Input::new()
                .with_prompt("Stalwart admin user")
                .default("admin".into())
                .interact_text()?;
            let token: String = Password::new()
                .with_prompt("Stalwart admin password")
                .interact()?;
            let inbound: String = Input::new()
                .with_prompt("Inbound webhook token (for Stalwart -> postblox)")
                .default(uuid::Uuid::new_v4().simple().to_string())
                .interact_text()?;
            (Some(url), Some(user), Some(token), Some(inbound))
        } else {
            (None, None, None, None)
        };

    let (embedding_url, embedding_model, embedding_api_key) = if Confirm::new()
        .with_prompt("Configure semantic search (embeddings)?")
        .default(false)
        .interact()?
    {
        let provider = Select::new()
            .with_prompt("Embedding provider")
            .items(&["OpenAI-compatible API", "Ollama (local)"])
            .default(0)
            .interact()?;

        let (default_url, default_model) = if provider == 1 {
            ("http://localhost:11434/v1/embeddings", "nomic-embed-text")
        } else {
            (
                "https://api.openai.com/v1/embeddings",
                "text-embedding-3-small",
            )
        };

        let url: String = Input::new()
            .with_prompt("Embedding URL")
            .default(default_url.into())
            .interact_text()?;
        let model: String = Input::new()
            .with_prompt("Model name")
            .default(default_model.into())
            .interact_text()?;
        let api_key = if provider == 0 {
            let k: String = Password::new().with_prompt("API key").interact()?;
            Some(k).filter(|s| !s.is_empty())
        } else {
            None
        };
        (Some(url), Some(model), api_key)
    } else {
        (None, None, None)
    };

    let org_name: String = Input::new()
        .with_prompt("Organization name")
        .default("default".into())
        .interact_text()?;

    Ok(CollectedConfig {
        database_url,
        host,
        port,
        stalwart_url,
        stalwart_admin_user,
        stalwart_admin_token,
        stalwart_inbound_token,
        embedding_url,
        embedding_model,
        embedding_api_key,
        org_name,
    })
}

async fn test_db_connection(url: &str) -> Result<sqlx::PgPool, InitError> {
    use sqlx::postgres::PgPoolOptions;

    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(url)
        .await
        .map_err(|e| InitError::Database(e.to_string()))?;

    sqlx::query("SELECT 1")
        .execute(&pool)
        .await
        .map_err(|e| InitError::Database(e.to_string()))?;

    Ok(pool)
}

async fn test_url(url: &str, map_err: fn(String) -> InitError) -> Result<(), InitError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| map_err(e.to_string()))?;

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| map_err(e.to_string()))?;

    if resp.status().is_server_error() {
        return Err(map_err(format!("server returned HTTP {}", resp.status())));
    }

    Ok(())
}

async fn test_stalwart(url: &str) -> Result<(), InitError> {
    test_url(url, InitError::Stalwart).await
}

async fn test_embedding(url: &str) -> Result<(), InitError> {
    let base = url.trim_end_matches("/v1/embeddings").trim_end_matches('/');
    test_url(base, InitError::Embedding).await
}

fn generate_toml(config: &CollectedConfig) -> String {
    let mut out = String::new();
    out.push_str(&format!("database_url = {:?}\n", config.database_url));

    if config.host != "0.0.0.0" {
        out.push_str(&format!("host = {:?}\n", config.host));
    }
    if config.port != 3000 {
        out.push_str(&format!("port = {}\n", config.port));
    }

    if config.stalwart_url.is_some() {
        out.push('\n');
        if let Some(v) = &config.stalwart_url {
            out.push_str(&format!("stalwart_url = {:?}\n", v));
        }
        if let Some(v) = &config.stalwart_admin_user {
            out.push_str(&format!("stalwart_admin_user = {:?}\n", v));
        }
        if let Some(v) = &config.stalwart_admin_token {
            out.push_str(&format!("stalwart_admin_token = {:?}\n", v));
        }
        if let Some(v) = &config.stalwart_inbound_token {
            out.push_str(&format!("stalwart_inbound_token = {:?}\n", v));
        }
    }

    if config.embedding_url.is_some() {
        out.push('\n');
        if let Some(v) = &config.embedding_url {
            out.push_str(&format!("embedding_url = {:?}\n", v));
        }
        if let Some(v) = &config.embedding_model {
            out.push_str(&format!("embedding_model = {:?}\n", v));
        }
        if let Some(v) = &config.embedding_api_key {
            out.push_str(&format!("embedding_api_key = {:?}\n", v));
        }
    }

    out
}

fn write_config(path: &PathBuf, content: &str) -> Result<(), InitError> {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    // Check existing file permissions before overwriting
    if path.exists() {
        #[cfg(unix)]
        {
            let meta = fs::metadata(path)?;
            let mode = meta.permissions().mode();
            if mode & 0o077 != 0 {
                eprintln!(
                    "  warning: existing {} is world-readable (mode {:o})",
                    path.display(),
                    mode & 0o777
                );
            }
        }
    }

    fs::write(path, content)?;

    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;

    Ok(())
}

async fn create_org_and_key(pool: &sqlx::PgPool, org_name: &str) -> Result<String, InitError> {
    let gk = super::generate_api_key();

    let org = crate::db::organizations::create(pool, org_name)
        .await
        .map_err(|e| InitError::Database(e.to_string()))?;

    let key = crate::db::api_keys::create(pool, org.id, &gk.key_hash, &gk.prefix, Some("default"))
        .await
        .map_err(|e| InitError::Database(e.to_string()))?;

    crate::db::members::ensure_admin_exists(pool, org.id, key.id)
        .await
        .map_err(|e| InitError::Database(e.to_string()))?;

    Ok(gk.full_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_toml_minimal() {
        let config = CollectedConfig {
            database_url: "postgres://localhost/postblox".into(),
            host: "0.0.0.0".into(),
            port: 3000,
            stalwart_url: None,
            stalwart_admin_user: None,
            stalwart_admin_token: None,
            stalwart_inbound_token: None,
            embedding_url: None,
            embedding_model: None,
            embedding_api_key: None,
            org_name: "default".into(),
        };
        let toml = generate_toml(&config);
        assert!(toml.contains("database_url"));
        assert!(!toml.contains("\nhost"));
        assert!(!toml.contains("\nport"));
        assert!(!toml.contains("stalwart"));
        assert!(!toml.contains("embedding"));
    }

    #[test]
    fn test_generate_toml_full() {
        let config = CollectedConfig {
            database_url: "postgres://user:pass@db:5432/test".into(),
            host: "127.0.0.1".into(),
            port: 8080,
            stalwart_url: Some("http://stalwart:8080".into()),
            stalwart_admin_user: Some("admin".into()),
            stalwart_admin_token: Some("secret".into()),
            stalwart_inbound_token: Some("tok".into()),
            embedding_url: Some("http://localhost:11434/v1/embeddings".into()),
            embedding_model: Some("nomic-embed-text".into()),
            embedding_api_key: Some("sk-test".into()),
            org_name: "acme".into(),
        };
        let toml = generate_toml(&config);
        assert!(toml.contains("database_url = \"postgres://user:pass@db:5432/test\""));
        assert!(toml.contains("host = \"127.0.0.1\""));
        assert!(toml.contains("port = 8080"));
        assert!(toml.contains("stalwart_url"));
        assert!(toml.contains("stalwart_admin_user"));
        assert!(toml.contains("stalwart_admin_token"));
        assert!(toml.contains("stalwart_inbound_token"));
        assert!(toml.contains("embedding_url"));
        assert!(toml.contains("embedding_model"));
        assert!(toml.contains("embedding_api_key"));
    }

    #[test]
    fn test_generate_toml_default_host_port_omitted() {
        let config = CollectedConfig {
            database_url: "postgres://localhost/pb".into(),
            host: "0.0.0.0".into(),
            port: 3000,
            stalwart_url: None,
            stalwart_admin_user: None,
            stalwart_admin_token: None,
            stalwart_inbound_token: None,
            embedding_url: None,
            embedding_model: None,
            embedding_api_key: None,
            org_name: "test".into(),
        };
        let toml = generate_toml(&config);
        assert_eq!(toml.lines().count(), 1);
    }

    #[test]
    fn test_generate_toml_stalwart_without_embedding() {
        let config = CollectedConfig {
            database_url: "postgres://localhost/pb".into(),
            host: "0.0.0.0".into(),
            port: 3000,
            stalwart_url: Some("http://localhost:8080".into()),
            stalwart_admin_user: Some("admin".into()),
            stalwart_admin_token: Some("pass".into()),
            stalwart_inbound_token: None,
            embedding_url: None,
            embedding_model: None,
            embedding_api_key: None,
            org_name: "test".into(),
        };
        let toml = generate_toml(&config);
        assert!(toml.contains("stalwart_url"));
        assert!(!toml.contains("embedding"));
    }

    #[test]
    fn test_generate_toml_embedding_without_api_key() {
        let config = CollectedConfig {
            database_url: "postgres://localhost/pb".into(),
            host: "0.0.0.0".into(),
            port: 3000,
            stalwart_url: None,
            stalwart_admin_user: None,
            stalwart_admin_token: None,
            stalwart_inbound_token: None,
            embedding_url: Some("http://localhost:11434/v1/embeddings".into()),
            embedding_model: Some("nomic".into()),
            embedding_api_key: None,
            org_name: "test".into(),
        };
        let toml = generate_toml(&config);
        assert!(toml.contains("embedding_url"));
        assert!(toml.contains("embedding_model"));
        assert!(!toml.contains("embedding_api_key"));
    }

    #[test]
    fn test_generate_toml_is_valid_toml() {
        let config = CollectedConfig {
            database_url: "postgres://user:p@ss\"word@db/test".into(),
            host: "0.0.0.0".into(),
            port: 3000,
            stalwart_url: Some("http://stalwart:8080".into()),
            stalwart_admin_user: Some("admin".into()),
            stalwart_admin_token: Some("sec\"ret".into()),
            stalwart_inbound_token: None,
            embedding_url: None,
            embedding_model: None,
            embedding_api_key: None,
            org_name: "test".into(),
        };
        let toml_str = generate_toml(&config);
        let parsed: Result<crate::config::Config, _> = toml::from_str(&toml_str);
        assert!(parsed.is_ok(), "generated TOML should parse: {toml_str}");
    }

    #[test]
    fn test_write_config_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("postblox.toml");
        write_config(&path, "database_url = \"postgres://localhost/test\"\n").unwrap();
        assert!(path.exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(&path).unwrap();
            assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn test_write_config_warns_world_readable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("postblox.toml");
        std::fs::write(&path, "old content").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }

        // Should succeed (overwrites with 0600)
        write_config(&path, "database_url = \"postgres://localhost/test\"\n").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(&path).unwrap();
            assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn test_collect_non_interactive_requires_database_url() {
        let args = InitArgs {
            non_interactive: true,
            database_url: None,
            host: None,
            port: None,
            stalwart_url: None,
            stalwart_admin_user: None,
            stalwart_admin_token: None,
            stalwart_inbound_token: None,
            embedding_url: None,
            embedding_model: None,
            embedding_api_key: None,
            org_name: None,
            config_path: "postblox.toml".into(),
            force: false,
        };
        let result = collect_non_interactive(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("database-url"));
    }

    #[test]
    fn test_collect_non_interactive_defaults() {
        let args = InitArgs {
            non_interactive: true,
            database_url: Some("postgres://localhost/pb".into()),
            host: None,
            port: None,
            stalwart_url: None,
            stalwart_admin_user: None,
            stalwart_admin_token: None,
            stalwart_inbound_token: None,
            embedding_url: None,
            embedding_model: None,
            embedding_api_key: None,
            org_name: None,
            config_path: "postblox.toml".into(),
            force: false,
        };
        let config = collect_non_interactive(&args).unwrap();
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 3000);
        assert_eq!(config.org_name, "default");
        assert!(config.stalwart_url.is_none());
        assert!(config.embedding_url.is_none());
    }

    #[test]
    fn test_collect_non_interactive_with_all_flags() {
        let args = InitArgs {
            non_interactive: true,
            database_url: Some("postgres://db/pb".into()),
            host: Some("127.0.0.1".into()),
            port: Some(8080),
            stalwart_url: Some("http://stalwart:8080".into()),
            stalwart_admin_user: Some("admin".into()),
            stalwart_admin_token: Some("token".into()),
            stalwart_inbound_token: Some("inbound".into()),
            embedding_url: Some("http://ollama:11434/v1/embeddings".into()),
            embedding_model: Some("nomic".into()),
            embedding_api_key: Some("sk-key".into()),
            org_name: Some("acme".into()),
            config_path: "custom.toml".into(),
            force: true,
        };
        let config = collect_non_interactive(&args).unwrap();
        assert_eq!(config.database_url, "postgres://db/pb");
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 8080);
        assert_eq!(config.stalwart_url.as_deref(), Some("http://stalwart:8080"));
        assert_eq!(
            config.embedding_url.as_deref(),
            Some("http://ollama:11434/v1/embeddings")
        );
        assert_eq!(config.org_name, "acme");
    }

    #[test]
    fn test_init_error_display() {
        let err = InitError::Database("connection refused".into());
        assert!(err.to_string().contains("connection refused"));

        let err = InitError::ConfigExists("postblox.toml".into());
        assert!(err.to_string().contains("postblox.toml"));

        let err = InitError::Stalwart("timeout".into());
        assert!(err.to_string().contains("timeout"));

        let err = InitError::Embedding("unreachable".into());
        assert!(err.to_string().contains("unreachable"));
    }
}
