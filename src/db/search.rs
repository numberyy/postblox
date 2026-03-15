use sqlx::PgPool;
use uuid::Uuid;

use crate::models::Message;

const SELECT_COLS_PREFIXED: &str = "\
    m.id, m.inbox_id, m.thread_id, m.message_id_header, m.in_reply_to, \
    m.references_header, m.from_addr, m.to_addrs, m.cc_addrs, m.subject, m.text_body, m.html_body, \
    m.extracted_text, m.direction, m.raw_headers, m.created_at, \
    m.slop_score, m.slop_signals, m.category, m.priority, m.triage_status, m.requires_action";

#[derive(Debug, Default)]
pub struct ParsedSearch {
    pub free_text: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub subject: Option<String>,
    pub inbox_name: Option<String>,
    pub has_attachment: bool,
}

impl ParsedSearch {
    pub fn has_fields(&self) -> bool {
        self.from.is_some()
            || self.to.is_some()
            || self.subject.is_some()
            || self.inbox_name.is_some()
            || self.has_attachment
    }
}

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

pub fn parse_search_query(q: &str) -> ParsedSearch {
    let mut parsed = ParsedSearch::default();
    let mut free_parts: Vec<&str> = Vec::new();

    for token in q.split_whitespace() {
        if let Some(rest) = token.strip_prefix('@') {
            if let Some((key, value)) = rest.split_once(':') {
                match key.to_ascii_lowercase().as_str() {
                    "from" => parsed.from = Some(value.to_string()),
                    "to" => parsed.to = Some(value.to_string()),
                    "subject" => parsed.subject = Some(value.to_string()),
                    "in" => parsed.inbox_name = Some(value.to_string()),
                    "has" if value.eq_ignore_ascii_case("attachment") => {
                        parsed.has_attachment = true;
                    }
                    _ => free_parts.push(token),
                }
            } else {
                free_parts.push(token);
            }
        } else {
            free_parts.push(token);
        }
    }

    parsed.free_text = free_parts.join(" ");
    parsed
}

pub async fn field_search(
    pool: &PgPool,
    org_id: Uuid,
    parsed: &ParsedSearch,
    inbox_id: Option<Uuid>,
    limit: i64,
    offset: i64,
) -> Result<Vec<Message>, sqlx::Error> {
    let mut sql = format!(
        "SELECT {SELECT_COLS_PREFIXED} FROM messages m \
         JOIN inboxes i ON i.id = m.inbox_id \
         WHERE i.org_id = $1"
    );
    let mut param_idx: i32 = 1;
    let mut str_binds: Vec<String> = Vec::new();
    let mut uuid_bind: Option<Uuid> = None;
    let has_free_text = !parsed.free_text.is_empty();
    let mut free_text_idx: Option<i32> = None;

    if has_free_text {
        param_idx += 1;
        free_text_idx = Some(param_idx);
        sql.push_str(&format!(
            " AND m.search_vector @@ plainto_tsquery('english', ${param_idx})"
        ));
        str_binds.push(parsed.free_text.clone());
    }

    if let Some(from) = &parsed.from {
        param_idx += 1;
        sql.push_str(&format!(" AND m.from_addr ILIKE ${param_idx}"));
        str_binds.push(format!("%{}%", escape_like(from)));
    }

    if let Some(to) = &parsed.to {
        param_idx += 1;
        sql.push_str(&format!(" AND m.to_addrs::text ILIKE ${param_idx}"));
        str_binds.push(format!("%{}%", escape_like(to)));
    }

    if let Some(subject) = &parsed.subject {
        param_idx += 1;
        sql.push_str(&format!(" AND m.subject ILIKE ${param_idx}"));
        str_binds.push(format!("%{}%", escape_like(subject)));
    }

    if let Some(inbox_name) = &parsed.inbox_name {
        param_idx += 1;
        sql.push_str(&format!(
            " AND (i.name ILIKE ${param_idx} OR i.email ILIKE ${param_idx})"
        ));
        str_binds.push(format!("%{}%", escape_like(inbox_name)));
    }

    if parsed.has_attachment {
        sql.push_str(" AND EXISTS (SELECT 1 FROM attachments a WHERE a.message_id = m.id)");
    }

    if let Some(iid) = inbox_id {
        param_idx += 1;
        sql.push_str(&format!(" AND m.inbox_id = ${param_idx}"));
        uuid_bind = Some(iid);
    }

    if let Some(ft_idx) = free_text_idx {
        sql.push_str(&format!(
            " ORDER BY ts_rank(m.search_vector, plainto_tsquery('english', ${ft_idx})) DESC, m.created_at DESC"
        ));
    } else {
        sql.push_str(" ORDER BY m.created_at DESC");
    }

    param_idx += 1;
    sql.push_str(&format!(" LIMIT ${param_idx}"));
    param_idx += 1;
    sql.push_str(&format!(" OFFSET ${param_idx}"));

    let mut query = sqlx::query_as::<_, Message>(&sql).bind(org_id);

    for val in &str_binds {
        query = query.bind(val);
    }

    if let Some(iid) = uuid_bind {
        query = query.bind(iid);
    }

    query = query.bind(limit).bind(offset);

    query.fetch_all(pool).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty() {
        let p = parse_search_query("");
        assert!(p.free_text.is_empty());
        assert!(p.from.is_none());
        assert!(p.to.is_none());
        assert!(p.subject.is_none());
        assert!(p.inbox_name.is_none());
        assert!(!p.has_attachment);
        assert!(!p.has_fields());
    }

    #[test]
    fn test_parse_free_text_only() {
        let p = parse_search_query("invoice");
        assert_eq!(p.free_text, "invoice");
        assert!(!p.has_fields());
    }

    #[test]
    fn test_parse_from_filter() {
        let p = parse_search_query("@from:alice invoice");
        assert_eq!(p.from.as_deref(), Some("alice"));
        assert_eq!(p.free_text, "invoice");
        assert!(p.has_fields());
    }

    #[test]
    fn test_parse_multiple_filters() {
        let p = parse_search_query("@from:alice @subject:deploy");
        assert_eq!(p.from.as_deref(), Some("alice"));
        assert_eq!(p.subject.as_deref(), Some("deploy"));
        assert!(p.free_text.is_empty());
    }

    #[test]
    fn test_parse_has_attachment() {
        let p = parse_search_query("@has:attachment docs");
        assert!(p.has_attachment);
        assert_eq!(p.free_text, "docs");
    }

    #[test]
    fn test_parse_inbox_filter() {
        let p = parse_search_query("@in:dev");
        assert_eq!(p.inbox_name.as_deref(), Some("dev"));
        assert!(p.free_text.is_empty());
    }

    #[test]
    fn test_parse_mixed() {
        let p = parse_search_query("@from:alice @to:bob budget");
        assert_eq!(p.from.as_deref(), Some("alice"));
        assert_eq!(p.to.as_deref(), Some("bob"));
        assert_eq!(p.free_text, "budget");
    }

    #[test]
    fn test_parse_unknown_prefix_kept_as_text() {
        let p = parse_search_query("@unknown:val other");
        assert_eq!(p.free_text, "@unknown:val other");
        assert!(!p.has_fields());
    }

    #[test]
    fn test_parse_case_insensitive_prefix() {
        let p = parse_search_query("@FROM:alice");
        assert_eq!(p.from.as_deref(), Some("alice"));
    }

    #[test]
    fn test_parse_bare_at_kept_as_text() {
        let p = parse_search_query("@nocolon");
        assert_eq!(p.free_text, "@nocolon");
    }

    #[test]
    fn test_parse_has_attachment_case_insensitive() {
        let p = parse_search_query("@has:Attachment");
        assert!(p.has_attachment);
    }

    #[test]
    fn test_parse_has_nonattachment_kept_as_text() {
        let p = parse_search_query("@has:something");
        assert!(!p.has_attachment);
        assert_eq!(p.free_text, "@has:something");
    }

    #[test]
    fn test_parse_colon_in_value() {
        // split_once(':') ensures full email is captured
        let p = parse_search_query("@from:alice@example.com");
        assert_eq!(p.from.as_deref(), Some("alice@example.com"));

        // actual colon in value: only first colon splits key from value
        let p2 = parse_search_query("@subject:re:hello");
        assert_eq!(p2.subject.as_deref(), Some("re:hello"));
    }

    #[test]
    fn test_parse_all_fields() {
        let p = parse_search_query(
            "@from:alice @to:bob @subject:budget @in:dev @has:attachment report",
        );
        assert_eq!(p.from.as_deref(), Some("alice"));
        assert_eq!(p.to.as_deref(), Some("bob"));
        assert_eq!(p.subject.as_deref(), Some("budget"));
        assert_eq!(p.inbox_name.as_deref(), Some("dev"));
        assert!(p.has_attachment);
        assert_eq!(p.free_text, "report");
    }

    #[test]
    fn test_escape_like_metacharacters() {
        assert_eq!(escape_like("100%"), "100\\%");
        assert_eq!(escape_like("a_b"), "a\\_b");
        assert_eq!(escape_like("no\\slash"), "no\\\\slash");
        assert_eq!(escape_like("normal"), "normal");
    }
}
