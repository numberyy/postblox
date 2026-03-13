# Labels

Labels are per-inbox tags that can be attached to messages for organization.

## List labels

```
GET /api/v1/inboxes/{inbox_id}/labels
```

**Response (200):** Array of label objects.

## Create label

```
POST /api/v1/inboxes/{inbox_id}/labels
```

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Label name |
| `color` | string | no | Hex color (e.g. `#ff0000`) |

**Response (201):** The created label.

## Delete label

```
DELETE /api/v1/inboxes/{inbox_id}/labels/{id}
```

**Response (204):** No content.

## List labels for a message

```
GET /api/v1/inboxes/{inbox_id}/messages/{message_id}/labels
```

## Add label to message

```
POST /api/v1/inboxes/{inbox_id}/messages/{message_id}/labels
```

**Request body:**

```json
{"label_id": "..."}
```

## Remove label from message

```
DELETE /api/v1/inboxes/{inbox_id}/messages/{message_id}/labels/{label_id}
```

**Response (204):** No content.
