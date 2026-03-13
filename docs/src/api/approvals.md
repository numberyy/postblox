# Approvals

When an inbox is in `approval` mode, outbound messages require human approval before delivery. Approvals track the decision lifecycle.

## List approvals

```
GET /api/v1/approvals
```

**Query parameters:**

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `limit` | integer | 50 | Max results (1-100) |
| `offset` | integer | 0 | Pagination offset |
| `status` | string | *(none)* | Filter by status: `pending`, `approved`, `rejected` |

**Response (200):** Array of approval objects.

```json
[
  {
    "id": "...",
    "org_id": "...",
    "inbox_id": "...",
    "message_id": "...",
    "status": "pending",
    "decided_by": null,
    "decided_at": null,
    "created_at": "2026-03-13T00:00:00Z"
  }
]
```

## Get approval

```
GET /api/v1/approvals/{id}
```

## Approve message

```
POST /api/v1/approvals/{id}/approve
```

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `decided_by` | string | yes | Who approved (email or identifier) |

Approving a message triggers delivery via Stalwart SMTP and records a trust point for the inbox. After reaching the `trust_auto_upgrade_threshold`, the inbox auto-upgrades to `auto_approve` mode.

**Response (200):** Updated approval object.

## Reject message

```
POST /api/v1/approvals/{id}/reject
```

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `decided_by` | string | yes | Who rejected |

**Response (200):** Updated approval object.

## Batch approve/reject

```
POST /api/v1/approvals/batch
```

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `ids` | UUID[] | yes | Approval IDs to decide (non-empty) |
| `status` | string | yes | `approved` or `rejected` |
| `decided_by` | string | yes | Who made this decision |

**Response (200):** Array of updated approval objects.
