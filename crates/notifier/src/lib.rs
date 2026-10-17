//! Notifier service and the production WebSocket router, shared with socket tests.
pub mod postgres_listener;
mod recorded_authority;
pub mod service;
pub mod subscriber_manager;
pub mod websocket_server;

/// Return connection metadata that is safe to include in logs.
pub fn mask_connection_url(url: &str) -> String {
    let Some((scheme, remainder)) = url.split_once("://") else {
        return "<redacted connection URL>".to_string();
    };
    if scheme.is_empty()
        || !scheme.chars().enumerate().all(|(index, ch)| {
            ch.is_ascii_alphabetic() || index > 0 && "+-.0123456789".contains(ch)
        })
    {
        return "<redacted connection URL>".to_string();
    }
    let suffix_start = remainder.find(['?', '#']).unwrap_or(remainder.len());
    let without_suffix = &remainder[..suffix_start];
    let authority_end = without_suffix.find('/').unwrap_or(without_suffix.len());
    let authority = &without_suffix[..authority_end];
    let path = &without_suffix[authority_end..];
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if host.is_empty()
        || host.contains('@')
        || host.chars().any(char::is_whitespace)
        || path.chars().any(char::is_whitespace)
    {
        return "<redacted connection URL>".to_string();
    }
    format!("{scheme}://{host}{path}")
}
