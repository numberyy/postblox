# Audit

The audit log records all significant actions: permission changes, message approvals/rejections, trust upgrades.

## List audit entries

```
GET /api/v1/audit
```

**Query parameters:**

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `limit` | integer | 50 | Max results (1-100) |
| `offset` | integer | 0 | Pagination offset |
| `inbox_id` | UUID | *(none)* | Filter by inbox |
| `action` | string | *(none)* | Filter by action type |
| `after` | ISO datetime | *(none)* | Entries after this time |
| `before` | ISO datetime | *(none)* | Entries before this time |

**Response (200):** Array of audit entries.

```json
[
  {
    "id": "...",
    "org_id": "...",
    "inbox_id": "...",
    "action": "message_approved",
    "actor": "admin@example.com",
    "details": {"message_id": "...", "approval_id": "..."},
    "created_at": "2026-03-13T00:00:00Z"
  }
]
```

## Action types

| Action | Description |
|--------|-------------|
| `message_approved` | An outbound message was approved |
| `message_rejected` | An outbound message was rejected |
| `permission_changed` | Inbox permissions were modified |
