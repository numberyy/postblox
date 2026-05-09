//! OAuth2 flows for IMAP/SMTP authentication.
//!
//! The namespace is provider-scoped: each external identity provider
//! gets its own submodule. Only Gmail is implemented today, in
//! [`google`]; future providers (Microsoft Graph, etc.) will land as
//! peer modules and reuse the same shape.
//!
//! Nothing provider-agnostic lives at this level yet — the moment we
//! have two real providers, common types (config validators, token
//! storage helpers) get extracted here. Until then, per the project's
//! "no abstractions before the third use" rule, [`google`] is the
//! whole module.

pub mod google;
