# Briefing

Get a summary of recent email activity across all inboxes.

## Get briefing

```
GET /api/v1/briefing
```

**Query parameters:**

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `period` | string | `24h` | Time window: `1h`, `6h`, `12h`, `24h`, or `7d` |

**Response (200):**

```json
{
  "period": "24h",
  "since": "2026-03-12T00:00:00Z",
  "total_received": 42,
  "total_sent": 15,
  "by_inbox": [
    {
      "inbox_id": "...",
      "inbox_email": "bot@example.com",
      "received": 42,
      "sent": 15
    }
  ],
  "top_senders": [
    {"address": "alice@example.com", "count": 8}
  ],
  "top_subjects": [
    {"subject": "Weekly Report", "count": 3}
  ]
}
```
