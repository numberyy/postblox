# Linked Accounts

Linked accounts connect external IMAP inboxes to postblox for one-shot pull sync.

## List linked accounts

```
GET /api/v1/linked-accounts
```

**Response (200):** Array of linked account objects (credentials are redacted).

## Create linked account

```
POST /api/v1/linked-accounts
```

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `inbox_id` | UUID | yes | postblox inbox to sync into |
| `imap_host` | string | yes | IMAP server hostname |
| `imap_port` | integer | no | IMAP port (default 993) |
| `username` | string | yes | IMAP username |
| `password` | string | yes | IMAP password |

**Response (201):** The created linked account.

## Get linked account

```
GET /api/v1/linked-accounts/{id}
```

## Delete linked account

```
DELETE /api/v1/linked-accounts/{id}
```

**Response (204):** No content.

## Trigger sync

```
POST /api/v1/linked-accounts/{id}/sync
```

Triggers an IMAP pull sync for the linked account. Messages are fetched, parsed, threaded, and stored in the target inbox.

**Response (200):** Sync result with message count.
