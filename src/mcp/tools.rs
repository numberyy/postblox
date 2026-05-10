use serde_json::{json, Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolSpec {
    pub name: &'static str,
    pub op: &'static str,
    pub description: &'static str,
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

pub const TOOLS: [ToolSpec; 12] = [
    ToolSpec {
        name: "postblox_account_list",
        op: "account.list",
        description: "List configured postblox email accounts.",
        dangerous: false,
        required: &[],
    },
    ToolSpec {
        name: "postblox_folder_list",
        op: "folder.list",
        description: "List folders for one account.",
        dangerous: false,
        required: &["account_id"],
    },
    ToolSpec {
        name: "postblox_thread_list",
        op: "thread.list",
        description: "List recent threads for one account.",
        dangerous: false,
        required: &["account_id"],
    },
    ToolSpec {
        name: "postblox_message_list_by_folder",
        op: "message.list_by_folder",
        description: "List messages in a folder.",
        dangerous: false,
        required: &["folder_id"],
    },
    ToolSpec {
        name: "postblox_message_list_by_thread",
        op: "message.list_by_thread",
        description: "List messages in a thread.",
        dangerous: false,
        required: &["thread_id"],
    },
    ToolSpec {
        name: "postblox_message_get",
        op: "message.get",
        description: "Fetch one message by id.",
        dangerous: false,
        required: &["id"],
    },
    ToolSpec {
        name: "postblox_search",
        op: "search",
        description: "Search local indexed email with optional account/folder/thread/date/sender/flag filters.",
        dangerous: false,
        required: &["q"],
    },
    ToolSpec {
        name: "postblox_draft_create",
        op: "draft.create",
        description: "Create an email draft.",
        dangerous: true,
        required: &["account_id", "to_addrs", "cc_addrs", "bcc_addrs"],
    },
    ToolSpec {
        name: "postblox_draft_update",
        op: "draft.update",
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
        op: "draft.delete",
        description: "Delete an email draft.",
        dangerous: true,
        required: &["id"],
    },
    ToolSpec {
        name: "postblox_message_set_flags",
        op: "message.set_flags",
        description: "Replace flags on one message.",
        dangerous: true,
        required: &["id", "flags"],
    },
    ToolSpec {
        name: "postblox_message_send",
        op: "message.send",
        description: "Send an existing draft through the account SMTP settings.",
        dangerous: true,
        required: &["account_id", "draft_id"],
    },
];

pub fn find_tool(name: &str) -> Option<&'static ToolSpec> {
    TOOLS.iter().find(|tool| tool.name == name)
}

pub fn list_tools() -> Value {
    json!({
        "tools": TOOLS.iter().map(tool_json).collect::<Vec<_>>(),
    })
}

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
        ("postblox_thread_list", "limit")
        | ("postblox_message_list_by_folder", "limit")
        | ("postblox_search", "limit") => Some(FieldKind::Integer {
            minimum: 1,
            maximum: 500,
        }),
        ("postblox_thread_list", "offset")
        | ("postblox_message_list_by_folder", "offset")
        | ("postblox_search", "offset") => Some(FieldKind::Integer {
            minimum: 0,
            maximum: i64::MAX,
        }),
        _ => None,
    }
}

fn validate_field(field: &str, value: &Value, kind: FieldKind) -> Result<(), String> {
    match kind {
        FieldKind::String | FieldKind::UuidString => {
            if value.is_string() {
                Ok(())
            } else {
                Err(format!("{field} must be a string"))
            }
        }
        FieldKind::NullableString | FieldKind::NullableUuidString => {
            if value.is_string() || value.is_null() {
                Ok(())
            } else {
                Err(format!("{field} must be a string or null"))
            }
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
            integer("limit", 1, 500, "Maximum rows to return."),
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
    fn test_tool_list_has_exactly_twelve_stable_names() {
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
            ]
        );
    }

    #[test]
    fn test_list_tools_returns_json_schemas_without_extra_tools() {
        let listed = list_tools();
        let tools = listed["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 12);
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
    fn test_validate_arguments_rejects_bad_integer_bounds() {
        let tool = find_tool("postblox_search").unwrap();
        let err = validate_arguments(tool, json!({"q": "mail", "limit": 0})).unwrap_err();
        assert_eq!(err, "limit must be between 1 and 500");
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
}
