# API Keys

Manage API keys for your organization. Keys use the format `pb_<prefix>.<random>`.

## List API keys

```
GET /api/v1/api-keys
```

**Response (200):** Array of API key objects (hash and full key are never returned).

```json
[
  {
    "id": "...",
    "org_id": "...",
    "prefix": "pb_abc12",
    "name": "default",
    "last_used_at": "2026-03-13T00:00:00Z",
    "created_at": "2026-03-13T00:00:00Z"
  }
]
```

## Create API key

```
POST /api/v1/api-keys
```

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | no | Human-readable key name |

**Response (201):** Key object including the full `api_key` (shown only once).

## Delete API key

```
DELETE /api/v1/api-keys/{id}
```

**Response (204):** No content.
