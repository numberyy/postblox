# MCP Tools

postblox-mcp is a Model Context Protocol server that exposes 44 tools for AI agents. It communicates via stdio JSON-RPC and wraps the postblox REST API.

## Setup

```bash
# Environment variables
export POSTBLOX_URL=http://localhost:3000
export POSTBLOX_API_KEY=pb_abc12.deadbeef...

# Run directly
postblox-mcp

# Or via Claude Code .mcp.json
```

See [Getting Started](./getting-started.md#mcp-setup-for-claude-code) for `.mcp.json` configuration.

## Tool reference

### Inbox management

| Tool | Description | Required params |
|------|-------------|-----------------|
| `postblox_list_inboxes` | List all inboxes | *(none)* |
| `postblox_create_inbox` | Create a new inbox | `email` |
| `postblox_get_inbox` | Get inbox by ID | `inbox_id` |
| `postblox_delete_inbox` | Delete an inbox | `inbox_id` |

**postblox_create_inbox** optional params: `display_name`, `inbox_type`

### Messages

| Tool | Description | Required params |
|------|-------------|-----------------|
| `postblox_send_email` | Send an email from an inbox | `inbox_id`, `to`, `subject` |
| `postblox_list_messages` | List messages in an inbox | `inbox_id` |
| `postblox_get_message` | Get a specific message | `inbox_id`, `message_id` |
| `postblox_reply` | Reply to a message (auto-threads) | `inbox_id`, `message_id`, `text_body` |

**postblox_send_email** optional params: `text_body`, `html_body`, `cc`

**postblox_list_messages** optional params: `limit`, `offset`, `thread_id`

**postblox_reply** optional params: `html_body`, `cc`

### Threads

| Tool | Description | Required params |
|------|-------------|-----------------|
| `postblox_list_threads` | List threads in an inbox | `inbox_id` |
| `postblox_get_thread` | Get thread with messages | `inbox_id`, `thread_id` |

Optional params: `limit`, `offset`

### Search & briefing

| Tool | Description | Required params |
|------|-------------|-----------------|
| `postblox_search` | Full-text or semantic search | `q` |
| `postblox_briefing` | Activity summary | *(none)* |

**postblox_search** optional params: `inbox_id`, `limit`, `offset`, `semantic` (boolean), `threshold` (0.0-1.0)

**postblox_briefing** optional params: `period` (`1h`, `6h`, `12h`, `24h`, `7d`)

### Webhooks

| Tool | Description | Required params |
|------|-------------|-----------------|
| `postblox_register_webhook` | Register a webhook | `url`, `events` |
| `postblox_list_webhooks` | List all webhooks | *(none)* |
| `postblox_delete_webhook` | Delete a webhook | `webhook_id` |

### Labels

| Tool | Description | Required params |
|------|-------------|-----------------|
| `postblox_list_labels` | List inbox labels | `inbox_id` |
| `postblox_create_label` | Create a label | `inbox_id`, `name` |
| `postblox_add_label_to_message` | Tag a message | `inbox_id`, `message_id`, `label_id` |
| `postblox_remove_label_from_message` | Untag a message | `inbox_id`, `message_id`, `label_id` |

**postblox_create_label** optional params: `color` (hex, e.g. `#ff0000`)

### Drafts

| Tool | Description | Required params |
|------|-------------|-----------------|
| `postblox_list_drafts` | List drafts | `inbox_id` |
| `postblox_create_draft` | Create a draft | `inbox_id` |
| `postblox_update_draft` | Update a draft | `inbox_id`, `draft_id` |
| `postblox_send_draft` | Send a draft | `inbox_id`, `draft_id` |
| `postblox_delete_draft` | Delete a draft | `inbox_id`, `draft_id` |

**postblox_create_draft** optional params: `to`, `cc`, `subject`, `text_body`, `html_body`, `in_reply_to_message_id`

**postblox_update_draft** optional params: `to`, `cc`, `subject`, `text_body`, `html_body`

### Domains

| Tool | Description | Required params |
|------|-------------|-----------------|
| `postblox_list_domains` | List all domains | *(none)* |
| `postblox_create_domain` | Add a domain | `name` |
| `postblox_get_domain` | Get domain details | `domain_id` |
| `postblox_verify_domain` | Trigger DNS verification | `domain_id` |
| `postblox_delete_domain` | Delete a domain | `domain_id` |

### Approvals

| Tool | Description | Required params |
|------|-------------|-----------------|
| `postblox_list_approvals` | List pending approvals | *(none)* |
| `postblox_get_approval` | Get approval details | `approval_id` |
| `postblox_approve_message` | Approve a message | `approval_id`, `decided_by` |
| `postblox_reject_message` | Reject a message | `approval_id`, `decided_by` |
| `postblox_batch_approvals` | Batch approve/reject | `ids`, `status`, `decided_by` |

**postblox_list_approvals** optional params: `limit`, `offset`

**postblox_batch_approvals** `status`: `approved` or `rejected`

### Permissions & trust

| Tool | Description | Required params |
|------|-------------|-----------------|
| `postblox_get_permissions` | Get inbox permissions | `inbox_id` |
| `postblox_set_permissions` | Update permissions | `inbox_id` |
| `postblox_get_trust` | Get trust level | `inbox_id` |

**postblox_set_permissions** optional params: `send_mode` (`shadow`, `approval`, `auto_approve`, `autonomous`), `rules` (JSON array)

### Audit & notifications

| Tool | Description | Required params |
|------|-------------|-----------------|
| `postblox_list_audit` | List audit entries | *(none)* |
| `postblox_list_notifications` | List notification channels | *(none)* |
| `postblox_create_notification` | Create a notification | `provider`, `config` |
| `postblox_delete_notification` | Delete a notification | `notification_id` |

**postblox_list_audit** optional params: `limit`, `offset`, `inbox_id`, `action`, `after`, `before`

### Linked inboxes

| Tool | Description | Required params |
|------|-------------|-----------------|
| `postblox_link_inbox` | Link an IMAP inbox | `inbox_id`, `imap_host`, `username`, `password` |
| `postblox_list_linked_inboxes` | List linked inboxes | *(none)* |
| `postblox_sync_linked_inbox` | Trigger IMAP sync | `linked_account_id` |

**postblox_link_inbox** optional params: `imap_port` (default 993)
