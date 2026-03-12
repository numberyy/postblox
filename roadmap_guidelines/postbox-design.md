# PostBox — Open Source Email Infrastructure for AI Agents

> Self-hosted, API-first email platform that gives AI agents their own inboxes.  
> The open-source alternative to AgentMail.

---

## Why PostBox?

AgentMail solves a real problem — AI agents need email identities to participate on the internet. But it's a paid SaaS with no self-hosted option. PostBox replicates the full AgentMail feature set on infrastructure you own:

- **$0/month** — run on your own server or a $5 VPS
- **Unlimited inboxes** — no per-inbox pricing
- **No rate limits** — your server, your rules
- **Full data ownership** — emails never leave your infrastructure
- **MCP native** — built-in MCP server for Claude Code, Cursor, and other AI tools

---

## Architecture

```
┌──────────────────────────────────────────────────────────────┐
│                      Agent / Client Layer                     │
│     Claude Code │ Cursor │ LangChain │ CrewAI │ Custom       │
└──────────┬──────────────────────────┬────────────────────────┘
           │  REST API / WebSocket    │  MCP (stdio/SSE)
┌──────────▼──────────────────────────▼────────────────────────┐
│                    PostBox API Server                         │
│                   (Elixir / Phoenix)                          │
│                                                              │
│  ┌─────────────┐ ┌──────────────┐ ┌────────────────────┐    │
│  │ Inbox Mgr   │ │ Thread/Msg   │ │ Webhook Dispatcher │    │
│  │ (GenServer) │ │ Engine       │ │ (Broadway)         │    │
│  └─────────────┘ └──────────────┘ └────────────────────┘    │
│  ┌─────────────┐ ┌──────────────┐ ┌────────────────────┐    │
│  │ Label/List  │ │ Attachment   │ │ Domain Manager     │    │
│  │ Engine      │ │ Store        │ │ (DNS + DKIM)       │    │
│  └─────────────┘ └──────────────┘ └────────────────────┘    │
│  ┌─────────────┐ ┌──────────────┐ ┌────────────────────┐    │
│  │ MCP Server  │ │ WebSocket    │ │ Semantic Search    │    │
│  │ (stdio/SSE) │ │ (Channels)  │ │ (pgvector)         │    │
│  └─────────────┘ └──────────────┘ └────────────────────┘    │
└──────────┬───────────────────────────────────────────────────┘
           │  JMAP / IMAP / SMTP (internal)
┌──────────▼───────────────────────────────────────────────────┐
│                   Stalwart Mail Server                        │
│                                                              │
│  SMTP (25/587)  │  IMAP (143/993)  │  JMAP  │  Management   │
│  SPF/DKIM/DMARC │  TLS/DANE        │  Push  │  REST API     │
└──────────────────────────────────────────────────────────────┘
           │
     DNS (MX, SPF, DKIM, DMARC records)
```

### Why This Stack?

**Stalwart Mail Server** handles the hard email problems — deliverability, SPF/DKIM/DMARC, SMTP queueing, spam filtering. It's written in Rust, actively maintained, has a full REST management API, supports JMAP push events, and can programmatically create accounts. We don't reinvent any of this.

**Elixir/Phoenix** sits on top as the agent-facing API layer. The BEAM VM is ideal here: each agent inbox maps naturally to a GenServer process, Phoenix Channels give us real-time WebSocket events out of the box, Broadway handles webhook delivery with backpressure, and the supervision tree means individual inbox crashes don't take down the system. Phoenix is also the most natural fit for the MCP server implementation.

**PostgreSQL + pgvector** for metadata, threading, labels, and semantic search over email content.

---

## Core Data Model

### Hierarchy

```
Organization (tenant)
  └── Domain (custom domains, verified via DNS)
  └── API Key (scoped permissions)
  └── Inbox (agent email identity)
        └── Thread (email conversation)
              └── Message (individual email)
                    └── Attachment (files)
        └── Label (categorization)
        └── Draft (unsent messages)
```

### Key Entities

```elixir
# Organization — top-level tenant
%Organization{
  id: uuid,
  name: string,
  settings: map,        # default domain, retention policy, etc.
  created_at: datetime
}

# Inbox — an agent's email identity
%Inbox{
  id: uuid,             # internal ID
  inbox_id: string,     # "sales-agent@yourdomain.com"
  organization_id: uuid,
  username: string,
  domain: string,
  display_name: string,
  client_id: string,    # idempotency key
  metadata: map,        # arbitrary agent metadata
  created_at: datetime
}

# Thread — groups related messages
%Thread{
  id: uuid,
  thread_id: string,
  inbox_id: uuid,
  subject: string,
  labels: [string],
  senders: [string],
  recipients: [string],
  preview: string,
  last_message_id: string,
  message_count: integer,
  size: integer,
  created_at: datetime,
  updated_at: datetime
}

# Message — individual email
%Message{
  id: uuid,
  message_id: string,   # RFC 2822 Message-ID
  inbox_id: uuid,
  thread_id: uuid,
  labels: [string],      # ["received", "important", ...]
  from: string,
  to: [string],
  cc: [string],
  bcc: [string],
  reply_to: string,
  subject: string,
  text: text,            # plain text body
  html: text,            # HTML body
  extracted_text: text,  # reply content without quoted history
  extracted_html: text,
  preview: string,       # short preview
  in_reply_to: string,   # threading header
  references: [string],  # threading headers
  size: integer,
  embedding: vector(1536), # for semantic search
  created_at: datetime
}

# Attachment
%Attachment{
  id: uuid,
  message_id: uuid,
  filename: string,
  content_type: string,
  size: integer,
  storage_path: string,  # local filesystem or S3-compat
  created_at: datetime
}

# Webhook
%Webhook{
  id: uuid,
  organization_id: uuid,
  url: string,
  secret: string,        # HMAC signing key
  events: [string],      # ["message.received", "message.sent", ...]
  active: boolean,
  created_at: datetime
}

# Domain
%Domain{
  id: uuid,
  organization_id: uuid,
  domain: string,
  status: :pending | :verified | :failed,
  dns_records: [map],    # required DNS records for verification
  dkim_selector: string,
  created_at: datetime
}
```

---

## API Design

### Authentication

API key via `Authorization: Bearer pb_...` header. Keys are scoped to an organization and can have granular permissions.

### Endpoints

All endpoints are prefixed with `/api/v1/`.

#### Inboxes

```
POST   /inboxes                    # Create inbox
GET    /inboxes                    # List inboxes (paginated)
GET    /inboxes/:inbox_id          # Get inbox
PATCH  /inboxes/:inbox_id          # Update inbox
DELETE /inboxes/:inbox_id          # Delete inbox
```

**Create Inbox:**
```json
POST /api/v1/inboxes
{
  "username": "sales-agent",       // optional, auto-generated if omitted
  "domain": "yourdomain.com",     // optional, uses default domain
  "display_name": "Sales Agent",  // optional
  "client_id": "my-agent-v1",     // optional, idempotency key
  "metadata": {                    // optional, arbitrary JSON
    "agent_type": "sales",
    "model": "claude-sonnet-4-20250514"
  }
}
```

**Response:**
```json
{
  "inbox_id": "sales-agent@yourdomain.com",
  "username": "sales-agent",
  "domain": "yourdomain.com",
  "display_name": "Sales Agent",
  "metadata": { ... },
  "created_at": "2026-03-10T12:00:00Z"
}
```

#### Messages

```
POST   /inboxes/:inbox_id/messages/send     # Send email
GET    /inboxes/:inbox_id/messages           # List messages (paginated)
GET    /inboxes/:inbox_id/messages/:msg_id   # Get message
DELETE /inboxes/:inbox_id/messages/:msg_id   # Delete message
```

**Send Message:**
```json
POST /api/v1/inboxes/sales-agent@yourdomain.com/messages/send
{
  "to": ["user@example.com"],
  "cc": [],
  "bcc": [],
  "subject": "Follow up on your inquiry",
  "text": "Plain text body",
  "html": "<p>HTML body</p>",
  "reply_to": "thread-msg-id",    // optional, for threading
  "attachments": [                 // optional
    {
      "filename": "proposal.pdf",
      "content_type": "application/pdf",
      "data": "base64..."
    }
  ]
}
```

#### Threads

```
GET    /inboxes/:inbox_id/threads            # List threads (paginated)
GET    /inboxes/:inbox_id/threads/:thread_id # Get thread with messages
DELETE /inboxes/:inbox_id/threads/:thread_id # Delete thread
```

#### Labels

```
POST   /inboxes/:inbox_id/labels             # Create label
GET    /inboxes/:inbox_id/labels             # List labels
PUT    /inboxes/:inbox_id/messages/:id/labels # Set message labels
DELETE /inboxes/:inbox_id/labels/:label_id   # Delete label
```

#### Drafts

```
POST   /inboxes/:inbox_id/drafts             # Create draft
GET    /inboxes/:inbox_id/drafts             # List drafts
PATCH  /inboxes/:inbox_id/drafts/:draft_id   # Update draft
POST   /inboxes/:inbox_id/drafts/:draft_id/send # Send draft
DELETE /inboxes/:inbox_id/drafts/:draft_id   # Delete draft
```

#### Domains

```
POST   /domains                    # Add domain
GET    /domains                    # List domains
GET    /domains/:domain_id         # Get domain (with DNS records)
POST   /domains/:domain_id/verify  # Trigger verification
DELETE /domains/:domain_id         # Remove domain
```

#### Webhooks

```
POST   /webhooks                   # Register webhook
GET    /webhooks                   # List webhooks
PATCH  /webhooks/:webhook_id       # Update webhook
DELETE /webhooks/:webhook_id       # Delete webhook
```

#### Search

```
POST   /search                     # Semantic search across inboxes
{
  "query": "invoices from last quarter",
  "inbox_ids": ["inbox1", "inbox2"],  // optional, search all if omitted
  "limit": 10
}
```

#### Organization

```
GET    /organization               # Get org info
PATCH  /organization               # Update org settings
GET    /organization/metrics       # Usage metrics
```

---

## Webhook Events

PostBox mirrors AgentMail's event model. All payloads include:

```json
{
  "type": "event",
  "event_type": "message.received",
  "event_id": "evt_...",
  "timestamp": "2026-03-10T12:00:00Z",
  // ... event-specific data
}
```

### Event Types

| Event | Trigger | Payload |
|-------|---------|---------|
| `message.received` | Email arrives in inbox | `message` + `thread` objects |
| `message.sent` | Email sent from inbox | `send` with recipients |
| `message.delivered` | Remote server accepted | `delivery` with recipients |
| `message.bounced` | Delivery failed | `bounce` with type/reason |
| `message.complained` | Recipient marked spam | `complaint` with type |
| `message.rejected` | Pre-send validation fail | `reject` with reason |
| `domain.verified` | DNS verification passed | `domain` with status |

### Webhook Security

Payloads are signed with HMAC-SHA256. The signature is sent in the `X-PostBox-Signature` header. Consumers verify by computing `HMAC-SHA256(webhook_secret, raw_body)`.

---

## WebSocket Events

Real-time events via Phoenix Channels at `wss://your-server/socket`.

```javascript
// Connect
const socket = new Socket("wss://your-server/socket", {
  params: { api_key: "pb_..." }
});
socket.connect();

// Subscribe to inbox events
const channel = socket.channel("inbox:sales-agent@yourdomain.com");
channel.on("message.received", (payload) => {
  console.log("New email:", payload.message.subject);
});
channel.join();

// Or subscribe to all org events
const orgChannel = socket.channel("organization:events");
```

---

## MCP Server

PostBox includes a built-in MCP (Model Context Protocol) server so AI agents can interact with email directly from Claude Code, Cursor, or any MCP-compatible client.

### Tools Exposed

```
postbox_create_inbox      — Create a new agent inbox
postbox_list_inboxes      — List all inboxes
postbox_send_email        — Send an email from an inbox
postbox_list_messages     — List messages in an inbox
postbox_get_message       — Get a specific message
postbox_list_threads      — List conversation threads
postbox_get_thread        — Get a thread with all messages
postbox_search            — Semantic search across inboxes
postbox_reply             — Reply to a message in a thread
postbox_add_label         — Add a label to a message
postbox_create_draft      — Create a draft email
postbox_send_draft        — Send an existing draft
```

### Configuration (Claude Code)

```json
// .claude/settings.json
{
  "mcpServers": {
    "postbox": {
      "type": "sse",
      "url": "http://localhost:4000/mcp/sse",
      "headers": {
        "Authorization": "Bearer pb_..."
      }
    }
  }
}
```

---

## Reply Extraction (Talon)

When emails arrive, PostBox strips quoted reply history and signature blocks using [talon](https://github.com/mailgun/talon) (or a port of it). This populates the `extracted_text` and `extracted_html` fields, giving agents just the new content without the noise of quoted history.

---

## Semantic Search

PostBox embeds email content on ingest using a configurable embedding model (defaults to a local model via Ollama, or can use OpenAI/Anthropic APIs). Embeddings are stored in pgvector and queried via cosine similarity.

```
POST /api/v1/search
{
  "query": "customer complaints about billing",
  "inbox_ids": ["support@yourdomain.com"],
  "limit": 10,
  "threshold": 0.7
}
```

The embedding provider is configurable in `config/runtime.exs`:

```elixir
config :postbox, PostBox.Embeddings,
  provider: :ollama,           # :ollama | :openai | :anthropic | :local
  model: "nomic-embed-text",   # model name
  base_url: "http://localhost:11434"
```

---

## Deployment

### Docker Compose (Recommended)

```yaml
version: "3.8"

services:
  postbox:
    image: ghcr.io/postbox/postbox:latest
    ports:
      - "4000:4000"      # API + WebSocket
      - "4001:4001"      # MCP SSE
    environment:
      DATABASE_URL: postgres://postbox:secret@db:5432/postbox
      STALWART_API_URL: http://stalwart:8080
      STALWART_API_KEY: your-stalwart-api-key
      SECRET_KEY_BASE: generate-with-mix-phx-gen-secret
      POSTBOX_API_KEY: pb_your-api-key
    depends_on:
      - db
      - stalwart

  stalwart:
    image: stalwartlabs/stalwart:latest
    ports:
      - "25:25"          # SMTP
      - "587:587"        # Submission
      - "993:993"        # IMAPS
      - "443:443"        # JMAP + Web Admin
    volumes:
      - stalwart_data:/opt/stalwart

  db:
    image: pgvector/pgvector:pg17
    environment:
      POSTGRES_DB: postbox
      POSTGRES_USER: postbox
      POSTGRES_PASSWORD: secret
    volumes:
      - pg_data:/var/lib/postgresql/data

  # Optional: local embeddings
  ollama:
    image: ollama/ollama:latest
    volumes:
      - ollama_data:/root/.ollama

volumes:
  stalwart_data:
  pg_data:
  ollama_data:
```

### Bare Metal / systemd

PostBox runs as a systemd service alongside Stalwart. For Fedora/RHEL:

```bash
# Install Elixir + PostgreSQL
sudo dnf install elixir postgresql-server

# Clone and build
git clone https://github.com/yourorg/postbox
cd postbox
mix deps.get && mix release

# Configure
cp config/runtime.example.exs config/runtime.exs
# Edit with your Stalwart API URL, database URL, etc.

# Run migrations
./bin/postbox eval "PostBox.Release.migrate()"

# Install systemd service
sudo cp deploy/postbox.service /etc/systemd/system/
sudo systemctl enable --now postbox
```

---

## Project Structure

```
postbox/
├── config/
│   ├── config.exs
│   ├── dev.exs
│   ├── prod.exs
│   ├── runtime.exs
│   └── test.exs
├── lib/
│   ├── postbox/
│   │   ├── accounts/            # Organizations, API keys
│   │   │   ├── organization.ex
│   │   │   └── api_key.ex
│   │   ├── inboxes/             # Inbox CRUD + lifecycle
│   │   │   ├── inbox.ex
│   │   │   └── inbox_manager.ex # GenServer per inbox
│   │   ├── mail/                # Message handling
│   │   │   ├── message.ex
│   │   │   ├── thread.ex
│   │   │   ├── draft.ex
│   │   │   ├── attachment.ex
│   │   │   ├── label.ex
│   │   │   └── reply_extractor.ex  # Talon-style
│   │   ├── delivery/            # Sending + event tracking
│   │   │   ├── sender.ex        # SMTP via Stalwart
│   │   │   ├── receiver.ex      # Inbound processing
│   │   │   └── events.ex        # Delivery events
│   │   ├── domains/             # Domain management + DNS
│   │   │   ├── domain.ex
│   │   │   └── verifier.ex
│   │   ├── webhooks/            # Webhook dispatch
│   │   │   ├── webhook.ex
│   │   │   ├── dispatcher.ex    # Broadway pipeline
│   │   │   └── signer.ex        # HMAC signing
│   │   ├── search/              # Semantic search
│   │   │   ├── embeddings.ex
│   │   │   └── search.ex
│   │   ├── stalwart/            # Stalwart API client
│   │   │   ├── client.ex
│   │   │   ├── accounts.ex
│   │   │   └── jmap.ex
│   │   ├── mcp/                 # MCP server
│   │   │   ├── server.ex
│   │   │   ├── router.ex
│   │   │   └── tools.ex
│   │   ├── repo.ex
│   │   └── application.ex
│   ├── postbox_web/
│   │   ├── controllers/
│   │   │   ├── inbox_controller.ex
│   │   │   ├── message_controller.ex
│   │   │   ├── thread_controller.ex
│   │   │   ├── draft_controller.ex
│   │   │   ├── label_controller.ex
│   │   │   ├── domain_controller.ex
│   │   │   ├── webhook_controller.ex
│   │   │   ├── search_controller.ex
│   │   │   └── organization_controller.ex
│   │   ├── channels/
│   │   │   ├── inbox_channel.ex
│   │   │   └── org_channel.ex
│   │   ├── plugs/
│   │   │   ├── api_auth.ex
│   │   │   └── idempotency.ex
│   │   ├── router.ex
│   │   └── endpoint.ex
│   └── postbox_web.ex
├── priv/
│   └── repo/migrations/
├── test/
├── docker-compose.yml
├── Dockerfile
├── mix.exs
├── README.md
└── LICENSE                      # MIT
```

---

## SDK Plan

### Python SDK (Priority 1)

Most AI agent frameworks are Python-based. Ship a `postbox-python` package:

```python
from postbox import PostBox

client = PostBox(
    api_key="pb_...",
    base_url="http://localhost:4000"  # self-hosted
)

# Create inbox
inbox = client.inboxes.create(
    username="sales-agent",
    display_name="Sales Agent"
)

# Send email
client.inboxes.messages.send(
    inbox.inbox_id,
    to="user@example.com",
    subject="Hello from my agent!",
    text="This is an automated message."
)

# List messages
messages = client.inboxes.messages.list(
    inbox.inbox_id,
    limit=10
)

# Semantic search
results = client.search(
    query="billing complaints",
    inbox_ids=[inbox.inbox_id]
)
```

### TypeScript SDK (Priority 2)

For Node.js agent frameworks:

```typescript
import { PostBox } from "postbox-sdk";

const client = new PostBox({
  apiKey: "pb_...",
  baseUrl: "http://localhost:4000",
});

const inbox = await client.inboxes.create({
  username: "support-agent",
});

await client.inboxes.messages.send(inbox.inboxId, {
  to: "user@example.com",
  subject: "Hello!",
  text: "Message body",
});
```

### Elixir Client (Priority 3)

For Pulse integration and other Elixir projects:

```elixir
{:ok, client} = PostBox.Client.new(api_key: "pb_...", base_url: "http://localhost:4000")

{:ok, inbox} = PostBox.Inboxes.create(client, %{
  username: "research-agent",
  display_name: "Research Agent"
})

{:ok, _msg} = PostBox.Messages.send(client, inbox.inbox_id, %{
  to: ["user@example.com"],
  subject: "Research findings",
  text: "Here are the results..."
})
```

---

## Integration Points

### Framework Integrations

| Framework | Integration Type | Description |
|-----------|-----------------|-------------|
| LangChain | Tool | `PostBoxSendTool`, `PostBoxReadTool` |
| CrewAI | Tool | Agent email capabilities |
| LlamaIndex | Tool | Email as data source |
| Jido | Native | Direct BEAM integration |
| OpenClaw | Skill | Email skill plugin |

### Pulse Integration

PostBox is designed to work alongside Pulse as the email layer:

- Pulse's notification triage can route email alerts through PostBox
- PostBox webhook events feed into Pulse's event stream
- Shared BEAM runtime — Pulse can directly call PostBox modules

---

## Differentiation from AgentMail

| Feature | AgentMail | PostBox |
|---------|-----------|---------|
| Hosting | SaaS only | Self-hosted |
| Pricing | $0-$500/mo | Free (your infra) |
| Inboxes | 3-300 per plan | Unlimited |
| Rate limits | Plan-dependent | None (your server) |
| Data ownership | Their servers | Your servers |
| MCP server | Yes | Yes (built-in) |
| Webhooks | Yes | Yes |
| WebSockets | Yes | Yes (Phoenix Channels) |
| Semantic search | Yes | Yes (pgvector + configurable model) |
| Reply extraction | Yes (Talon) | Yes |
| Custom domains | Paid plans only | Free (your DNS) |
| IMAP/SMTP access | Yes | Yes (via Stalwart) |
| SDKs | Python, Node | Python, Node, Elixir |
| Source | Closed | MIT License |

---

## Roadmap

### Phase 1 — Core (v0.1)
- [ ] Stalwart integration (account CRUD via REST API)
- [ ] Inbox create/list/get/delete
- [ ] Send + receive messages
- [ ] Threading (In-Reply-To / References headers)
- [ ] Basic webhook dispatch (message.received, message.sent)
- [ ] API key authentication
- [ ] Docker Compose deployment
- [ ] Python SDK

### Phase 2 — Parity (v0.2)
- [ ] Labels and label management
- [ ] Drafts
- [ ] Attachments (upload, download, inline)
- [ ] Reply extraction (extracted_text / extracted_html)
- [ ] All webhook event types (delivered, bounced, complained, rejected)
- [ ] WebSocket real-time events (Phoenix Channels)
- [ ] Domain management + DNS verification
- [ ] Idempotency support (client_id)
- [ ] TypeScript SDK

### Phase 3 — Intelligence (v0.3)
- [ ] Semantic search (pgvector + embedding provider)
- [ ] Automatic labeling (configurable prompts)
- [ ] Structured data extraction from emails
- [ ] MCP server (stdio + SSE transport)
- [ ] Elixir SDK
- [ ] Web admin dashboard (LiveView)

### Phase 4 — Scale (v1.0)
- [ ] Multi-tenancy with org isolation
- [ ] Metrics and analytics
- [ ] Bounce rate tracking + auto-suppression
- [ ] Batch operations
- [ ] Rate limiting (configurable per-org)
- [ ] Kubernetes deployment manifests
- [ ] Comprehensive docs site

---

## License

MIT — use it however you want, contribute back if you can.
