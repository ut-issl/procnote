//! Connecting a workspace to a server (RFC 8252 loopback redirect).
//!
//! The app listens on a random port of 127.0.0.1, opens the server's
//! `/device/connect` page in the browser with that callback and a random
//! state, and waits. After the person signs in (Cloudflare Access) and
//! confirms, the page redirects to the callback with the device token.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

use crate::link::ServerLink;

/// A connection waiting for the browser to come back.
pub struct PendingConnect {
    listener: TcpListener,
    state: String,
    server: String,
    device: String,
    /// The page to open in the browser.
    pub url: Url,
}

/// The server's connect page for a callback at `callback` with `state`.
pub fn connect_url(server: &str, callback: &str, state: &str, device: &str) -> Result<Url, String> {
    let mut url = Url::parse(server)
        .and_then(|base| base.join("/device/connect"))
        .map_err(|e| format!("invalid server URL {server}: {e}"))?;
    url.query_pairs_mut()
        .append_pair("callback", callback)
        .append_pair("state", state)
        .append_pair("device", device);
    Ok(url)
}

/// The token and owner in a callback request line (`GET /callback?… HTTP/1.1`),
/// if it carries `expected_state`.
pub fn parse_callback(
    request_line: &str,
    expected_state: &str,
) -> Result<(String, String), String> {
    let target = request_line
        .split_whitespace()
        .nth(1)
        .ok_or("malformed callback request")?;
    let url = Url::parse("http://127.0.0.1")
        .and_then(|base| base.join(target))
        .map_err(|e| format!("malformed callback: {e}"))?;
    let mut state = None;
    let mut token = None;
    let mut email = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "state" => state = Some(value.into_owned()),
            "token" => token = Some(value.into_owned()),
            "email" => email = Some(value.into_owned()),
            _ => {}
        }
    }
    if state.as_deref() != Some(expected_state) {
        return Err("the callback doesn't belong to this connection".to_string());
    }
    Ok((
        token.ok_or("the callback has no token")?,
        email.unwrap_or_default(),
    ))
}

/// Start connecting `device` to `server`: listen for the callback.
pub async fn begin(server: &str, device: &str) -> Result<PendingConnect, String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let state = uuid::Uuid::new_v4().simple().to_string();
    let callback = format!("http://127.0.0.1:{port}/callback");
    let url = connect_url(server, &callback, &state, device)?;
    Ok(PendingConnect {
        listener,
        state,
        server: server.trim_end_matches('/').to_string(),
        device: device.to_string(),
        url,
    })
}

const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>procnote</title>\
<p style=\"font-family:sans-serif\">Connected. You can close this tab and go back to procnote.</p>";

impl PendingConnect {
    /// Wait (up to `timeout`) for the browser to come back with a token.
    /// Requests that don't carry this connection's state are answered and ignored.
    pub async fn finish(self, timeout: Duration) -> Result<ServerLink, String> {
        tokio::time::timeout(timeout, async {
            loop {
                let (mut socket, _) = self.listener.accept().await.map_err(|e| e.to_string())?;
                let mut buffer = vec![0u8; 8192];
                let read = socket.read(&mut buffer).await.map_err(|e| e.to_string())?;
                let request = String::from_utf8_lossy(&buffer[..read]);
                let request_line = request.lines().next().unwrap_or_default();
                let outcome = parse_callback(request_line, &self.state);
                let body = if outcome.is_ok() { DONE_PAGE } else { "Not a procnote connection." };
                let response = format!(
                    "HTTP/1.1 {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    if outcome.is_ok() { "200 OK" } else { "400 Bad Request" },
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                if let Ok((token, email)) = outcome {
                    return Ok(ServerLink {
                        server: self.server.clone(),
                        token,
                        email,
                        device: self.device.clone(),
                    });
                }
            }
        })
        .await
        .map_err(|_| "timed out waiting for the browser".to_string())?
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
mod tests {
    use super::*;

    #[test]
    fn the_connect_page_gets_the_callback_state_and_device() {
        let url = connect_url(
            "https://procnote.example.ac.jp/",
            "http://127.0.0.1:5000/callback",
            "abc",
            "bench PC",
        )
        .unwrap();

        assert_eq!(url.path(), "/device/connect");
        let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
        assert_eq!(
            pairs,
            [
                (
                    "callback".to_string(),
                    "http://127.0.0.1:5000/callback".to_string()
                ),
                ("state".to_string(), "abc".to_string()),
                ("device".to_string(), "bench PC".to_string()),
            ]
        );
    }

    #[test]
    fn only_a_callback_with_this_state_yields_the_token() {
        let line = "GET /callback?state=abc&token=pnd_1&email=me%40example.ac.jp HTTP/1.1";

        assert_eq!(
            parse_callback(line, "abc"),
            Ok(("pnd_1".to_string(), "me@example.ac.jp".to_string()))
        );
        assert!(parse_callback(line, "other").is_err());
        assert!(parse_callback("GET /callback?state=abc HTTP/1.1", "abc").is_err());
        assert!(parse_callback("garbage", "abc").is_err());
    }

    #[tokio::test]
    async fn a_browser_coming_back_completes_the_connection() {
        let pending = begin("http://server.example", "bench PC").await.unwrap();
        let state = pending
            .url
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned();
        let callback = pending
            .url
            .query_pairs()
            .find(|(key, _)| key == "callback")
            .unwrap()
            .1
            .into_owned();
        let finishing = tokio::spawn(pending.finish(Duration::from_secs(5)));

        // A stray request first, then the real one.
        let stray = reqwest::get(format!("{callback}?state=wrong&token=x"))
            .await
            .unwrap();
        let done = reqwest::get(format!("{callback}?state={state}&token=pnd_t&email=me%40x"))
            .await
            .unwrap();

        assert_eq!(stray.status(), 400);
        assert_eq!(done.status(), 200);
        let link = finishing.await.unwrap().unwrap();
        assert_eq!(link.token, "pnd_t");
        assert_eq!(link.email, "me@x");
        assert_eq!(link.server, "http://server.example");
    }
}
