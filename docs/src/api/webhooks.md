# Webhooks

Register HTTP endpoints to receive real-time notifications for email events.

## Events

| Event | Trigger |
|-------|---------|
| `message.received` | Inbound email arrives |
| `message.sent` | Outbound email delivered |
| `approval.requested` | Message requires approval |
| `trust.changed` | Inbox trust level auto-upgraded |

## List webhooks

```
GET /api/v1/webhooks
```

**Response (200):** Array of webhook objects (secret is excluded from list responses).

```json
[
  {
    "id": "...",
    "org_id": "...",
    "url": "https://your-server.com/webhook",
    "events": ["message.received", "message.sent"],
    "active": true,
    "created_at": "2026-03-13T00:00:00Z"
  }
]
```

## Create webhook

```
POST /api/v1/webhooks
```

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `url` | string | yes | Callback URL |
| `events` | string[] | yes | Events to subscribe to |

**Response (201):** Webhook object including `secret` (shown only on creation).

```json
{
  "id": "...",
  "url": "https://your-server.com/webhook",
  "events": ["message.received"],
  "secret": "a1b2c3d4e5f6...",
  "active": true,
  "created_at": "2026-03-13T00:00:00Z"
}
```

## Verify webhook signatures

Webhook payloads are signed with HMAC-SHA256 using the webhook secret. The signature is sent in the `X-Postblox-Signature` header.

To verify:

```python
import hmac, hashlib

expected = hmac.new(
    webhook_secret.encode(),
    request_body,
    hashlib.sha256
).hexdigest()

assert hmac.compare_digest(expected, request.headers["X-Postblox-Signature"])
```

## Get webhook

```
GET /api/v1/webhooks/{id}
```

## Delete webhook

```
DELETE /api/v1/webhooks/{id}
```

**Response (204):** No content.
