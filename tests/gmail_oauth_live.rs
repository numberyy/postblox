use chrono::{Duration, Utc};

use postblox::oauth::google::{
    xoauth2_sasl_string, GoogleOAuth, GoogleOAuthConfig, GoogleOAuthHttpClient, GoogleOAuthToken,
};

#[tokio::test]
#[ignore = "requires POSTBLOX_GMAIL_CLIENT_ID, POSTBLOX_GMAIL_CLIENT_SECRET, POSTBLOX_GMAIL_REFRESH_TOKEN, and POSTBLOX_GMAIL_EMAIL"]
async fn live_gmail_oauth_refresh_requires_postblox_gmail_client_id_secret_refresh_token_email_env()
{
    let required = [
        "POSTBLOX_GMAIL_CLIENT_ID",
        "POSTBLOX_GMAIL_CLIENT_SECRET",
        "POSTBLOX_GMAIL_REFRESH_TOKEN",
        "POSTBLOX_GMAIL_EMAIL",
    ];
    let missing = required
        .iter()
        .copied()
        .filter(|name| std::env::var(name).is_err())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        eprintln!("missing env vars for live Gmail OAuth test: {missing:?}");
        return;
    }

    let client_id = std::env::var("POSTBLOX_GMAIL_CLIENT_ID").unwrap();
    let client_secret = std::env::var("POSTBLOX_GMAIL_CLIENT_SECRET").unwrap();
    let refresh_token = std::env::var("POSTBLOX_GMAIL_REFRESH_TOKEN").unwrap();
    let email = std::env::var("POSTBLOX_GMAIL_EMAIL").unwrap();
    let redirect_uri = std::env::var("POSTBLOX_GMAIL_REDIRECT_URI")
        .unwrap_or_else(|_| "http://127.0.0.1/callback".into());

    let config = GoogleOAuthConfig::gmail(client_id, client_secret, redirect_uri);
    let stale = GoogleOAuthToken {
        access_token: "expired".into(),
        refresh_token,
        expires_at: Utc::now() - Duration::seconds(1),
        token_type: "Bearer".into(),
        scope: Some("https://mail.google.com/".into()),
    };

    let refreshed = GoogleOAuthHttpClient::new()
        .refresh_token(&config, &stale)
        .await
        .unwrap();
    assert!(!refreshed.access_token.is_empty());
    assert!(refreshed.expires_at > Utc::now());
    assert!(xoauth2_sasl_string(&email, &refreshed.access_token).contains("auth=Bearer "));
}
