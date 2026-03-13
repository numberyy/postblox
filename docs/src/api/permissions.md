# Permissions

Permissions control how an inbox handles outbound messages. Each inbox has one permission configuration.

## Send modes

| Mode | Behavior |
|------|----------|
| `shadow` | All outbound sends are rejected. The inbox can only receive. |
| `approval` | Messages are held for human approval before delivery. **(default)** |
| `auto_approve` | Messages are checked against rules, then sent automatically if they pass. |
| `autonomous` | Messages are sent immediately with no checks (beyond guard patterns and hooks). |

## Get permissions

```
GET /api/v1/inboxes/{inbox_id}/permissions
```

**Response (200):** Permission object. Returns default values (`approval` mode, no rules) if no explicit permission has been set.

```json
{
  "id": "...",
  "inbox_id": "...",
  "send_mode": "approval",
  "rules": [],
  "created_at": "2026-03-13T00:00:00Z",
  "updated_at": "2026-03-13T00:00:00Z"
}
```

## Set permissions

```
PUT /api/v1/inboxes/{inbox_id}/permissions
```

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `send_mode` | string | no | One of: `shadow`, `approval`, `auto_approve`, `autonomous` |
| `rules` | array | no | Permission rules (only evaluated in `auto_approve` mode) |

## Rule types

Rules are evaluated in `auto_approve` mode. If any rule blocks, the send is rejected.

### domain_allowlist

Only allow sending to specific domains.

```json
{"type": "domain_allowlist", "domains": ["ok.com", "trusted.org"]}
```

### domain_blocklist

Block sending to specific domains.

```json
{"type": "domain_blocklist", "domains": ["competitor.com"]}
```

### keyword_blocklist

Block messages containing specific keywords in subject or body.

```json
{"type": "keyword_blocklist", "keywords": ["confidential", "secret"]}
```

### slop_threshold

Block messages with a slop (AI-generated content) score above a threshold.

```json
{"type": "slop_threshold", "threshold": 0.8}
```

### time_window

Only allow sending during specific hours.

```json
{"type": "time_window", "start_hour": 9, "end_hour": 17, "timezone": "America/New_York"}
```

### Example: combined rules

```bash
curl -X PUT "http://localhost:3000/api/v1/inboxes/$INBOX_ID/permissions" \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "send_mode": "auto_approve",
    "rules": [
      {"type": "domain_allowlist", "domains": ["customers.com"]},
      {"type": "slop_threshold", "threshold": 0.7},
      {"type": "time_window", "start_hour": 9, "end_hour": 17, "timezone": "UTC"}
    ]
  }'
```
