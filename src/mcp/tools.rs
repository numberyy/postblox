//! Built-in MCP tool catalogue and JSON-schema validation.

use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::daemon::Op;
use crate::db::sql_query::MAX_ROWS;

const SEARCH_MAX_ROWS: i64 = 200;

/// Static description of one MCP tool exposed by the bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolSpec {
    /// Public tool name advertised over JSON-RPC.
    pub name: &'static str,
    /// Daemon [`Op`] invoked when the tool is called.
    pub op: Op,
    /// Human-readable description surfaced in `tools/list`.
    pub description: &'static str,
    /// Whether the tool mutates state and therefore goes through gates.
    pub dangerous: bool,
    required: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldKind {
    String,
    NullableString,
    UuidString,
    NullableUuidString,
    NullableRfc3339,
    StringArray,
    Boolean,
    Integer { minimum: i64, maximum: i64 },
}

/// Catalogue of every MCP tool the bridge currently exposes.
pub const TOOLS: [ToolSpec; 14] = [
    ToolSpec {
        name: "postblox_account_list",
        op: Op::AccountList,
        description: "List configured postblox email accounts.",
        dangerous: false,
        required: &[],
    },
    ToolSpec {
        name: "postblox_folder_list",
        op: Op::FolderList,
        description: "List folders for one account.",
        dangerous: false,
        required: &["account_id"],
    },
    ToolSpec {
        name: "postblox_thread_list",
        op: Op::ThreadList,
        description: "List recent threads for one account.",
        dangerous: false,
        required: &["account_id"],
    },
    ToolSpec {
        name: "postblox_message_list_by_folder",
        op: Op::MessageListByFolder,
        description: "List message summary rows in a folder (no text_body/html_body).",
        dangerous: false,
        required: &["folder_id"],
    },
    ToolSpec {
        name: "postblox_message_list_by_thread",
        op: Op::MessageListByThread,
        description: "List message summary rows in a thread (no text_body/html_body).",
        dangerous: false,
        required: &["thread_id"],
    },
    ToolSpec {
        name: "postblox_message_get",
        op: Op::MessageGet,
        description: "Fetch one full message by id, including text_body/html_body.",
        dangerous: false,
        required: &["id"],
    },
    ToolSpec {
        name: "postblox_search",
        op: Op::Search,
        description: "Search local indexed email and return summary rows (no text_body/html_body) with optional account/folder/thread/date/sender/flag filters.",
        dangerous: false,
        required: &["q"],
    },
    ToolSpec {
        name: "postblox_draft_create",
        op: Op::DraftCreate,
        description: "Create an email draft.",
        dangerous: true,
        required: &["account_id", "to_addrs", "cc_addrs", "bcc_addrs"],
    },
    ToolSpec {
        name: "postblox_draft_update",
        op: Op::DraftUpdate,
        description: "Update an email draft.",
        dangerous: true,
        required: &[
            "id",
            "to_addrs",
            "cc_addrs",
            "bcc_addrs",
            "subject",
            "text_body",
            "html_body",
        ],
    },
    ToolSpec {
        name: "postblox_draft_delete",
        op: Op::DraftDelete,
        description: "Delete an email draft.",
        dangerous: true,
        required: &["id"],
    },
    ToolSpec {
        name: "postblox_message_set_flags",
        op: Op::MessageSetFlags,
        description: "Replace flags on one message.",
        dangerous: true,
        required: &["id", "flags"],
    },
    ToolSpec {
        name: "postblox_message_send",
        op: Op::MessageSend,
        description: "Send an existing draft through the account SMTP settings.",
        dangerous: true,
        required: &["account_id", "draft_id"],
    },
    ToolSpec {
        name: "postblox_sql_query",
        op: Op::SqlQuery,
        description: "Run a read-only SQL query against the local postblox database. \
                      Only SELECT statements are accepted; DDL and DML are rejected. \
                      Returns up to `limit` rows as JSON objects.",
        dangerous: true,
        required: &["sql"],
    },
    ToolSpec {
        name: "postblox_sql_schema",
        op: Op::SqlSchema,
        description: "Return CREATE statements for every table, view, index, and \
                      trigger in the postblox database. Use this before issuing \
                      ad-hoc queries via postblox_sql_query.",
        dangerous: false,
        required: &[],
    },
];

/// Locate a tool by its public name.
pub fn find_tool(name: &str) -> Option<&'static ToolSpec> {
    TOOLS.iter().find(|tool| tool.name == name)
}

/// Return the JSON-RPC `tools/list` payload describing every tool.
pub fn list_tools() -> Value {
    json!({
        "tools": TOOLS.iter().map(tool_json).collect::<Vec<_>>(),
    })
}

/// Validate `arguments` against the input schema of `tool`, returning the
/// arguments unchanged on success.
///
/// # Errors
///
/// Returns a human-readable error message (intended to be wrapped in
/// [`JsonRpcError::invalid_params`](super::protocol::JsonRpcError::invalid_params))
/// when:
/// - `arguments` is neither `null` nor a JSON object;
/// - a field declared in `tool.required` is absent;
/// - an unknown field is present (no schema entry for `(tool, field)`);
/// - a field's value does not match its declared kind — wrong primitive
///   type, malformed UUID, malformed RFC-3339 timestamp, non-string array
///   element, or integer outside the schema's `[minimum, maximum]` range.
pub fn validate_arguments(tool: &ToolSpec, arguments: Value) -> Result<Value, String> {
    let object = match arguments {
        Value::Null => Map::new(),
        Value::Object(object) => object,
        _ => return Err("arguments must be an object".into()),
    };

    for field in tool.required {
        if !object.contains_key(*field) {
            return Err(format!("missing required argument '{field}'"));
        }
    }

    for (field, value) in &object {
        let kind =
            field_kind(tool, field).ok_or_else(|| format!("unexpected argument '{field}'"))?;
        validate_field(field, value, kind)?;
    }

    Ok(Value::Object(object))
}

fn field_kind(tool: &ToolSpec, field: &str) -> Option<FieldKind> {
    match (tool.name, field) {
        ("postblox_folder_list", "account_id")
        | ("postblox_thread_list", "account_id")
        | ("postblox_message_list_by_folder", "folder_id")
        | ("postblox_message_list_by_thread", "thread_id")
        | ("postblox_message_get", "id")
        | ("postblox_draft_create", "account_id")
        | ("postblox_draft_update", "id")
        | ("postblox_draft_delete", "id")
        | ("postblox_message_set_flags", "id")
        | ("postblox_message_send", "account_id")
        | ("postblox_message_send", "draft_id") => Some(FieldKind::UuidString),
        ("postblox_draft_create", "in_reply_to_msg") => Some(FieldKind::NullableUuidString),
        ("postblox_search", "q") => Some(FieldKind::String),
        ("postblox_search", "account_id")
        | ("postblox_search", "folder_id")
        | ("postblox_search", "thread_id") => Some(FieldKind::NullableUuidString),
        ("postblox_search", "date_from") | ("postblox_search", "date_to") => {
            Some(FieldKind::NullableRfc3339)
        }
        ("postblox_search", "from_addr") | ("postblox_search", "to_addr") => {
            Some(FieldKind::NullableString)
        }
        ("postblox_search", "has_attachments") | ("postblox_search", "unread") => {
            Some(FieldKind::Boolean)
        }
        ("postblox_draft_create", "to_addrs")
        | ("postblox_draft_create", "cc_addrs")
        | ("postblox_draft_create", "bcc_addrs")
        | ("postblox_draft_update", "to_addrs")
        | ("postblox_draft_update", "cc_addrs")
        | ("postblox_draft_update", "bcc_addrs")
        | ("postblox_message_set_flags", "flags") => Some(FieldKind::StringArray),
        ("postblox_draft_create", "subject")
        | ("postblox_draft_create", "text_body")
        | ("postblox_draft_create", "html_body")
        | ("postblox_draft_update", "subject")
        | ("postblox_draft_update", "text_body")
        | ("postblox_draft_update", "html_body") => Some(FieldKind::NullableString),
        ("postblox_thread_list", "limit") | ("postblox_message_list_by_folder", "limit") => {
            Some(FieldKind::Integer {
                minimum: 1,
                maximum: 500,
            })
        }
        ("postblox_search", "limit") => Some(FieldKind::Integer {
            minimum: 1,
            maximum: SEARCH_MAX_ROWS,
        }),
        ("postblox_thread_list", "offset")
        | ("postblox_message_list_by_folder", "offset")
        | ("postblox_search", "offset") => Some(FieldKind::Integer {
            minimum: 0,
            maximum: i64::MAX,
        }),
        ("postblox_sql_query", "sql") => Some(FieldKind::String),
        ("postblox_sql_query", "limit") => Some(FieldKind::Integer {
            minimum: 1,
            maximum: MAX_ROWS as i64,
        }),
        _ => None,
    }
}

fn validate_field(field: &str, value: &Value, kind: FieldKind) -> Result<(), String> {
    match kind {
        FieldKind::String => {
            if value.is_string() {
                Ok(())
            } else {
                Err(format!("{field} must be a string"))
            }
        }
        FieldKind::NullableString => {
            if value.is_string() || value.is_null() {
                Ok(())
            } else {
                Err(format!("{field} must be a string or null"))
            }
        }
        FieldKind::UuidString => {
            let value = value
                .as_str()
                .ok_or_else(|| format!("{field} must be a string"))?;
            Uuid::parse_str(value)
                .map(|_| ())
                .map_err(|_| format!("{field} must be a uuid string"))
        }
        FieldKind::NullableUuidString => {
            if value.is_null() {
                return Ok(());
            }
            let value = value
                .as_str()
                .ok_or_else(|| format!("{field} must be a string or null"))?;
            Uuid::parse_str(value)
                .map(|_| ())
                .map_err(|_| format!("{field} must be a uuid string or null"))
        }
        FieldKind::NullableRfc3339 => {
            if value.is_null() {
                return Ok(());
            }
            let s = value
                .as_str()
                .ok_or_else(|| format!("{field} must be an rfc3339 string or null"))?;
            chrono::DateTime::parse_from_rfc3339(s)
                .map(|_| ())
                .map_err(|_| format!("{field} must be an rfc3339 string or null"))
        }
        FieldKind::StringArray => {
            let values = value
                .as_array()
                .ok_or_else(|| format!("{field} must be an array of strings"))?;
            if values.iter().all(Value::is_string) {
                Ok(())
            } else {
                Err(format!("{field} must be an array of strings"))
            }
        }
        FieldKind::Boolean => {
            if value.is_boolean() {
                Ok(())
            } else {
                Err(format!("{field} must be a boolean"))
            }
        }
        FieldKind::Integer { minimum, maximum } => {
            let value = value
                .as_i64()
                .ok_or_else(|| format!("{field} must be an integer"))?;
            if value < minimum || value > maximum {
                Err(format!("{field} must be between {minimum} and {maximum}"))
            } else {
                Ok(())
            }
        }
    }
}

fn tool_json(tool: &ToolSpec) -> Value {
    json!({
        "name": tool.name,
        "description": tool.description,
        "inputSchema": input_schema(tool),
    })
}

fn input_schema(tool: &ToolSpec) -> Value {
    let properties = match tool.name {
        "postblox_account_list" => Map::new(),
        "postblox_folder_list" => fields(&[uuid("account_id", "Account id.")]),
        "postblox_thread_list" => fields(&[
            uuid("account_id", "Account id."),
            integer("limit", 1, 500, "Maximum rows to return."),
            integer("offset", 0, i64::MAX, "Rows to skip."),
        ]),
        "postblox_message_list_by_folder" => fields(&[
            uuid("folder_id", "Folder id."),
            integer("limit", 1, 500, "Maximum rows to return."),
            integer("offset", 0, i64::MAX, "Rows to skip."),
        ]),
        "postblox_message_list_by_thread" => fields(&[uuid("thread_id", "Thread id.")]),
        "postblox_message_get" => fields(&[uuid("id", "Message id.")]),
        "postblox_search" => fields(&[
            string("q", "Search query."),
            nullable_uuid("account_id", "Restrict to one account."),
            nullable_uuid("folder_id", "Restrict to one folder."),
            nullable_uuid("thread_id", "Restrict to one thread."),
            nullable_rfc3339("date_from", "Earliest internal date (inclusive)."),
            nullable_rfc3339("date_to", "Latest internal date (inclusive)."),
            nullable_string("from_addr", "Substring match against the sender."),
            nullable_string("to_addr", "Substring match against the recipients."),
            boolean("has_attachments", "Filter by presence of attachments."),
            boolean("unread", "Filter by read state (true = unread)."),
            integer("limit", 1, SEARCH_MAX_ROWS, "Maximum rows to return."),
            integer("offset", 0, i64::MAX, "Rows to skip."),
        ]),
        "postblox_draft_create" => fields(&[
            uuid("account_id", "Account id."),
            nullable_uuid("in_reply_to_msg", "Message id this draft replies to."),
            string_array("to_addrs", "To recipients."),
            string_array("cc_addrs", "Cc recipients."),
            string_array("bcc_addrs", "Bcc recipients."),
            nullable_string("subject", "Draft subject."),
            nullable_string("text_body", "Plain text body."),
            nullable_string("html_body", "HTML body."),
        ]),
        "postblox_draft_update" => fields(&[
            uuid("id", "Draft id."),
            string_array("to_addrs", "To recipients."),
            string_array("cc_addrs", "Cc recipients."),
            string_array("bcc_addrs", "Bcc recipients."),
            nullable_string("subject", "Draft subject."),
            nullable_string("text_body", "Plain text body."),
            nullable_string("html_body", "HTML body."),
        ]),
        "postblox_draft_delete" => fields(&[uuid("id", "Draft id.")]),
        "postblox_message_set_flags" => fields(&[
            uuid("id", "Message id."),
            string_array("flags", "Complete replacement flag list."),
        ]),
        "postblox_message_send" => fields(&[
            uuid("account_id", "Account id."),
            uuid("draft_id", "Draft id."),
        ]),
        "postblox_sql_query" => fields(&[
            string("sql", "Read-only SQL statement to execute."),
            integer(
                "limit",
                1,
                MAX_ROWS as i64,
                "Maximum rows to return; omitted limits use the daemon default.",
            ),
        ]),
        "postblox_sql_schema" => Map::new(),
        _ => Map::new(),
    };

    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": properties,
        "required": tool.required,
    })
}

fn fields(fields: &[(&str, Value)]) -> Map<String, Value> {
    fields
        .iter()
        .map(|(name, schema)| ((*name).to_string(), schema.clone()))
        .collect()
}

fn string(name: &'static str, description: &'static str) -> (&'static str, Value) {
    (
        name,
        json!({ "type": "string", "description": description }),
    )
}

fn nullable_string(name: &'static str, description: &'static str) -> (&'static str, Value) {
    (
        name,
        json!({ "type": ["string", "null"], "description": description }),
    )
}

fn uuid(name: &'static str, description: &'static str) -> (&'static str, Value) {
    (
        name,
        json!({ "type": "string", "format": "uuid", "description": description }),
    )
}

fn nullable_uuid(name: &'static str, description: &'static str) -> (&'static str, Value) {
    (
        name,
        json!({ "type": ["string", "null"], "format": "uuid", "description": description }),
    )
}

fn nullable_rfc3339(name: &'static str, description: &'static str) -> (&'static str, Value) {
    (
        name,
        json!({ "type": ["string", "null"], "format": "date-time", "description": description }),
    )
}

fn boolean(name: &'static str, description: &'static str) -> (&'static str, Value) {
    (
        name,
        json!({ "type": "boolean", "description": description }),
    )
}

fn string_array(name: &'static str, description: &'static str) -> (&'static str, Value) {
    (
        name,
        json!({
            "type": "array",
            "items": { "type": "string" },
            "description": description
        }),
    )
}

fn integer(
    name: &'static str,
    minimum: i64,
    maximum: i64,
    description: &'static str,
) -> (&'static str, Value) {
    (
        name,
        json!({
            "type": "integer",
            "minimum": minimum,
            "maximum": maximum,
            "description": description
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_list_has_exactly_fourteen_stable_names() {
        let names = TOOLS.iter().map(|tool| tool.name).collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "postblox_account_list",
                "postblox_folder_list",
                "postblox_thread_list",
                "postblox_message_list_by_folder",
                "postblox_message_list_by_thread",
                "postblox_message_get",
                "postblox_search",
                "postblox_draft_create",
                "postblox_draft_update",
                "postblox_draft_delete",
                "postblox_message_set_flags",
                "postblox_message_send",
                "postblox_sql_query",
                "postblox_sql_schema",
            ]
        );

        let ops = TOOLS.iter().map(|tool| tool.op).collect::<Vec<_>>();
        assert_eq!(
            ops,
            vec![
                Op::AccountList,
                Op::FolderList,
                Op::ThreadList,
                Op::MessageListByFolder,
                Op::MessageListByThread,
                Op::MessageGet,
                Op::Search,
                Op::DraftCreate,
                Op::DraftUpdate,
                Op::DraftDelete,
                Op::MessageSetFlags,
                Op::MessageSend,
                Op::SqlQuery,
                Op::SqlSchema,
            ]
        );
    }

    #[test]
    fn test_list_tools_returns_json_schemas_without_extra_tools() {
        let listed = list_tools();
        let tools = listed["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 14);
        assert_eq!(tools[0]["name"], "postblox_account_list");
        assert_eq!(tools[0]["inputSchema"]["type"], "object");
        assert_eq!(
            tools[8]["inputSchema"]["required"],
            json!([
                "id",
                "to_addrs",
                "cc_addrs",
                "bcc_addrs",
                "subject",
                "text_body",
                "html_body"
            ])
        );
        assert_eq!(
            tools[11]["inputSchema"]["required"],
            json!(["account_id", "draft_id"])
        );
        assert_eq!(tools[6]["name"], "postblox_search");
        assert_eq!(
            tools[6]["inputSchema"]["properties"]["limit"]["maximum"],
            json!(SEARCH_MAX_ROWS)
        );
        assert_eq!(tools[12]["name"], "postblox_sql_query");
        assert_eq!(tools[12]["inputSchema"]["required"], json!(["sql"]));
        assert_eq!(
            tools[12]["inputSchema"]["properties"]["limit"]["maximum"],
            json!(MAX_ROWS as i64)
        );
        assert_eq!(tools[13]["name"], "postblox_sql_schema");
        assert_eq!(tools[13]["inputSchema"]["required"], json!([]));
    }

    #[test]
    fn test_validate_arguments_requires_object() {
        let tool = find_tool("postblox_message_get").unwrap();
        let err = validate_arguments(tool, json!("nope")).unwrap_err();
        assert_eq!(err, "arguments must be an object");
    }

    #[test]
    fn test_validate_arguments_rejects_missing_required_field() {
        let tool = find_tool("postblox_message_get").unwrap();
        let err = validate_arguments(tool, json!({})).unwrap_err();
        assert_eq!(err, "missing required argument 'id'");
    }

    #[test]
    fn test_validate_arguments_rejects_unknown_field() {
        let tool = find_tool("postblox_message_get").unwrap();
        let err = validate_arguments(
            tool,
            json!({
                "id": "00000000-0000-0000-0000-000000000001",
                "extra": true
            }),
        )
        .unwrap_err();
        assert_eq!(err, "unexpected argument 'extra'");
    }

    #[test]
    fn test_validate_arguments_rejects_wrong_primitive_type() {
        let tool = find_tool("postblox_folder_list").unwrap();
        let err = validate_arguments(tool, json!({"account_id": 7})).unwrap_err();
        assert_eq!(err, "account_id must be a string");
    }

    #[test]
    fn test_validate_arguments_rejects_malformed_uuid_string() {
        let tool = find_tool("postblox_message_get").unwrap();
        let err = validate_arguments(tool, json!({"id": "not-a-uuid"})).unwrap_err();
        assert_eq!(err, "id must be a uuid string");
    }

    #[test]
    fn test_validate_arguments_rejects_malformed_nullable_uuid_string() {
        let tool = find_tool("postblox_search").unwrap();
        let err =
            validate_arguments(tool, json!({"q": "mail", "account_id": "not-a-uuid"})).unwrap_err();
        assert_eq!(err, "account_id must be a uuid string or null");
    }

    #[test]
    fn test_validate_arguments_accepts_nullable_uuid_null() {
        let tool = find_tool("postblox_search").unwrap();
        let args = validate_arguments(tool, json!({"q": "mail", "account_id": null})).unwrap();
        assert!(args["account_id"].is_null());
    }

    #[test]
    fn test_validate_arguments_rejects_bad_integer_bounds() {
        let tool = find_tool("postblox_search").unwrap();
        let err = validate_arguments(tool, json!({"q": "mail", "limit": 0})).unwrap_err();
        assert_eq!(err, "limit must be between 1 and 200");
    }

    #[test]
    fn test_validate_arguments_accepts_search_limit_at_daemon_cap() {
        let tool = find_tool("postblox_search").unwrap();
        let args =
            validate_arguments(tool, json!({"q": "mail", "limit": SEARCH_MAX_ROWS})).unwrap();
        assert_eq!(args["limit"], SEARCH_MAX_ROWS);
    }

    #[test]
    fn test_validate_arguments_rejects_search_limit_over_daemon_cap() {
        let tool = find_tool("postblox_search").unwrap();
        let err = validate_arguments(tool, json!({"q": "mail", "limit": SEARCH_MAX_ROWS + 1}))
            .unwrap_err();
        assert_eq!(
            err,
            format!("limit must be between 1 and {SEARCH_MAX_ROWS}")
        );
    }

    #[test]
    fn test_validate_arguments_rejects_non_string_array_values() {
        let tool = find_tool("postblox_message_set_flags").unwrap();
        let err = validate_arguments(
            tool,
            json!({
                "id": "00000000-0000-0000-0000-000000000001",
                "flags": "\\Seen"
            }),
        )
        .unwrap_err();
        assert_eq!(err, "flags must be an array of strings");

        let err = validate_arguments(
            tool,
            json!({
                "id": "00000000-0000-0000-0000-000000000001",
                "flags": ["\\Seen", 1]
            }),
        )
        .unwrap_err();
        assert_eq!(err, "flags must be an array of strings");
    }

    #[test]
    fn test_validate_arguments_rejects_partial_draft_update() {
        let tool = find_tool("postblox_draft_update").unwrap();
        let err = validate_arguments(tool, json!({"id": "00000000-0000-0000-0000-000000000001"}))
            .unwrap_err();
        assert_eq!(err, "missing required argument 'to_addrs'");
    }

    #[test]
    fn test_validate_arguments_accepts_required_fields() {
        let tool = find_tool("postblox_message_get").unwrap();
        let args = validate_arguments(tool, json!({"id": "00000000-0000-0000-0000-000000000001"}))
            .unwrap();
        assert_eq!(args["id"], "00000000-0000-0000-0000-000000000001");
    }

    #[test]
    fn test_validate_arguments_accepts_full_draft_update() {
        let tool = find_tool("postblox_draft_update").unwrap();
        let args = validate_arguments(
            tool,
            json!({
                "id": "00000000-0000-0000-0000-000000000001",
                "to_addrs": ["to@example.com"],
                "cc_addrs": [],
                "bcc_addrs": [],
                "subject": null,
                "text_body": "body",
                "html_body": null
            }),
        )
        .unwrap();
        assert_eq!(args["to_addrs"], json!(["to@example.com"]));
    }

    #[test]
    fn test_validate_arguments_accepts_sql_query_limit() {
        let tool = find_tool("postblox_sql_query").unwrap();
        let args =
            validate_arguments(tool, json!({"sql": "SELECT 1", "limit": MAX_ROWS as i64})).unwrap();
        assert_eq!(args["sql"], "SELECT 1");
        assert_eq!(args["limit"], MAX_ROWS as i64);
    }

    #[test]
    fn test_validate_arguments_rejects_sql_query_limit_over_max() {
        let tool = find_tool("postblox_sql_query").unwrap();
        let err = validate_arguments(
            tool,
            json!({"sql": "SELECT 1", "limit": (MAX_ROWS + 1) as i64}),
        )
        .unwrap_err();
        assert_eq!(err, format!("limit must be between 1 and {MAX_ROWS}"));
    }

    #[test]
    fn test_validate_arguments_accepts_sql_schema_without_args() {
        let tool = find_tool("postblox_sql_schema").unwrap();
        let args = validate_arguments(tool, json!({})).unwrap();
        assert_eq!(args, json!({}));
    }
}
