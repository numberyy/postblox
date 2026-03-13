# Threads

Threads group related messages using `In-Reply-To` and `References` headers. Threading is automatic — no manual thread management needed.

## List threads

```
GET /api/v1/inboxes/{inbox_id}/threads
```

**Query parameters:**

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `limit` | integer | 50 | Max results (1-100) |
| `offset` | integer | 0 | Pagination offset |

**Response (200):** Array of thread objects.

```json
[
  {
    "id": "...",
    "inbox_id": "...",
    "subject": "Re: Project update",
    "message_count": 5,
    "last_message_at": "2026-03-13T12:00:00Z",
    "created_at": "2026-03-10T08:00:00Z"
  }
]
```

## Get thread

```
GET /api/v1/inboxes/{inbox_id}/threads/{id}
```

**Response (200):** Thread object with its messages.
