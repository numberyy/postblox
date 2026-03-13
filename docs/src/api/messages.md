# Messages

Messages belong to an inbox. Outbound messages go through the permission engine before delivery.

## List messages

```
GET /api/v1/inboxes/{inbox_id}/messages
```

**Query parameters:**

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `limit` | integer | 50 | Max results (1-100) |
| `offset` | integer | 0 | Pagination offset |
| `thread_id` | UUID | *(none)* | Filter to a specific thread |
| `unslopify` | boolean | false | Exclude messages classified as slop |

**Response (200):** Array of message objects.

```json
[
  {
    "id": "...",
    "inbox_id": "...",
    "thread_id": "...",
    "message_id_header": "abc123@postblox",
    "from_addr": "bot@example.com",
    "to_addrs": ["user@example.com"],
    "subject": "Hello",
    "text_body": "...",
    "html_body": null,
    "direction": "outbound",
    "slop_score": null,
    "created_at": "2026-03-13T00:00:00Z"
  }
]
```

## Send message

```
POST /api/v1/inboxes/{inbox_id}/messages
```

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `to` | string[] | yes | Recipient email addresses (at least one) |
| `subject` | string | no | Email subject |
| `text_body` | string | no | Plain text body |
| `html_body` | string | no | HTML body |
| `cc` | string[] | no | CC recipients |

**Permission flow:**

1. Guard patterns checked against subject/body — blocked if matched
2. Permission mode checked for the inbox:
   - `shadow` — send is rejected (403)
   - `approval` — message stored, approval created (202 Accepted)
   - `auto_approve` — rules evaluated (slop score, domain allowlist, etc.)
   - `autonomous` — sent immediately
3. `before_send` hooks run — blocked if any hook returns `block` or fails
4. Message delivered via Stalwart SMTP

**Response (201):** The created message object (sent immediately).

**Response (202):** The created message object (awaiting approval).

## Get message

```
GET /api/v1/inboxes/{inbox_id}/messages/{id}
```

**Response (200):** Single message object.

## Delivery status

```
GET /api/v1/inboxes/{inbox_id}/messages/{id}/delivery-status
```

**Response (200):** Delivery status including any bounce information.
