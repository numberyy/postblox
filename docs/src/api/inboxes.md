# Inboxes

An inbox represents an email address managed by postblox.

## List inboxes

```
GET /api/v1/inboxes
```

**Response (200):** Array of inbox objects.

```json
[
  {
    "id": "...",
    "org_id": "...",
    "email": "bot@example.com",
    "display_name": "My Bot",
    "inbox_type": "native",
    "created_at": "2026-03-13T00:00:00Z"
  }
]
```

## Create inbox

```
POST /api/v1/inboxes
```

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `email` | string | yes | Email address |
| `display_name` | string | no | Display name |
| `inbox_type` | string | no | `native` (default) or `linked` |

**Response (201):** The created inbox object.

If Stalwart is configured, a corresponding mail account is created automatically.

## Get inbox

```
GET /api/v1/inboxes/{id}
```

**Response (200):** Single inbox object.

## Delete inbox

```
DELETE /api/v1/inboxes/{id}
```

**Response (204):** No content.

If Stalwart is configured, the corresponding mail account is deleted.
