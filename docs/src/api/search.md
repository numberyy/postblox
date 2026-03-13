# Search

Search across messages using full-text or semantic (vector similarity) search.

## Search messages

```
GET /api/v1/search
```

**Query parameters:**

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `q` | string | *(required)* | Search query |
| `inbox_id` | UUID | *(none)* | Limit search to a specific inbox |
| `limit` | integer | 50 | Max results (1-100) |
| `offset` | integer | 0 | Pagination offset |
| `semantic` | boolean | false | Use semantic search instead of full-text |
| `threshold` | float | 0.7 | Minimum similarity threshold for semantic search (0.0-1.0) |

**Full-text search** (default): PostgreSQL `to_tsvector`/`to_tsquery` against message content. Works out of the box.

**Semantic search** (`semantic=true`): Embeds the query via the configured embedding provider and searches by vector similarity (cosine distance) using pgvector HNSW index. Requires `embedding_url` to be configured.

**Response (200):** Array of message objects matching the query.

```bash
# Full-text search
curl "http://localhost:3000/api/v1/search?q=invoice" \
  -H "Authorization: Bearer $API_KEY"

# Semantic search
curl "http://localhost:3000/api/v1/search?q=payment+related+emails&semantic=true&threshold=0.6" \
  -H "Authorization: Bearer $API_KEY"
```
