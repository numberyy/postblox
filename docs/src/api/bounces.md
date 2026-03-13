# Bounces

Bounce tracking records delivery failures for outbound messages.

## Get delivery status

```
GET /api/v1/inboxes/{inbox_id}/messages/{id}/delivery-status
```

**Response (200):** Delivery status object with bounce details if delivery failed.

## Inbound bounce webhook

```
POST /internal/stalwart/bounce
```

This is an internal endpoint called by Stalwart when a delivery attempt bounces. It is not authenticated via API key — it uses the Stalwart inbound token if configured.
