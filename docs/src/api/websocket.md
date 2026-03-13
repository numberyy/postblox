# WebSocket

Real-time event stream via WebSocket. Events are scoped to the authenticated organization.

## Connect

```
ws://localhost:3000/api/v1/ws?key=pb_abc12.deadbeef...
```

Authentication is via the `key` query parameter (not a header).

## Events

Events are sent as JSON messages:

```json
{
  "event": "message.received",
  "data": {
    "inbox_id": "...",
    "message_id": "..."
  }
}
```

| Event | Data fields | Description |
|-------|-------------|-------------|
| `message.received` | `inbox_id`, `message_id` | New inbound email |
| `message.sent` | `inbox_id`, `message_id` | Outbound email delivered |
| `approval.requested` | `inbox_id`, `message_id`, `approval_id` | Message awaiting approval |
| `trust.changed` | `inbox_id`, `new_mode`, `approved_count` | Inbox trust level changed |

## Connection limits

Maximum 1,000 concurrent WebSocket connections per server instance.
