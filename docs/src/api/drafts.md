# Drafts

Drafts are unsent messages that can be edited before sending.

## List drafts

```
GET /api/v1/inboxes/{inbox_id}/drafts
```

**Query parameters:**

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `limit` | integer | 50 | Max results (1-100) |
| `offset` | integer | 0 | Pagination offset |

**Response (200):** Array of draft objects.

## Create draft

```
POST /api/v1/inboxes/{inbox_id}/drafts
```

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `to` | string[] | no | Recipient email addresses |
| `cc` | string[] | no | CC recipients |
| `subject` | string | no | Email subject |
| `text_body` | string | no | Plain text body |
| `html_body` | string | no | HTML body |
| `in_reply_to_message_id` | UUID | no | Message being replied to |

**Response (201):** The created draft.

## Get draft

```
GET /api/v1/inboxes/{inbox_id}/drafts/{id}
```

## Update draft

```
PUT /api/v1/inboxes/{inbox_id}/drafts/{id}
```

**Request body:** Same fields as create (all optional). Only provided fields are updated.

## Send draft

```
POST /api/v1/inboxes/{inbox_id}/drafts/{id}/send
```

Converts the draft to a sent message, runs it through the permission engine, and delivers it. The draft is deleted after sending.

**Response:** Same as [Send message](./messages.md#send-message) — 201 if sent, 202 if pending approval.

## Delete draft

```
DELETE /api/v1/inboxes/{inbox_id}/drafts/{id}
```

**Response (204):** No content.
