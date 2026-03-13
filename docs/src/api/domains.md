# Domains

Register and verify domains for email sending. Domain verification checks DNS records (SPF, DKIM, DMARC) configured in Stalwart.

## List domains

```
GET /api/v1/domains
```

**Response (200):** Array of domain objects.

## Create domain

```
POST /api/v1/domains
```

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Domain name (e.g. `example.com`) |

**Response (201):** The created domain with DNS records to configure.

## Get domain

```
GET /api/v1/domains/{id}
```

**Response (200):** Domain details including verification status and DNS records.

## Verify domain

```
POST /api/v1/domains/{id}/verify
```

Triggers DNS verification. Updates the domain's verified status based on current DNS records.

**Response (200):** Updated domain object with verification results.

## Delete domain

```
DELETE /api/v1/domains/{id}
```

**Response (204):** No content.
