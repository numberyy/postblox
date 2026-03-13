# Organizations

Organizations are the top-level tenant. All inboxes, messages, and settings belong to an organization.

## Bootstrap

Creates a new organization and returns its first API key. This is the only unauthenticated `/api/v1` endpoint.

```
POST /api/v1/organizations
```

**Request body:**

```json
{
  "name": "my-org"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Organization name |

**Response (201):**

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

The `api_key` is shown only once. Store it securely.
