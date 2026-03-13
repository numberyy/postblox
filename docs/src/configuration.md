# Configuration

postblox reads configuration from a TOML file or environment variables. The config file path defaults to `postblox.toml` in the working directory, overridden by `POSTBLOX_CONFIG`.

If no config file exists, all settings fall back to environment variables.

## Full example

```toml
database_url = "postgres://postblox:postblox@localhost:5432/postblox"
host = "0.0.0.0"
port = 3000

# Stalwart Mail Server
stalwart_url = "http://localhost:8080"
stalwart_admin_user = "admin"
stalwart_admin_token = "changeme"
stalwart_inbound_token = "webhook-secret"
stalwart_smtp_host = "localhost"
stalwart_smtp_port = 25

# Outbound guard patterns (block messages matching these regexes)
[[guard_patterns]]
name = "ssn"
pattern = "\\b\\d{3}-\\d{2}-\\d{4}\\b"

[[guard_patterns]]
name = "credit_card"
pattern = "\\b\\d{4}[- ]?\\d{4}[- ]?\\d{4}[- ]?\\d{4}\\b"

# Semantic search (any OpenAI-compatible embedding API)
embedding_url = "http://localhost:11434/v1/embeddings"
embedding_model = "nomic-embed-text"
embedding_api_key = ""

# Progressive trust
trust_auto_upgrade_threshold = 10

# Rate limiting
[rate_limit]
requests_per_minute = 60
requests_per_hour = 1000

# Exec hooks
[[hooks]]
event = "message.received"
command = "/usr/local/bin/notify"
args = ["--channel", "email"]
timeout_secs = 5

[[hooks]]
event = "before_send"
command = "/usr/local/bin/check"
timeout_secs = 10
```

## Reference

### Core

| Field | Env var | Default | Description |
|-------|---------|---------|-------------|
| `database_url` | `DATABASE_URL` | *(required)* | PostgreSQL connection string |
| `host` | `POSTBLOX_HOST` | `0.0.0.0` | Listen address |
| `port` | `POSTBLOX_PORT` | `3000` | Listen port |

### Stalwart

| Field | Env var | Default | Description |
|-------|---------|---------|-------------|
| `stalwart_url` | `STALWART_URL` | *(none)* | Stalwart HTTP API URL. If unset, email delivery is disabled. |
| `stalwart_admin_user` | `STALWART_ADMIN_USER` | *(none)* | Admin username for Stalwart account management |
| `stalwart_admin_token` | `STALWART_ADMIN_TOKEN` | *(none)* | Admin auth token |
| `stalwart_inbound_token` | `STALWART_INBOUND_TOKEN` | *(none)* | Shared secret for inbound webhook from Stalwart |
| `stalwart_smtp_host` | `STALWART_SMTP_HOST` | *(none)* | SMTP host for outbound delivery |
| `stalwart_smtp_port` | `STALWART_SMTP_PORT` | *(none)* | SMTP port |

### Embeddings

| Field | Env var | Default | Description |
|-------|---------|---------|-------------|
| `embedding_url` | `EMBEDDING_URL` | *(none)* | OpenAI-compatible embedding API URL. If unset, semantic search is disabled. |
| `embedding_model` | `EMBEDDING_MODEL` | *(none)* | Model name to pass in the API request |
| `embedding_api_key` | `EMBEDDING_API_KEY` | *(none)* | API key for the embedding service (empty string for local models) |

### Trust

| Field | Env var | Default | Description |
|-------|---------|---------|-------------|
| `trust_auto_upgrade_threshold` | `TRUST_AUTO_UPGRADE_THRESHOLD` | `10` | Number of approved messages before an inbox auto-upgrades from `approval` to `auto_approve` mode |

### Rate limiting

| Field | Env var | Default | Description |
|-------|---------|---------|-------------|
| `rate_limit.requests_per_minute` | `RATE_LIMIT_PER_MINUTE` | `60` | Max API requests per minute per organization |
| `rate_limit.requests_per_hour` | `RATE_LIMIT_PER_HOUR` | `1000` | Max API requests per hour per organization |

### Guard patterns

Guard patterns are regexes checked against outbound message subject, text body, and HTML body. If any pattern matches, the send is blocked.

```toml
[[guard_patterns]]
name = "ssn"
pattern = "\\b\\d{3}-\\d{2}-\\d{4}\\b"
```

| Field | Description |
|-------|-------------|
| `name` | Human-readable name for the pattern (shown in error messages) |
| `pattern` | Regex pattern to match against |

Guard patterns can only be configured in the TOML file, not via environment variables.

### Hooks

Hooks run external commands in response to events. They receive a JSON payload on stdin and return JSON on stdout.

```toml
[[hooks]]
event = "message.received"
command = "/usr/local/bin/notify"
args = ["--channel", "email"]
timeout_secs = 5
```

| Field | Default | Description |
|-------|---------|-------------|
| `event` | *(required)* | Event name: `message.received`, `message.sent`, `approval.requested`, `before_send` |
| `command` | *(required)* | Path to executable |
| `args` | `[]` | Command arguments |
| `timeout_secs` | `10` | Seconds before the hook is killed |

**Event hooks** (`message.received`, `message.sent`, `approval.requested`) run asynchronously — failures are logged but don't block the request.

**`before_send` hooks** run synchronously before outbound delivery. They are fail-closed: if the hook returns `{"action": "block", "reason": "..."}`, times out, exits non-zero, or returns invalid JSON, the send is blocked.

Hooks can only be configured in the TOML file, not via environment variables.
