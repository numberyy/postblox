# Authentication

All `/api/v1/*` endpoints require a Bearer token in the `Authorization` header.

```
Authorization: Bearer pb_abc12.deadbeefdeadbeefdeadbeef...
```

## API key format

API keys follow the format `pb_<prefix>.<random>`:
- Prefix: first 8 characters, used for fast lookup
- The full key is hashed (SHA-256) before storage — it cannot be recovered

API keys are scoped to an organization. All resources accessed through a key belong to that organization.

## Getting an API key

API keys are created in two ways:

1. **Bootstrap** — `POST /api/v1/organizations` returns the first API key for a new organization
2. **Create** — `POST /api/v1/api-keys` creates additional keys (requires an existing key)

## WebSocket authentication

The WebSocket endpoint uses a query parameter instead of a header:

```
ws://localhost:3000/api/v1/ws?key=pb_abc12.deadbeef...
```
