# API Reference

All API endpoints are under `/api/v1/` and require Bearer token authentication (except bootstrap and health).

Base URL: `http://localhost:3000`

## Common patterns

**Authentication**: Every request to `/api/v1/*` must include:
```
Authorization: Bearer pb_abc12.deadbeef...
```

**Pagination**: List endpoints accept `limit` (1-100, default 50) and `offset` (default 0) query parameters.

**Error responses**: All errors return JSON:
```json
{"error": "description of what went wrong"}
```

| Status | Meaning |
|--------|---------|
| 400 | Bad request (validation error) |
| 401 | Missing or invalid API key |
| 403 | Forbidden (permission denied, guard blocked, hook blocked) |
| 404 | Resource not found |
| 409 | Conflict (duplicate entry) |
| 429 | Rate limit exceeded |
| 500 | Internal server error |

**Rate limiting**: All authenticated API requests include rate limit headers:

| Header | Description |
|--------|-------------|
| `X-RateLimit-Limit` | Max requests per window |
| `X-RateLimit-Remaining` | Remaining requests in current window |
| `X-RateLimit-Reset` | Seconds until the window resets |
| `Retry-After` | Seconds to wait (only on 429 responses) |

Defaults: 60 requests/minute, 1000 requests/hour. Keyed by API key hash. Configurable via `[rate_limit]` in TOML config.

## Non-authenticated endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | Health check (returns `{"status":"ok","database":true}`) |
| POST | `/internal/stalwart/inbound` | Inbound email webhook from Stalwart |
| POST | `/internal/stalwart/bounce` | Bounce notification from Stalwart |
| GET | `/api/v1/ws?key=<api_key>` | WebSocket connection |
