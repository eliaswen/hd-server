//! HTTP bootstrap: `GET /env_settings` (native parser `0x14c412`).
//!
//! The parser only enables the game driver (`world+0x155`, setter
//! `0x3b2228` called at `0x14c973`) when `status` is NOT the special
//! update value; every other branch parks the bootstrap object in its
//! error/update state (`obj+0x24 = 6`) and the client never leaves Loading.
//! Bootstrap envelope fields follow the live client.

use std::{path::PathBuf, time::Duration};

use anyhow::{bail, ensure, Result};
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

const MAX_HEADERS: usize = 16 * 1024;
/// The status value whose branch skips the driver enable (INFERRED "update").
const GATE_BLOCKING_STATUS: &str = "update";

pub struct Bootstrap {
    pub game_host: String,
    pub asset_url: String,
    /// An explicit --asset-url takes precedence over the JSON override field.
    pub explicit_asset_url: bool,
    /// JSON override, re-read on every request; explicit_asset_url replaces its asset list.
    pub override_path: Option<PathBuf>,
}

impl Bootstrap {
    pub fn validate(body: &serde_json::Value) -> Result<()> {
        ensure!(body.is_object(), "env_settings JSON must be an object");
        ensure!(
            !body
                .get("status")
                .and_then(|s| s.as_str())
                .is_some_and(|s| s.eq_ignore_ascii_case(GATE_BLOCKING_STATUS)),
            "env_settings status=update prevents the client entering the game"
        );
        Ok(())
    }
    fn default_body(&self) -> Vec<u8> {
        // Field names and the string-valued storeReady come from the native parser.
        serde_json::to_vec(&json!({
            "environment": "prod",
            "serverHost": self.game_host,
            "assetUrlList": [self.asset_url],
            "status": "ok",
            "storeReady": "true"
        }))
        .expect("serializing a JSON object of strings")
    }

    /// Warns when a response would keep the client's game driver disabled.
    pub fn check(body: &serde_json::Value) {
        let status = body.get("status").and_then(|s| s.as_str()).unwrap_or("");
        if status.eq_ignore_ascii_case(GATE_BLOCKING_STATUS) {
            tracing::warn!("env_settings status={status:?} takes the update branch; the client will not enter the game");
        }
        if body.get("environment").and_then(|s| s.as_str()) == Some("maintenance") {
            tracing::warn!("env_settings environment=maintenance shows the maintenance dialog");
        }
    }
}

async fn read_headers(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut headers = Vec::new();
    let mut chunk = [0; 1024];
    loop {
        let remaining = (MAX_HEADERS - headers.len()).min(chunk.len());
        if remaining == 0 {
            bail!("HTTP headers exceed {MAX_HEADERS} bytes");
        }
        let n = stream.read(&mut chunk[..remaining]).await?;
        if n == 0 {
            bail!("EOF before HTTP headers completed");
        }
        headers.extend_from_slice(&chunk[..n]);
        if headers.windows(4).any(|w| w == b"\r\n\r\n") {
            return Ok(headers);
        }
    }
}

/// Path of an origin-form (`/x?y`) or proxy absolute-form
/// (`http://host[:port]/x?y`) request target. The latter lets a device use
/// this server as its HTTP proxy for the bootstrap request.
fn request_path(target: &str) -> &str {
    let target = match target.strip_prefix("http://") {
        Some(rest) => rest.find('/').map_or("/", |i| &rest[i..]),
        None => target,
    };
    target.split('?').next().unwrap_or_default()
}

pub async fn serve(mut stream: TcpStream, bootstrap: &Bootstrap) -> Result<()> {
    let headers = timeout(Duration::from_secs(10), read_headers(&mut stream)).await??;
    let text = std::str::from_utf8(&headers)?;
    let mut parts = text.lines().next().unwrap_or_default().split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let version = parts.next().unwrap_or_default();
    tracing::info!(%method, %target, "HTTP request");
    let (status, body) = if !matches!(version, "HTTP/1.0" | "HTTP/1.1") || parts.next().is_some() {
        ("400 Bad Request", b"{}".to_vec())
    } else if method != "GET" {
        ("405 Method Not Allowed", b"{}".to_vec())
    } else if request_path(target) != "/env_settings" {
        ("404 Not Found", b"{}".to_vec())
    } else if let Some(path) = &bootstrap.override_path {
        match tokio::fs::read(path).await.map(|b| {
            let parsed = serde_json::from_slice::<serde_json::Value>(&b);
            (b, parsed)
        }) {
            Ok((bytes, Ok(mut json))) => {
                Bootstrap::check(&json);
                if Bootstrap::validate(&json).is_ok() {
                    let bytes = if bootstrap.explicit_asset_url {
                        json["assetUrlList"] = json!([bootstrap.asset_url]);
                        serde_json::to_vec(&json)?
                    } else {
                        bytes
                    };
                    ("200 OK", bytes)
                } else {
                    ("500 Internal Server Error", b"{}".to_vec())
                }
            }
            _ => {
                tracing::warn!(path = %path.display(), "env settings override unavailable or invalid");
                ("500 Internal Server Error", b"{}".to_vec())
            }
        }
    } else {
        ("200 OK", bootstrap.default_body())
    };
    if tracing::enabled!(tracing::Level::DEBUG) {
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        tracing::debug!(%status,body_bytes=body.len(),path=request_path(target),override_file=?bootstrap.override_path,
            server_host=?value.get("serverHost"),asset_urls=?value.get("assetUrlList"),environment=?value.get("environment"),
            "HTTP response selected; env_settings advertises the game address and asset URLs before TCP login");
    }
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
        body.len()
    );
    timeout(Duration::from_secs(10), async {
        stream.write_all(response.as_bytes()).await?;
        stream.write_all(&body).await?;
        stream.shutdown().await
    })
    .await??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[test]
    fn update_status_is_rejected() {
        assert!(Bootstrap::validate(&json!({"status":"UPDATE"})).is_err());
        assert!(Bootstrap::validate(&json!({"status":"ok"})).is_ok());
    }

    fn bootstrap() -> Bootstrap {
        Bootstrap {
            game_host: "198.18.0.1".into(),
            asset_url: "http://192.168.240.1:8080/assets/".into(),
            explicit_asset_url: false,
            override_path: None,
        }
    }

    async fn request(chunks: &[&[u8]]) -> Vec<u8> {
        request_with(chunks, bootstrap()).await
    }

    async fn request_with(chunks: &[&[u8]], bootstrap: Bootstrap) -> Vec<u8> {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            serve(stream, &bootstrap).await.unwrap();
        });
        for chunk in chunks {
            client.write_all(chunk).await.unwrap();
            tokio::task::yield_now().await;
        }
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        server.await.unwrap();
        response
    }

    #[tokio::test]
    async fn explicit_asset_url_overrides_json_and_survives_reload() {
        let path = std::env::temp_dir().join(format!("hd-env-assets-{}.json", std::process::id()));
        let request = [b"GET /env_settings HTTP/1.1\r\n\r\n".as_slice()];
        for assets in [json!(null), json!(["http://file-assets/"])] {
            std::fs::write(
                &path,
                serde_json::to_vec(
                    &json!({"status":"ok","assetUrlList":assets,"custom":"preserved"}),
                )
                .unwrap(),
            )
            .unwrap();
            for explicit in [false, true] {
                let bootstrap = Bootstrap {
                    asset_url: "http://game-assets.haydaygame.com/".into(),
                    explicit_asset_url: explicit,
                    override_path: Some(path.clone()),
                    ..bootstrap()
                };
                let bytes = request_with(&request, bootstrap).await;
                let boundary = bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
                let value: serde_json::Value =
                    serde_json::from_slice(&bytes[boundary + 4..]).unwrap();
                assert_eq!(value["custom"], "preserved");
                assert_eq!(
                    value["assetUrlList"],
                    if explicit {
                        json!(["http://game-assets.haydaygame.com/"])
                    } else {
                        assets.clone()
                    }
                );
            }
        }
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn bootstrap_accepts_fragmented_real_client_request() {
        let response = request(&[
            b"GET /env_settings?version=1.43.150&platform=android&store=2 HTTP/1.1\r\n",
            b"Host: gamea.haydaygame.com:8080\r\nConnection: close\r\n\r\n",
        ])
        .await;
        let boundary = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let headers = std::str::from_utf8(&response[..boundary]).unwrap();
        let body = &response[boundary + 4..];
        assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(headers.contains(&format!("Content-Length: {}", body.len())));
        let json: serde_json::Value = serde_json::from_slice(body).unwrap();
        assert_eq!(json["serverHost"], "198.18.0.1");
        assert_eq!(json["storeReady"], "true");
        assert_eq!(json["assetUrlList"][0], "http://192.168.240.1:8080/assets/");
        // The driver gate requires a status other than the update value.
        assert!(!json["status"]
            .as_str()
            .unwrap()
            .eq_ignore_ascii_case(GATE_BLOCKING_STATUS));
    }

    #[test]
    fn proxy_absolute_form_targets() {
        assert_eq!(request_path("/env_settings?version=1"), "/env_settings");
        assert_eq!(
            request_path("http://gamea.haydaygame.com:8080/env_settings?version=1.43.150"),
            "/env_settings"
        );
        assert_eq!(request_path("http://example.com"), "/");
        assert_eq!(request_path("example.com:443"), "example.com:443");
    }

    #[tokio::test]
    async fn rejects_unimplemented_routes_and_methods() {
        for (input, status) in [
            (&b"GET /assets/fingerprint.json HTTP/1.1\r\n\r\n"[..], "404"),
            (&b"POST /env_settings HTTP/1.1\r\n\r\n"[..], "405"),
            (&b"GET /env_settings INVALID\r\n\r\n"[..], "400"),
        ] {
            let response = request(&[input]).await;
            assert!(std::str::from_utf8(&response)
                .unwrap()
                .starts_with(&format!("HTTP/1.1 {status}")));
        }
    }
}
