use axum::http::StatusCode;
use reqwest::Client;

const HEADER_AUTHORIZATION: &str = "authorization";
const HEADER_COOKIE: &str = "cookie";
const HEADER_X_ORIGINAL_URI: &str = "x-original-uri";

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
        request = request.header(HEADER_AUTHORIZATION, value);
    }

    if let Some(value) = cookie {
        request = request.header(HEADER_COOKIE, value);
    }

    request = request.header(
        HEADER_X_ORIGINAL_URI,
        "/socket.io",
    );

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
