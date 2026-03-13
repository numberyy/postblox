# Trust

Progressive trust tracks an inbox's approval history. After enough approved messages, an inbox can auto-upgrade from `approval` to `auto_approve` mode.

## Get trust level

```
GET /api/v1/inboxes/{inbox_id}/trust
```

**Response (200):**

```json
{
  "inbox_id": "...",
  "approved_count": 7,
  "rejected_count": 1,
  "threshold": 10,
  "current_mode": "approval"
}
```

## Auto-upgrade behavior

When `approved_count` reaches the configured `trust_auto_upgrade_threshold` (default 10), the inbox's send mode automatically upgrades from `approval` to `auto_approve`.

This triggers:
- An audit log entry with action `permission_changed` and actor `system:trust_auto_upgrade`
- A `trust.changed` event dispatched to webhooks and WebSocket
