use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SendMode {
    Shadow,
    #[default]
    Approval,
    AutoApprove,
    Autonomous,
}

impl fmt::Display for SendMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Shadow => "shadow",
            Self::Approval => "approval",
            Self::AutoApprove => "auto_approve",
            Self::Autonomous => "autonomous",
        })
    }
}

impl FromStr for SendMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "shadow" => Ok(Self::Shadow),
            "approval" => Ok(Self::Approval),
            "auto_approve" => Ok(Self::AutoApprove),
            "autonomous" => Ok(Self::Autonomous),
            other => Err(format!("invalid send mode: {other}")),
        }
    }
}

impl sqlx::Type<sqlx::Postgres> for SendMode {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <String as sqlx::Type<sqlx::Postgres>>::type_info()
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        <String as sqlx::Type<sqlx::Postgres>>::compatible(ty)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for SendMode {
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        let s = <String as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        s.parse::<SendMode>()
            .map_err(|e| -> sqlx::error::BoxDynError { e.into() })
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for SendMode {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <&str as sqlx::Encode<sqlx::Postgres>>::encode(
            match self {
                Self::Shadow => "shadow",
                Self::Approval => "approval",
                Self::AutoApprove => "auto_approve",
                Self::Autonomous => "autonomous",
            },
            buf,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Permission {
    pub id: Uuid,
    pub inbox_id: Uuid,
    pub send_mode: SendMode,
    pub rules: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Permission {
    pub fn default_for_inbox(inbox_id: Uuid) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::nil(),
            inbox_id,
            send_mode: SendMode::Approval,
            rules: serde_json::json!([]),
            created_at: now,
            updated_at: now,
        }
    }

    pub fn mode(&self) -> SendMode {
        self.send_mode
    }

    pub fn rules(&self) -> crate::core::rules::RuleSet {
        match serde_json::from_value(self.rules.clone()) {
            Ok(rs) => rs,
            Err(e) => {
                tracing::warn!(permission_id = %self.id, "invalid rules JSON, treating as empty: {e}");
                crate::core::rules::RuleSet(vec![])
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Organization {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct ApiKey {
    pub id: Uuid,
    pub org_id: Uuid,
    pub key_hash: String,
    pub prefix: String,
    pub name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Inbox {
    pub id: Uuid,
    pub org_id: Uuid,
    pub email: String,
    pub display_name: Option<String>,
    pub inbox_type: InboxType,
    pub active: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Thread {
    pub id: Uuid,
    pub inbox_id: Uuid,
    pub subject: Option<String>,
    pub message_count: i32,
    pub last_message_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Inbound,
    Outbound,
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Inbound => "inbound",
            Self::Outbound => "outbound",
        })
    }
}

impl FromStr for Direction {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "inbound" => Ok(Self::Inbound),
            "outbound" => Ok(Self::Outbound),
            other => Err(format!("invalid direction: {other}")),
        }
    }
}

impl sqlx::Type<sqlx::Postgres> for Direction {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <String as sqlx::Type<sqlx::Postgres>>::type_info()
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        <String as sqlx::Type<sqlx::Postgres>>::compatible(ty)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for Direction {
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        let s = <String as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        s.parse::<Direction>()
            .map_err(|e| -> sqlx::error::BoxDynError { e.into() })
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for Direction {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <&str as sqlx::Encode<sqlx::Postgres>>::encode(
            match self {
                Self::Inbound => "inbound",
                Self::Outbound => "outbound",
            },
            buf,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Message {
    pub id: Uuid,
    pub inbox_id: Uuid,
    pub thread_id: Option<Uuid>,
    pub message_id_header: Option<String>,
    pub in_reply_to: Option<String>,
    pub references_header: Option<String>,
    pub from_addr: String,
    pub to_addrs: serde_json::Value,
    pub cc_addrs: Option<serde_json::Value>,
    pub subject: Option<String>,
    pub text_body: Option<String>,
    pub html_body: Option<String>,
    pub extracted_text: Option<String>,
    pub direction: Direction,
    pub raw_headers: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    #[sqlx(default)]
    pub slop_score: Option<f32>,
    #[sqlx(default)]
    pub slop_signals: Option<serde_json::Value>,
    #[sqlx(default)]
    pub category: Option<String>,
    #[sqlx(default)]
    pub priority: Option<String>,
    #[sqlx(default)]
    pub triage_status: Option<String>,
    #[sqlx(default)]
    pub requires_action: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateMessage {
    pub inbox_id: Uuid,
    pub thread_id: Option<Uuid>,
    pub message_id_header: Option<String>,
    pub in_reply_to: Option<String>,
    pub references_header: Option<String>,
    pub from_addr: String,
    pub to_addrs: serde_json::Value,
    pub cc_addrs: Option<serde_json::Value>,
    pub subject: Option<String>,
    pub text_body: Option<String>,
    pub html_body: Option<String>,
    pub extracted_text: Option<String>,
    pub direction: Direction,
    pub raw_headers: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Webhook {
    pub id: Uuid,
    pub org_id: Uuid,
    pub url: String,
    pub events: serde_json::Value,
    pub secret: String,
    pub active: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Label {
    pub id: Uuid,
    pub inbox_id: Uuid,
    pub name: String,
    pub color: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Draft {
    pub id: Uuid,
    pub inbox_id: Uuid,
    pub to_addrs: serde_json::Value,
    pub cc_addrs: Option<serde_json::Value>,
    pub subject: Option<String>,
    pub text_body: Option<String>,
    pub html_body: Option<String>,
    pub in_reply_to_message_id: Option<Uuid>,
    pub updated_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateDraft {
    pub inbox_id: Uuid,
    pub to_addrs: serde_json::Value,
    pub cc_addrs: Option<serde_json::Value>,
    pub subject: Option<String>,
    pub text_body: Option<String>,
    pub html_body: Option<String>,
    pub in_reply_to_message_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Domain {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub status: DomainStatus,
    pub stalwart_principal_id: Option<String>,
    pub verified_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct LinkedAccount {
    pub id: Uuid,
    pub inbox_id: Uuid,
    pub org_id: Uuid,
    pub provider: String,
    pub imap_host: String,
    pub imap_port: i32,
    pub username: String,
    #[serde(skip_serializing)]
    pub password: String,
    pub last_sync_at: Option<DateTime<Utc>>,
    pub sync_status: crate::sync::SyncStatus,
    pub message_count: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateLinkedAccount {
    pub inbox_id: Uuid,
    pub org_id: Uuid,
    pub imap_host: String,
    pub imap_port: Option<i32>,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct SenderReputation {
    pub id: Uuid,
    pub org_id: Uuid,
    pub sender_email: String,
    pub total_messages: i32,
    pub slop_count: i32,
    pub last_seen_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

impl SenderReputation {
    pub fn slop_ratio(&self) -> f32 {
        if self.total_messages > 0 {
            self.slop_count as f32 / self.total_messages as f32
        } else {
            0.0
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct SlopFeedback {
    pub id: Uuid,
    pub org_id: Uuid,
    pub message_id: Uuid,
    pub is_slop: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    MessageSent,
    MessageReceived,
    MessageApproved,
    MessageRejected,
    PermissionChanged,
    InboxCreated,
    InboxDeleted,
    WebhookCreated,
    WebhookDeleted,
    DomainCreated,
    SyncTriggered,
    ApiKeyCreated,
    ApiKeyDeleted,
}

impl fmt::Display for AuditAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::MessageSent => "message_sent",
            Self::MessageReceived => "message_received",
            Self::MessageApproved => "message_approved",
            Self::MessageRejected => "message_rejected",
            Self::PermissionChanged => "permission_changed",
            Self::InboxCreated => "inbox_created",
            Self::InboxDeleted => "inbox_deleted",
            Self::WebhookCreated => "webhook_created",
            Self::WebhookDeleted => "webhook_deleted",
            Self::DomainCreated => "domain_created",
            Self::SyncTriggered => "sync_triggered",
            Self::ApiKeyCreated => "api_key_created",
            Self::ApiKeyDeleted => "api_key_deleted",
        })
    }
}

impl FromStr for AuditAction {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "message_sent" => Ok(Self::MessageSent),
            "message_received" => Ok(Self::MessageReceived),
            "message_approved" => Ok(Self::MessageApproved),
            "message_rejected" => Ok(Self::MessageRejected),
            "permission_changed" => Ok(Self::PermissionChanged),
            "inbox_created" => Ok(Self::InboxCreated),
            "inbox_deleted" => Ok(Self::InboxDeleted),
            "webhook_created" => Ok(Self::WebhookCreated),
            "webhook_deleted" => Ok(Self::WebhookDeleted),
            "domain_created" => Ok(Self::DomainCreated),
            "sync_triggered" => Ok(Self::SyncTriggered),
            "api_key_created" => Ok(Self::ApiKeyCreated),
            "api_key_deleted" => Ok(Self::ApiKeyDeleted),
            other => Err(format!("unknown audit action: {other}")),
        }
    }
}

impl sqlx::Type<sqlx::Postgres> for AuditAction {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <String as sqlx::Type<sqlx::Postgres>>::type_info()
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        <String as sqlx::Type<sqlx::Postgres>>::compatible(ty)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for AuditAction {
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        let s = <String as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        s.parse::<AuditAction>()
            .map_err(|e| -> sqlx::error::BoxDynError { e.into() })
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for AuditAction {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <&str as sqlx::Encode<sqlx::Postgres>>::encode(
            match self {
                Self::MessageSent => "message_sent",
                Self::MessageReceived => "message_received",
                Self::MessageApproved => "message_approved",
                Self::MessageRejected => "message_rejected",
                Self::PermissionChanged => "permission_changed",
                Self::InboxCreated => "inbox_created",
                Self::InboxDeleted => "inbox_deleted",
                Self::WebhookCreated => "webhook_created",
                Self::WebhookDeleted => "webhook_deleted",
                Self::DomainCreated => "domain_created",
                Self::SyncTriggered => "sync_triggered",
                Self::ApiKeyCreated => "api_key_created",
                Self::ApiKeyDeleted => "api_key_deleted",
            },
            buf,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct AuditEntry {
    pub id: Uuid,
    pub org_id: Uuid,
    pub inbox_id: Option<Uuid>,
    pub action: AuditAction,
    pub actor: String,
    pub details: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Rejected,
}

impl fmt::Display for ApprovalStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        })
    }
}

impl FromStr for ApprovalStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "approved" => Ok(Self::Approved),
            "rejected" => Ok(Self::Rejected),
            other => Err(format!("unknown approval status: {other}")),
        }
    }
}

impl sqlx::Type<sqlx::Postgres> for ApprovalStatus {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <String as sqlx::Type<sqlx::Postgres>>::type_info()
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        <String as sqlx::Type<sqlx::Postgres>>::compatible(ty)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for ApprovalStatus {
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        let s = <String as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        s.parse::<ApprovalStatus>()
            .map_err(|e| -> sqlx::error::BoxDynError { e.into() })
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for ApprovalStatus {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <&str as sqlx::Encode<sqlx::Postgres>>::encode(
            match self {
                Self::Pending => "pending",
                Self::Approved => "approved",
                Self::Rejected => "rejected",
            },
            buf,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Approval {
    pub id: Uuid,
    pub org_id: Uuid,
    pub inbox_id: Uuid,
    pub message_id: Uuid,
    pub status: ApprovalStatus,
    pub decided_by: Option<String>,
    pub decided_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateApproval {
    pub org_id: Uuid,
    pub inbox_id: Uuid,
    pub message_id: Uuid,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SearchResultWithInbox {
    pub id: Uuid,
    pub subject: Option<String>,
    pub from_addr: String,
    pub created_at: DateTime<Utc>,
    pub inbox_email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ApprovalWithDetails {
    pub id: Uuid,
    pub inbox_id: Uuid,
    pub message_id: Uuid,
    pub status: ApprovalStatus,
    pub created_at: DateTime<Utc>,
    pub subject: Option<String>,
    pub from_addr: String,
    pub inbox_email: String,
}

// === Trust ===

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct TrustScore {
    pub id: Uuid,
    pub inbox_id: Uuid,
    pub total_sends: i32,
    pub approved_count: i32,
    pub rejected_count: i32,
    pub auto_upgraded: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationProvider {
    Ntfy,
    Email,
    Webhook,
    Desktop,
}

impl fmt::Display for NotificationProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Ntfy => "ntfy",
            Self::Email => "email",
            Self::Webhook => "webhook",
            Self::Desktop => "desktop",
        })
    }
}

impl FromStr for NotificationProvider {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ntfy" => Ok(Self::Ntfy),
            "email" => Ok(Self::Email),
            "webhook" => Ok(Self::Webhook),
            "desktop" => Ok(Self::Desktop),
            other => Err(format!("unknown notification provider: {other}")),
        }
    }
}

impl sqlx::Type<sqlx::Postgres> for NotificationProvider {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <String as sqlx::Type<sqlx::Postgres>>::type_info()
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        <String as sqlx::Type<sqlx::Postgres>>::compatible(ty)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for NotificationProvider {
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        let s = <String as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        s.parse::<NotificationProvider>()
            .map_err(|e| -> sqlx::error::BoxDynError { e.into() })
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for NotificationProvider {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <&str as sqlx::Encode<sqlx::Postgres>>::encode(
            match self {
                Self::Ntfy => "ntfy",
                Self::Email => "email",
                Self::Webhook => "webhook",
                Self::Desktop => "desktop",
            },
            buf,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct NotificationConfig {
    pub id: Uuid,
    pub org_id: Uuid,
    pub provider: NotificationProvider,
    pub config: serde_json::Value,
    pub active: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateNotificationConfig {
    pub org_id: Uuid,
    pub provider: NotificationProvider,
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatusType {
    Delivered,
    Bounced,
    Complained,
}

impl fmt::Display for DeliveryStatusType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Delivered => "delivered",
            Self::Bounced => "bounced",
            Self::Complained => "complained",
        })
    }
}

impl FromStr for DeliveryStatusType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "delivered" => Ok(Self::Delivered),
            "bounced" => Ok(Self::Bounced),
            "complained" => Ok(Self::Complained),
            other => Err(format!("unknown delivery status: {other}")),
        }
    }
}

impl sqlx::Type<sqlx::Postgres> for DeliveryStatusType {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <String as sqlx::Type<sqlx::Postgres>>::type_info()
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        <String as sqlx::Type<sqlx::Postgres>>::compatible(ty)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for DeliveryStatusType {
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        let s = <String as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        s.parse::<DeliveryStatusType>()
            .map_err(|e| -> sqlx::error::BoxDynError { e.into() })
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for DeliveryStatusType {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <&str as sqlx::Encode<sqlx::Postgres>>::encode(
            match self {
                Self::Delivered => "delivered",
                Self::Bounced => "bounced",
                Self::Complained => "complained",
            },
            buf,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BounceType {
    Hard,
    Soft,
}

impl fmt::Display for BounceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Hard => "hard",
            Self::Soft => "soft",
        })
    }
}

impl FromStr for BounceType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "hard" => Ok(Self::Hard),
            "soft" => Ok(Self::Soft),
            other => Err(format!("unknown bounce type: {other}")),
        }
    }
}

impl sqlx::Type<sqlx::Postgres> for BounceType {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <String as sqlx::Type<sqlx::Postgres>>::type_info()
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        <String as sqlx::Type<sqlx::Postgres>>::compatible(ty)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for BounceType {
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        let s = <String as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        s.parse::<BounceType>()
            .map_err(|e| -> sqlx::error::BoxDynError { e.into() })
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for BounceType {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <&str as sqlx::Encode<sqlx::Postgres>>::encode(
            match self {
                Self::Hard => "hard",
                Self::Soft => "soft",
            },
            buf,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Admin,
    Member,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Admin => "admin",
            Self::Member => "member",
        })
    }
}

impl FromStr for Role {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "admin" => Ok(Self::Admin),
            "member" => Ok(Self::Member),
            other => Err(format!("invalid role: {other}")),
        }
    }
}

impl sqlx::Type<sqlx::Postgres> for Role {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <String as sqlx::Type<sqlx::Postgres>>::type_info()
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        <String as sqlx::Type<sqlx::Postgres>>::compatible(ty)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for Role {
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        let s = <String as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        s.parse::<Role>()
            .map_err(|e| -> sqlx::error::BoxDynError { e.into() })
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for Role {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        let s = match self {
            Role::Admin => "admin",
            Role::Member => "member",
        };
        <&str as sqlx::Encode<sqlx::Postgres>>::encode(s, buf)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxType {
    Native,
    Relay,
}

impl fmt::Display for InboxType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Native => "native",
            Self::Relay => "relay",
        })
    }
}

impl FromStr for InboxType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "native" => Ok(Self::Native),
            "relay" => Ok(Self::Relay),
            other => Err(format!("unknown inbox type: {other}")),
        }
    }
}

impl sqlx::Type<sqlx::Postgres> for InboxType {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <String as sqlx::Type<sqlx::Postgres>>::type_info()
    }
    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        <String as sqlx::Type<sqlx::Postgres>>::compatible(ty)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for InboxType {
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        let s = <String as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        s.parse::<InboxType>()
            .map_err(|e| -> sqlx::error::BoxDynError { e.into() })
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for InboxType {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <&str as sqlx::Encode<sqlx::Postgres>>::encode(
            match self { Self::Native => "native", Self::Relay => "relay" },
            buf,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Attachment,
    Inline,
}

impl fmt::Display for Disposition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Attachment => "attachment",
            Self::Inline => "inline",
        })
    }
}

impl FromStr for Disposition {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "attachment" => Ok(Self::Attachment),
            "inline" => Ok(Self::Inline),
            other => Err(format!("unknown disposition: {other}")),
        }
    }
}

impl sqlx::Type<sqlx::Postgres> for Disposition {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <String as sqlx::Type<sqlx::Postgres>>::type_info()
    }
    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        <String as sqlx::Type<sqlx::Postgres>>::compatible(ty)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for Disposition {
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        let s = <String as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        s.parse::<Disposition>()
            .map_err(|e| -> sqlx::error::BoxDynError { e.into() })
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for Disposition {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <&str as sqlx::Encode<sqlx::Postgres>>::encode(
            match self { Self::Attachment => "attachment", Self::Inline => "inline" },
            buf,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainStatus {
    Pending,
    Verified,
    Failed,
}

impl fmt::Display for DomainStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Pending => "pending",
            Self::Verified => "verified",
            Self::Failed => "failed",
        })
    }
}

impl FromStr for DomainStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "verified" => Ok(Self::Verified),
            "failed" => Ok(Self::Failed),
            other => Err(format!("unknown domain status: {other}")),
        }
    }
}

impl sqlx::Type<sqlx::Postgres> for DomainStatus {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <String as sqlx::Type<sqlx::Postgres>>::type_info()
    }
    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        <String as sqlx::Type<sqlx::Postgres>>::compatible(ty)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for DomainStatus {
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        let s = <String as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        s.parse::<DomainStatus>()
            .map_err(|e| -> sqlx::error::BoxDynError { e.into() })
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for DomainStatus {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <&str as sqlx::Encode<sqlx::Postgres>>::encode(
            match self { Self::Pending => "pending", Self::Verified => "verified", Self::Failed => "failed" },
            buf,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct OrgMember {
    pub id: Uuid,
    pub org_id: Uuid,
    pub api_key_id: Uuid,
    pub role: Role,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct DeliveryStatus {
    pub id: Uuid,
    pub message_id: Uuid,
    pub status: DeliveryStatusType,
    pub bounce_type: Option<BounceType>,
    pub details: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Attachment {
    pub id: Uuid,
    pub message_id: Uuid,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub storage_key: String,
    pub disposition: Disposition,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateAttachment {
    pub message_id: Uuid,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub storage_key: String,
    pub disposition: Disposition,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_send_mode_display_all_variants() {
        assert_eq!(SendMode::Shadow.to_string(), "shadow");
        assert_eq!(SendMode::Approval.to_string(), "approval");
        assert_eq!(SendMode::AutoApprove.to_string(), "auto_approve");
        assert_eq!(SendMode::Autonomous.to_string(), "autonomous");
    }

    #[test]
    fn test_send_mode_from_str_roundtrip() {
        for mode in [
            SendMode::Shadow,
            SendMode::Approval,
            SendMode::AutoApprove,
            SendMode::Autonomous,
        ] {
            let s = mode.to_string();
            let parsed: SendMode = s.parse().unwrap();
            assert_eq!(parsed, mode);
        }
    }

    #[test]
    fn test_send_mode_from_str_invalid_returns_err() {
        let result: Result<SendMode, _> = "invalid".parse();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid send mode"));
    }

    #[test]
    fn test_send_mode_default_is_approval() {
        assert_eq!(SendMode::default(), SendMode::Approval);
    }

    #[test]
    fn test_send_mode_serde_roundtrip() {
        let mode = SendMode::AutoApprove;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, "\"auto_approve\"");
        let back: SendMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, mode);
    }

    #[test]
    fn test_send_mode_serde_all_variants() {
        for (mode, expected_json) in [
            (SendMode::Shadow, "\"shadow\""),
            (SendMode::Approval, "\"approval\""),
            (SendMode::AutoApprove, "\"auto_approve\""),
            (SendMode::Autonomous, "\"autonomous\""),
        ] {
            let json = serde_json::to_string(&mode).unwrap();
            assert_eq!(json, expected_json);
            let back: SendMode = serde_json::from_str(&json).unwrap();
            assert_eq!(back, mode);
        }
    }

    #[test]
    fn test_permission_mode_returns_send_mode() {
        let perm = Permission {
            id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            send_mode: SendMode::Autonomous,
            rules: serde_json::json!({}),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        assert_eq!(perm.mode(), SendMode::Autonomous);
    }

    #[test]
    fn test_permission_serialization_roundtrip() {
        let perm = Permission {
            id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            send_mode: SendMode::Approval,
            rules: serde_json::json!({"max_daily": 100}),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let json = serde_json::to_string(&perm).unwrap();
        let back: Permission = serde_json::from_str(&json).unwrap();
        assert_eq!(perm, back);
    }

    #[test]
    fn test_organization_serialization_roundtrip() {
        let org = Organization {
            id: Uuid::new_v4(),
            name: "Acme Corp".into(),
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&org).unwrap();
        let back: Organization = serde_json::from_str(&json).unwrap();
        assert_eq!(org, back);
    }

    #[test]
    fn test_api_key_serialization_roundtrip() {
        let key = ApiKey {
            id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            key_hash: "hash123".into(),
            prefix: "pb_abcd".into(),
            name: Some("prod key".into()),
            created_at: Utc::now(),
            last_used_at: None,
        };
        let json = serde_json::to_string(&key).unwrap();
        let back: ApiKey = serde_json::from_str(&json).unwrap();
        assert_eq!(key, back);
    }

    #[test]
    fn test_api_key_nullable_fields() {
        let json = serde_json::json!({
            "id": Uuid::new_v4(),
            "org_id": Uuid::new_v4(),
            "key_hash": "h",
            "prefix": "pb_1234",
            "name": null,
            "created_at": "2026-03-12T00:00:00Z",
            "last_used_at": null
        });
        let key: ApiKey = serde_json::from_value(json).unwrap();
        assert!(key.name.is_none());
        assert!(key.last_used_at.is_none());
    }

    #[test]
    fn test_inbox_serialization_roundtrip() {
        let inbox = Inbox {
            id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            email: "bot@example.com".into(),
            display_name: Some("Bot".into()),
            inbox_type: InboxType::Native,
            active: true,
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&inbox).unwrap();
        let back: Inbox = serde_json::from_str(&json).unwrap();
        assert_eq!(inbox, back);
    }

    #[test]
    fn test_thread_serialization_roundtrip() {
        let thread = Thread {
            id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            subject: Some("Hello".into()),
            message_count: 5,
            last_message_at: Some(Utc::now()),
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&thread).unwrap();
        let back: Thread = serde_json::from_str(&json).unwrap();
        assert_eq!(thread, back);
    }

    #[test]
    fn test_thread_defaults_in_deserialization() {
        let json = serde_json::json!({
            "id": Uuid::new_v4(),
            "inbox_id": Uuid::new_v4(),
            "subject": null,
            "message_count": 0,
            "last_message_at": null,
            "created_at": "2026-03-12T00:00:00Z"
        });
        let thread: Thread = serde_json::from_value(json).unwrap();
        assert_eq!(thread.message_count, 0);
        assert!(thread.subject.is_none());
        assert!(thread.last_message_at.is_none());
    }

    #[test]
    fn test_message_serialization_roundtrip() {
        let msg = Message {
            id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            thread_id: None,
            message_id_header: Some("<abc@example.com>".into()),
            in_reply_to: None,
            references_header: None,
            from_addr: "sender@example.com".into(),
            to_addrs: serde_json::json!(["rcpt@example.com"]),
            cc_addrs: Some(serde_json::json!([])),
            subject: Some("Test".into()),
            text_body: Some("Hello".into()),
            html_body: None,
            extracted_text: None,
            direction: Direction::Inbound,
            raw_headers: None,
            created_at: Utc::now(),
            slop_score: None,
            slop_signals: None,
            category: None,
            priority: None,
            triage_status: None,
            requires_action: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn test_message_jsonb_fields_various_shapes() {
        let msg = Message {
            id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            thread_id: None,
            message_id_header: None,
            in_reply_to: None,
            references_header: None,
            from_addr: "a@b.com".into(),
            to_addrs: serde_json::json!(["a@b.com", "c@d.com", "e@f.com"]),
            cc_addrs: None,
            subject: None,
            text_body: None,
            html_body: None,
            extracted_text: None,
            direction: Direction::Outbound,
            raw_headers: Some(serde_json::json!({"X-Custom": "val", "X-Other": "val2"})),
            created_at: Utc::now(),
            slop_score: None,
            slop_signals: None,
            category: None,
            priority: None,
            triage_status: None,
            requires_action: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.to_addrs.as_array().unwrap().len(), 3);
        assert!(back.cc_addrs.is_none());
        assert!(back.raw_headers.is_some());
    }

    #[test]
    fn test_webhook_serialization_roundtrip() {
        let wh = Webhook {
            id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            url: "https://example.com/hook".into(),
            events: serde_json::json!(["message.inbound", "message.outbound"]),
            secret: "whsec_abc123".into(),
            active: true,
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&wh).unwrap();
        let back: Webhook = serde_json::from_str(&json).unwrap();
        assert_eq!(wh, back);
    }

    #[test]
    fn test_create_message_serialize() {
        let cm = CreateMessage {
            inbox_id: Uuid::new_v4(),
            thread_id: None,
            message_id_header: Some("<msg1@example.com>".into()),
            in_reply_to: None,
            references_header: None,
            from_addr: "bot@example.com".into(),
            to_addrs: serde_json::json!(["user@example.com"]),
            cc_addrs: None,
            subject: Some("Hi".into()),
            text_body: Some("Body".into()),
            html_body: None,
            extracted_text: None,
            direction: Direction::Outbound,
            raw_headers: None,
        };
        let json = serde_json::to_value(&cm).unwrap();
        assert_eq!(json["direction"], "outbound");
        assert!(json["to_addrs"][0] == "user@example.com");
    }

    #[test]
    fn test_label_serialization_roundtrip() {
        let label = Label {
            id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            name: "important".into(),
            color: Some("#ff0000".into()),
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&label).unwrap();
        let back: Label = serde_json::from_str(&json).unwrap();
        assert_eq!(label, back);
    }

    #[test]
    fn test_draft_serialization_roundtrip() {
        let draft = Draft {
            id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            to_addrs: serde_json::json!(["a@b.com"]),
            cc_addrs: Some(serde_json::json!(["c@d.com"])),
            subject: Some("Draft subject".into()),
            text_body: Some("Hello".into()),
            html_body: None,
            in_reply_to_message_id: None,
            updated_at: Utc::now(),
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&draft).unwrap();
        let back: Draft = serde_json::from_str(&json).unwrap();
        assert_eq!(draft, back);
    }

    #[test]
    fn test_create_draft_optional_fields() {
        let json = serde_json::json!({
            "inbox_id": Uuid::new_v4(),
            "to_addrs": []
        });
        let cd: CreateDraft = serde_json::from_value(json).unwrap();
        assert!(cd.subject.is_none());
        assert!(cd.text_body.is_none());
        assert!(cd.html_body.is_none());
        assert!(cd.cc_addrs.is_none());
        assert!(cd.in_reply_to_message_id.is_none());
    }

    #[test]
    fn test_domain_serialization_roundtrip() {
        let domain = Domain {
            id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            name: "example.com".into(),
            status: DomainStatus::Pending,
            stalwart_principal_id: None,
            verified_at: None,
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&domain).unwrap();
        let back: Domain = serde_json::from_str(&json).unwrap();
        assert_eq!(domain, back);
    }

    #[test]
    fn test_domain_nullable_fields() {
        let json = serde_json::json!({
            "id": Uuid::new_v4(),
            "org_id": Uuid::new_v4(),
            "name": "test.com",
            "status": "verified",
            "stalwart_principal_id": "principal-123",
            "verified_at": "2026-03-12T00:00:00Z",
            "created_at": "2026-03-12T00:00:00Z"
        });
        let domain: Domain = serde_json::from_value(json).unwrap();
        assert_eq!(
            domain.stalwart_principal_id.as_deref(),
            Some("principal-123")
        );
        assert!(domain.verified_at.is_some());
    }

    #[test]
    fn test_linked_account_password_excluded_from_serialization() {
        let acct = LinkedAccount {
            id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            provider: "imap".into(),
            imap_host: "imap.gmail.com".into(),
            imap_port: 993,
            username: "user@gmail.com".into(),
            password: "enc_secret".into(),
            last_sync_at: Some(Utc::now()),
            sync_status: crate::sync::SyncStatus::Idle,
            message_count: 42,
            created_at: Utc::now(),
        };
        let json = serde_json::to_value(&acct).unwrap();
        assert!(
            json.get("password").is_none(),
            "password must not appear in serialized output"
        );
        assert_eq!(json["username"].as_str().unwrap(), "user@gmail.com");
    }

    #[test]
    fn test_linked_account_nullable_fields() {
        let json = serde_json::json!({
            "id": Uuid::new_v4(),
            "inbox_id": Uuid::new_v4(),
            "org_id": Uuid::new_v4(),
            "provider": "imap",
            "imap_host": "imap.example.com",
            "imap_port": 993,
            "username": "user",
            "password": "pw",
            "last_sync_at": null,
            "sync_status": "idle",
            "message_count": 0,
            "created_at": "2026-03-12T00:00:00Z"
        });
        let acct: LinkedAccount = serde_json::from_value(json).unwrap();
        assert!(acct.last_sync_at.is_none());
        assert_eq!(acct.message_count, 0);
    }

    #[test]
    fn test_create_linked_account_serialization_roundtrip() {
        let create = CreateLinkedAccount {
            inbox_id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            imap_host: "imap.outlook.com".into(),
            imap_port: Some(993),
            username: "user@outlook.com".into(),
            password: "enc_pass".into(),
        };
        let json = serde_json::to_string(&create).unwrap();
        let back: CreateLinkedAccount = serde_json::from_str(&json).unwrap();
        assert_eq!(create, back);
    }

    #[test]
    fn test_create_linked_account_optional_fields() {
        let json = serde_json::json!({
            "inbox_id": Uuid::new_v4(),
            "org_id": Uuid::new_v4(),
            "imap_host": "imap.example.com",
            "username": "user",
            "password": "pw"
        });
        let create: CreateLinkedAccount = serde_json::from_value(json).unwrap();
        assert!(create.imap_port.is_none());
    }

    #[test]
    fn test_message_unicode_fields() {
        let msg = Message {
            id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            thread_id: None,
            message_id_header: None,
            in_reply_to: None,
            references_header: None,
            from_addr: "用户@example.com".into(),
            to_addrs: serde_json::json!(["пользователь@example.com"]),
            cc_addrs: None,
            subject: Some("日本語のメール".into()),
            text_body: Some("مرحبا بالعالم".into()),
            html_body: None,
            extracted_text: None,
            direction: Direction::Inbound,
            raw_headers: None,
            created_at: Utc::now(),
            slop_score: None,
            slop_signals: None,
            category: None,
            priority: None,
            triage_status: None,
            requires_action: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn test_audit_action_display_all_variants() {
        assert_eq!(AuditAction::MessageSent.to_string(), "message_sent");
        assert_eq!(AuditAction::MessageReceived.to_string(), "message_received");
        assert_eq!(AuditAction::MessageApproved.to_string(), "message_approved");
        assert_eq!(AuditAction::MessageRejected.to_string(), "message_rejected");
        assert_eq!(
            AuditAction::PermissionChanged.to_string(),
            "permission_changed"
        );
        assert_eq!(AuditAction::InboxCreated.to_string(), "inbox_created");
        assert_eq!(AuditAction::InboxDeleted.to_string(), "inbox_deleted");
        assert_eq!(AuditAction::WebhookCreated.to_string(), "webhook_created");
        assert_eq!(AuditAction::WebhookDeleted.to_string(), "webhook_deleted");
        assert_eq!(AuditAction::DomainCreated.to_string(), "domain_created");
        assert_eq!(AuditAction::SyncTriggered.to_string(), "sync_triggered");
        assert_eq!(AuditAction::ApiKeyCreated.to_string(), "api_key_created");
        assert_eq!(AuditAction::ApiKeyDeleted.to_string(), "api_key_deleted");
    }

    #[test]
    fn test_audit_action_from_str_roundtrip() {
        let actions = [
            AuditAction::MessageSent,
            AuditAction::MessageReceived,
            AuditAction::MessageApproved,
            AuditAction::MessageRejected,
            AuditAction::PermissionChanged,
            AuditAction::InboxCreated,
            AuditAction::InboxDeleted,
            AuditAction::WebhookCreated,
            AuditAction::WebhookDeleted,
            AuditAction::DomainCreated,
            AuditAction::SyncTriggered,
            AuditAction::ApiKeyCreated,
            AuditAction::ApiKeyDeleted,
        ];
        for action in actions {
            let s = action.to_string();
            let parsed: AuditAction = s.parse().unwrap();
            assert_eq!(parsed, action);
        }
    }

    #[test]
    fn test_audit_action_from_str_unknown_returns_err() {
        let result: Result<AuditAction, _> = "nonexistent".parse();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown audit action"));
    }

    #[test]
    fn test_audit_action_serde_roundtrip() {
        let action = AuditAction::MessageApproved;
        let json = serde_json::to_string(&action).unwrap();
        assert_eq!(json, "\"message_approved\"");
        let back: AuditAction = serde_json::from_str(&json).unwrap();
        assert_eq!(back, action);
    }

    #[test]
    fn test_audit_entry_serialization_roundtrip() {
        let entry = AuditEntry {
            id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            inbox_id: Some(Uuid::new_v4()),
            action: AuditAction::MessageSent,
            actor: "api_key:pb_1234".into(),
            details: serde_json::json!({"to": "user@example.com"}),
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: AuditEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, back);
    }

    #[test]
    fn test_audit_entry_nullable_inbox_id() {
        let entry = AuditEntry {
            id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            inbox_id: None,
            action: AuditAction::DomainCreated,
            actor: "system".into(),
            details: serde_json::json!({}),
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: AuditEntry = serde_json::from_str(&json).unwrap();
        assert!(back.inbox_id.is_none());
    }

    #[test]
    fn test_approval_status_display_all_variants() {
        assert_eq!(ApprovalStatus::Pending.to_string(), "pending");
        assert_eq!(ApprovalStatus::Approved.to_string(), "approved");
        assert_eq!(ApprovalStatus::Rejected.to_string(), "rejected");
    }

    #[test]
    fn test_approval_status_from_str_roundtrip() {
        for status in [
            ApprovalStatus::Pending,
            ApprovalStatus::Approved,
            ApprovalStatus::Rejected,
        ] {
            let s = status.to_string();
            let parsed: ApprovalStatus = s.parse().unwrap();
            assert_eq!(parsed, status);
        }
    }

    #[test]
    fn test_approval_status_from_str_unknown_returns_err() {
        let result: Result<ApprovalStatus, _> = "unknown".parse();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown approval status"));
    }

    #[test]
    fn test_approval_status_serde_roundtrip() {
        let status = ApprovalStatus::Rejected;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"rejected\"");
        let back: ApprovalStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, status);
    }

    #[test]
    fn test_approval_serialization_roundtrip() {
        let approval = Approval {
            id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            message_id: Uuid::new_v4(),
            status: ApprovalStatus::Pending,
            decided_by: None,
            decided_at: None,
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&approval).unwrap();
        let back: Approval = serde_json::from_str(&json).unwrap();
        assert_eq!(approval, back);
    }

    #[test]
    fn test_approval_with_decision() {
        let approval = Approval {
            id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            message_id: Uuid::new_v4(),
            status: ApprovalStatus::Approved,
            decided_by: Some("admin@example.com".into()),
            decided_at: Some(Utc::now()),
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&approval).unwrap();
        let back: Approval = serde_json::from_str(&json).unwrap();
        assert_eq!(back.decided_by.as_deref(), Some("admin@example.com"));
        assert!(back.decided_at.is_some());
    }

    #[test]
    fn test_create_approval_serialization_roundtrip() {
        let ca = CreateApproval {
            org_id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            message_id: Uuid::new_v4(),
        };
        let json = serde_json::to_string(&ca).unwrap();
        let back: CreateApproval = serde_json::from_str(&json).unwrap();
        assert_eq!(ca, back);
    }

    // === Trust model tests ===

    #[test]
    fn test_trust_score_serialization_roundtrip() {
        let ts = TrustScore {
            id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            total_sends: 20,
            approved_count: 17,
            rejected_count: 3,
            auto_upgraded: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let json = serde_json::to_string(&ts).unwrap();
        let back: TrustScore = serde_json::from_str(&json).unwrap();
        assert_eq!(ts, back);
    }

    #[test]
    fn test_trust_score_defaults() {
        let json = serde_json::json!({
            "id": Uuid::new_v4(),
            "inbox_id": Uuid::new_v4(),
            "total_sends": 0,
            "approved_count": 0,
            "rejected_count": 0,
            "auto_upgraded": false,
            "created_at": "2026-03-12T00:00:00Z",
            "updated_at": "2026-03-12T00:00:00Z"
        });
        let ts: TrustScore = serde_json::from_value(json).unwrap();
        assert_eq!(ts.total_sends, 0);
        assert!(!ts.auto_upgraded);
    }

    #[test]
    fn test_notification_provider_display_all_variants() {
        assert_eq!(NotificationProvider::Ntfy.to_string(), "ntfy");
        assert_eq!(NotificationProvider::Email.to_string(), "email");
        assert_eq!(NotificationProvider::Webhook.to_string(), "webhook");
        assert_eq!(NotificationProvider::Desktop.to_string(), "desktop");
    }

    #[test]
    fn test_notification_provider_from_str_roundtrip() {
        for provider in [
            NotificationProvider::Ntfy,
            NotificationProvider::Email,
            NotificationProvider::Webhook,
            NotificationProvider::Desktop,
        ] {
            let s = provider.to_string();
            let parsed: NotificationProvider = s.parse().unwrap();
            assert_eq!(parsed, provider);
        }
    }

    #[test]
    fn test_notification_provider_from_str_invalid_returns_err() {
        let result: Result<NotificationProvider, _> = "slack".parse();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("unknown notification provider"));
    }

    #[test]
    fn test_notification_provider_serde_roundtrip() {
        let provider = NotificationProvider::Ntfy;
        let json = serde_json::to_string(&provider).unwrap();
        assert_eq!(json, "\"ntfy\"");
        let back: NotificationProvider = serde_json::from_str(&json).unwrap();
        assert_eq!(back, provider);
    }

    #[test]
    fn test_notification_config_serialization_roundtrip() {
        let nc = NotificationConfig {
            id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            provider: NotificationProvider::Ntfy,
            config: serde_json::json!({"url": "https://ntfy.sh/postblox"}),
            active: true,
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&nc).unwrap();
        let back: NotificationConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(nc, back);
    }

    #[test]
    fn test_create_notification_config_serialization_roundtrip() {
        let cnc = CreateNotificationConfig {
            org_id: Uuid::new_v4(),
            provider: NotificationProvider::Webhook,
            config: serde_json::json!({"url": "https://example.com/hook"}),
        };
        let json = serde_json::to_string(&cnc).unwrap();
        let back: CreateNotificationConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cnc, back);
    }

    #[test]
    fn test_create_notification_config_empty_config() {
        let json = serde_json::json!({
            "org_id": Uuid::new_v4(),
            "provider": "email",
            "config": {}
        });
        let cnc: CreateNotificationConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cnc.provider, NotificationProvider::Email);
        assert_eq!(cnc.config, serde_json::json!({}));
    }

    #[test]
    fn test_delivery_status_type_display_all_variants() {
        assert_eq!(DeliveryStatusType::Delivered.to_string(), "delivered");
        assert_eq!(DeliveryStatusType::Bounced.to_string(), "bounced");
        assert_eq!(DeliveryStatusType::Complained.to_string(), "complained");
    }

    #[test]
    fn test_delivery_status_type_from_str_roundtrip() {
        for t in [
            DeliveryStatusType::Delivered,
            DeliveryStatusType::Bounced,
            DeliveryStatusType::Complained,
        ] {
            let s = t.to_string();
            let parsed: DeliveryStatusType = s.parse().unwrap();
            assert_eq!(parsed, t);
        }
    }

    #[test]
    fn test_delivery_status_type_from_str_unknown_returns_err() {
        let result: Result<DeliveryStatusType, _> = "unknown".parse();
        assert!(result.is_err());
    }

    #[test]
    fn test_delivery_status_type_serde_roundtrip() {
        let t = DeliveryStatusType::Bounced;
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, "\"bounced\"");
        let back: DeliveryStatusType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn test_bounce_type_display_all_variants() {
        assert_eq!(BounceType::Hard.to_string(), "hard");
        assert_eq!(BounceType::Soft.to_string(), "soft");
    }

    #[test]
    fn test_bounce_type_from_str_roundtrip() {
        for bt in [BounceType::Hard, BounceType::Soft] {
            let s = bt.to_string();
            let parsed: BounceType = s.parse().unwrap();
            assert_eq!(parsed, bt);
        }
    }

    #[test]
    fn test_bounce_type_from_str_unknown_returns_err() {
        let result: Result<BounceType, _> = "unknown".parse();
        assert!(result.is_err());
    }

    #[test]
    fn test_bounce_type_serde_roundtrip() {
        let bt = BounceType::Hard;
        let json = serde_json::to_string(&bt).unwrap();
        assert_eq!(json, "\"hard\"");
        let back: BounceType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, bt);
    }

    #[test]
    fn test_delivery_status_serialization_roundtrip() {
        let ds = DeliveryStatus {
            id: Uuid::new_v4(),
            message_id: Uuid::new_v4(),
            status: DeliveryStatusType::Bounced,
            bounce_type: Some(BounceType::Hard),
            details: Some(serde_json::json!({"smtp_code": 550})),
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&ds).unwrap();
        let back: DeliveryStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(ds, back);
    }

    #[test]
    fn test_delivery_status_nullable_fields() {
        let ds = DeliveryStatus {
            id: Uuid::new_v4(),
            message_id: Uuid::new_v4(),
            status: DeliveryStatusType::Delivered,
            bounce_type: None,
            details: None,
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&ds).unwrap();
        let back: DeliveryStatus = serde_json::from_str(&json).unwrap();
        assert!(back.bounce_type.is_none());
        assert!(back.details.is_none());
    }

    #[test]
    fn test_role_display_all_variants() {
        assert_eq!(Role::Admin.to_string(), "admin");
        assert_eq!(Role::Member.to_string(), "member");
    }

    #[test]
    fn test_role_from_str_roundtrip() {
        for role in [Role::Admin, Role::Member] {
            let s = role.to_string();
            let parsed: Role = s.parse().unwrap();
            assert_eq!(parsed, role);
        }
    }

    #[test]
    fn test_role_from_str_invalid_returns_err() {
        let result: Result<Role, _> = "superadmin".parse();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid role"));
    }

    #[test]
    fn test_role_serde_roundtrip() {
        let role = Role::Admin;
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, "\"admin\"");
        let back: Role = serde_json::from_str(&json).unwrap();
        assert_eq!(back, role);
    }

    #[test]
    fn test_org_member_serialization_roundtrip() {
        let member = OrgMember {
            id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            api_key_id: Uuid::new_v4(),
            role: Role::Admin,
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&member).unwrap();
        let back: OrgMember = serde_json::from_str(&json).unwrap();
        assert_eq!(member, back);
    }

    #[test]
    fn test_attachment_serialization_roundtrip() {
        let att = Attachment {
            id: Uuid::new_v4(),
            message_id: Uuid::new_v4(),
            filename: "report.pdf".into(),
            content_type: "application/pdf".into(),
            size_bytes: 1048576,
            storage_key: "msg-123/report.pdf".into(),
            disposition: Disposition::Attachment,
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&att).unwrap();
        let back: Attachment = serde_json::from_str(&json).unwrap();
        assert_eq!(att, back);
    }

    #[test]
    fn test_create_attachment_serialization_roundtrip() {
        let ca = CreateAttachment {
            message_id: Uuid::new_v4(),
            filename: "data.csv".into(),
            content_type: "text/csv".into(),
            size_bytes: 512,
            storage_key: "msg-456/data.csv".into(),
            disposition: Disposition::Attachment,
        };
        let json = serde_json::to_string(&ca).unwrap();
        let back: CreateAttachment = serde_json::from_str(&json).unwrap();
        assert_eq!(ca, back);
    }

    #[test]
    fn test_attachment_inline_disposition() {
        let att = Attachment {
            id: Uuid::new_v4(),
            message_id: Uuid::new_v4(),
            filename: "logo.png".into(),
            content_type: "image/png".into(),
            size_bytes: 2048,
            storage_key: "msg-789/logo.png".into(),
            disposition: Disposition::Inline,
            created_at: Utc::now(),
        };
        let json = serde_json::to_value(&att).unwrap();
        assert_eq!(json["disposition"], "inline");
    }
}
