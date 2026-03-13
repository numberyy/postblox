# Getting Started

## Docker Compose quickstart

The fastest way to run postblox with all dependencies (PostgreSQL + pgvector, Stalwart Mail Server):

```bash
git clone https://github.com/numberly/postblox.git
cd postblox
docker compose up -d
```

This starts:
- **PostgreSQL** (pgvector/pgvector:pg17) on port 5432
- **Stalwart Mail Server** on ports 8080 (HTTP), 25 (SMTP), 993 (IMAP)
- **postblox** on port 3000

Wait for healthy status:

```bash
docker compose ps  # postgres and stalwart should show "healthy"; postblox shows "running"
curl http://localhost:3000/health
# {"status":"ok","database":true}
```

## Bootstrap an organization

Create your first organization and API key:

```bash
curl -X POST http://localhost:3000/api/v1/organizations \
  -H "Content-Type: application/json" \
  -d '{"name": "my-org"}'
```

Response:

```json
{
  "organization": {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "name": "my-org",
    "created_at": "2026-03-13T00:00:00Z"
  },
  "api_key": "pb_abc12.deadbeefdeadbeefdeadbeef..."
}
```

Save the `api_key` — it's shown only once. All subsequent API calls require it as a Bearer token.

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
