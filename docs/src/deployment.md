# Deployment

## Docker (recommended)

The project includes a multi-stage Dockerfile that produces a `scratch`-based image with no shell, no package manager — just the binary.

```bash
docker compose up -d
```

The `docker-compose.yml` starts PostgreSQL (pgvector), Stalwart Mail Server, and postblox.

### Services

| Service | Image | Ports |
|---------|-------|-------|
| postgres | pgvector/pgvector:pg17 | 5432 |
| stalwart | stalwartlabs/stalwart:latest | 8080, 25, 587, 465, 993 |
| postblox | built from Dockerfile | 3000 |
| ollama | ollama/ollama:latest | 11434 (optional, `--profile gpu`) |

### Volumes

- `postgres_data` — PostgreSQL data
- `stalwart_data` — Stalwart mail server data
- `postblox_data` — attachment storage (mounted at `/data`)

### Environment variables

Set these on the postblox container (or in `postblox.toml`):

```yaml
environment:
  DATABASE_URL: postgres://postblox:postblox@postgres:5432/postblox
  STALWART_URL: http://stalwart:8080
  STALWART_ADMIN_USER: admin
  STALWART_ADMIN_TOKEN: changeme
  STALWART_INBOUND_TOKEN: ${STALWART_INBOUND_TOKEN:-your-secret-token}
  POSTBLOX_HOST: 0.0.0.0
  POSTBLOX_PORT: "3000"
  ATTACHMENT_STORAGE_PATH: /data/attachments
```

### Building the image

```bash
docker build -t postblox .
```

The Dockerfile uses `rust:1.85-alpine` for the build stage and `scratch` for the final image. The release binary is compiled with `x86_64-unknown-linux-musl` for static linking.

## Bare metal

### Prerequisites

- Rust 1.80+ (see `rust-toolchain.toml`)
- PostgreSQL 15+ with pgvector extension
- Stalwart Mail Server (for email delivery)

### Build

```bash
cargo build --release
```

Binaries are in `target/release/`:
- `postblox` — main server
- `postblox-mcp` — MCP server
- `postblox-tui` — terminal UI

The release profile uses `opt-level = "z"`, LTO, single codegen unit, strip, and `panic = "abort"` for minimal binary size.

### Run

```bash
# With config file
cp postblox.toml.example postblox.toml
# Edit postblox.toml with your settings
./target/release/postblox

# Or with environment variables
DATABASE_URL=postgres://localhost/postblox \
POSTBLOX_HOST=0.0.0.0 \
POSTBLOX_PORT=3000 \
./target/release/postblox
```

### Database setup

postblox runs migrations automatically on startup via sqlx. Ensure the database exists:

```bash
createdb postblox
# Enable pgvector (for semantic search)
psql postblox -c "CREATE EXTENSION IF NOT EXISTS vector;"
```

### systemd unit file

```ini
[Unit]
Description=postblox email server
After=network.target postgresql.service

[Service]
Type=simple
User=postblox
Group=postblox
WorkingDirectory=/opt/postblox
ExecStart=/opt/postblox/postblox
Environment=POSTBLOX_CONFIG=/opt/postblox/postblox.toml
Restart=on-failure
RestartSec=5
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
```

```bash
sudo cp postblox.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now postblox
```

## Stalwart Mail Server setup

postblox uses Stalwart for SMTP send/receive and account management. `postblox init` auto-configures Stalwart via its REST API — no manual Stalwart configuration is needed.

### Inbound email flow

1. Stalwart receives email via SMTP (port 25)
2. Stalwart's MTA Hook forwards the message to postblox: `POST /internal/stalwart/mta-hook`
3. postblox parses, deduplicates, threads, classifies, and stores the message
4. postblox responds with `{"set":[{"path":"/action","value":"discard"}]}` — Stalwart drops its copy
5. Events dispatched to webhooks, WebSocket, hooks, and notifications

The MTA Hook is auto-configured by `postblox init`. It can also be configured manually via Stalwart's settings API or the raw inbound endpoint:

| Endpoint | Auth | Format | Use |
|----------|------|--------|-----|
| `/internal/stalwart/mta-hook` | Basic auth (inbound_token) | MTA Hook JSON | Auto-configured by init |
| `/internal/stalwart/inbound` | Bearer token (inbound_token) | Raw MIME | Manual/IMAP sync |

### Outbound email flow

postblox sends outbound email via Stalwart's SMTP:
- `stalwart_smtp_host` — SMTP hostname
- `stalwart_smtp_port` — SMTP port (typically 25 for local Stalwart)

### Outbound relay (recommended for new deployments)

Fresh server IPs have no email reputation — outbound mail will land in spam. Use an SMTP relay to send through warm IPs:

```toml
relay_host = "smtp.mailgun.org"
relay_port = 587
relay_username = "postmaster@mg.example.com"
relay_password = "your-relay-password"
relay_starttls = true
```

`postblox init` can configure the relay in both postblox and Stalwart. Supported providers: Mailgun, Amazon SES, Postmark, or any SMTP relay.

### Domain DNS setup

After running `postblox init --domain example.com`, add the DNS records it prints:

| Record | Name | Value |
|--------|------|-------|
| MX | `example.com` | Your server IP / hostname |
| TXT | `example.com` | `v=spf1 mx ~all` |
| TXT | `default._domainkey.example.com` | DKIM key (from Stalwart) |
| TXT | `_dmarc.example.com` | `v=DMARC1; p=quarantine; rua=mailto:...` |

Verify with `postblox doctor` — it checks MX, SPF, DKIM, and DMARC records.

## PostgreSQL requirements

- PostgreSQL 15+
- pgvector extension (for semantic search — optional if not using embeddings)
- Max connections: postblox uses a pool of 10 connections (configurable)
- Acquire timeout: 3 seconds

## Production checklist

- [ ] Set strong `stalwart_admin_token` and `stalwart_inbound_token`
- [ ] Use unique API keys per integration (not the bootstrap key)
- [ ] Configure guard patterns for sensitive data (SSN, credit cards, etc.)
- [ ] Set up `before_send` hooks for additional validation
- [ ] Configure notification channels for approval alerts
- [ ] Set appropriate rate limits for your workload
- [ ] Register and verify your sending domain
- [ ] Enable semantic search with an embedding provider if needed
- [ ] Set `RUST_LOG=postblox=info` for production logging
