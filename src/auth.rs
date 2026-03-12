use axum::http::StatusCode;
use once_cell::sync::Lazy;
use reqwest::Client;
use secure_string::SecureString;

static HEADER_AUTHORIZATION: Lazy<SecureString> = Lazy::new(|| SecureString::from("authorization"));
static HEADER_COOKIE: Lazy<SecureString> = Lazy::new(|| SecureString::from("cookie"));
static HEADER_X_ORIGINAL_URI: Lazy<SecureString> =
    Lazy::new(|| SecureString::from("x-original-uri"));

pub async fn check_auth(
    client: &Client,
    auth_url: Option<&str>,
    authorization: Option<&str>,
    cookie: Option<&str>,
) -> Result<(), StatusCode> {
    let Some(url) = auth_url.map(str::trim).filter(|url| !url.is_empty()) else {
        return Ok(());
    };

    let mut request = client.get(url);

    if let Some(value) = authorization {
        request = request.header(HEADER_AUTHORIZATION.unsecure(), value);
    }

    if let Some(value) = cookie {
        request = request.header(HEADER_COOKIE.unsecure(), value);
    }

    request = request.header(HEADER_X_ORIGINAL_URI.unsecure(), "/socket.io");

    let response = request
        .send()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match response.status().as_u16() {
        200..=299 => Ok(()),
        401 => Err(StatusCode::UNAUTHORIZED),
        403 => Err(StatusCode::FORBIDDEN),
        _ => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}
