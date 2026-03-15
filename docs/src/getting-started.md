# Getting Started

## Docker Compose quickstart

The fastest way to run postblox with all dependencies (PostgreSQL + pgvector, Stalwart Mail Server):

```bash
git clone https://github.com/numbery/postblox.git
cd postblox
docker compose up -d
```

This starts:
- **PostgreSQL** (pgvector/pgvector:pg17) on port 5432
- **Stalwart Mail Server** on ports 8080 (HTTP), 25/587/465 (SMTP), 993 (IMAP)
- **postblox** on port 3000

Wait for healthy status:

```bash
docker compose ps  # all services should show "healthy"
curl http://localhost:3000/health
# {"status":"ok","database":true}
```

## Setup with `postblox init`

The init wizard creates your config, runs migrations, configures Stalwart, and generates your first API key:

```bash
# Interactive mode
postblox init

# Non-interactive mode (CI/Docker)
postblox init --non-interactive \
  --database-url "postgres://postblox:postblox@localhost:5432/postblox" \
  --stalwart-url "http://localhost:8080" \
  --stalwart-admin-user admin \
  --stalwart-admin-token "your-admin-password" \
  --stalwart-inbound-token "your-secret-token" \
  --domain "example.com" \
  --org-name "my-org"
```

The wizard will:
1. Test database connectivity and run migrations
2. Connect to Stalwart and auto-configure the MTA Hook (inbound email forwarding)
3. Create your domain in Stalwart and print the DNS records you need to add
4. Optionally configure an outbound SMTP relay (Mailgun, SES, Postmark)
5. Generate `postblox.toml` with secure permissions (0600)
6. Create your organization and API key

Save the API key — it's shown only once. All subsequent API calls require it as a Bearer token.

## Verify with `postblox doctor`

```bash
postblox doctor
```

Checks: config file, database, migrations, Stalwart connectivity, MTA Hook configuration, DNS records (MX, SPF, DKIM, DMARC), embedding provider, relay connectivity, and file permissions.

## Create an inbox

```bash
export API_KEY="pb_abc12.deadbeefdeadbeefdeadbeef..."

curl -X POST http://localhost:3000/api/v1/inboxes \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"email": "bot@example.com", "display_name": "My Bot"}'
```

## Send a message

```bash
INBOX_ID="<inbox-id-from-above>"

curl -X POST "http://localhost:3000/api/v1/inboxes/$INBOX_ID/messages" \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "to": ["user@example.com"],
    "subject": "Hello from postblox",
    "text_body": "This email was sent by an AI agent."
  }'
```

If the inbox is in `approval` mode (the default), the response status will be `202 Accepted` and the message will await human approval before delivery.

## Receive via webhook

Register a webhook to get notified when emails arrive:

```bash
curl -X POST http://localhost:3000/api/v1/webhooks \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "url": "https://your-server.com/webhook",
    "events": ["message.received", "message.sent"]
  }'
```

The response includes a `secret` field for verifying webhook signatures (HMAC-SHA256).

## MCP setup for Claude Code

Add to your project's `.mcp.json`:

```json
{
  "mcpServers": {
    "postblox": {
      "command": "postblox-mcp",
      "env": {
        "POSTBLOX_URL": "http://localhost:3000",
        "POSTBLOX_API_KEY": "pb_abc12.deadbeef..."
      }
    }
  }
}
```

Or if using the Docker image:

```json
{
  "mcpServers": {
    "postblox": {
      "command": "docker",
      "args": ["run", "--rm", "-i", "--network=host",
               "-e", "POSTBLOX_URL=http://localhost:3000",
               "-e", "POSTBLOX_API_KEY=pb_abc12.deadbeef...",
               "postblox", "/postblox-mcp"]
    }
  }
}
```

Claude Code will then have access to 44 email management tools. See [MCP Tools](./mcp-tools.md) for the full list.

## Optional: Semantic search with Ollama

To enable semantic (vector similarity) search, start Ollama alongside:

```bash
docker compose --profile gpu up -d
docker exec -it postblox-ollama-1 ollama pull nomic-embed-text
```

Then configure the embedding provider in `postblox.toml`:

```toml
embedding_url = "http://localhost:11434/v1/embeddings"
embedding_model = "nomic-embed-text"
```

Or via environment variables:

```bash
EMBEDDING_URL=http://localhost:11434/v1/embeddings
EMBEDDING_MODEL=nomic-embed-text
```
