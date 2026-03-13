# Notifications

Notification channels deliver alerts for email events via ntfy, email, or webhook.

## List notifications

```
GET /api/v1/notifications
```

**Response (200):** Array of notification channel objects.

## Create notification

```
POST /api/v1/notifications
```

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `provider` | string | yes | `ntfy`, `email`, or `webhook` |
| `config` | object | yes | Provider-specific configuration |

### Provider configs

**ntfy:**
```json
{
  "provider": "ntfy",
  "config": {
    "url": "https://ntfy.sh",
    "topic": "postblox-alerts"
  }
}
```

**webhook:**
```json
{
  "provider": "webhook",
  "config": {
    "url": "https://your-server.com/notify"
  }
}
```

**Response (201):** The created notification channel.

## Delete notification

```
DELETE /api/v1/notifications/{id}
```

**Response (204):** No content.
