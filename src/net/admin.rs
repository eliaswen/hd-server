//! Optional, bounded HTTP/1 admin listener. One request per connection.
use crate::{game::home::HomeData, storage::Store};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};
const MAX_HEADERS: usize = 16 * 1024;
const MAX_BODY: usize = 4 * 1024 * 1024;

pub fn address(value: &str, insecure: bool) -> Result<SocketAddr> {
    let address: SocketAddr = value.parse().context("admin listener must be an IP:port")?;
    ensure!(
        address.ip().is_loopback() || insecure,
        "refusing non-loopback admin bind without --admin-insecure"
    );
    Ok(address)
}
struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
}
async fn read(stream: &mut TcpStream) -> Result<Request> {
    let mut bytes = Vec::new();
    let mut chunk = [0; 1024];
    let boundary = loop {
        if let Some(pos) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            ensure!(pos + 4 <= MAX_HEADERS, "headers too large");
            break pos + 4;
        }
        ensure!(bytes.len() < MAX_HEADERS, "headers too large");
        let n = stream.read(&mut chunk).await?;
        ensure!(n > 0, "EOF before headers");
        bytes.extend_from_slice(&chunk[..n]);
    };
    let text = std::str::from_utf8(&bytes[..boundary])?;
    let mut lines = text.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split_whitespace();
    let method = first.next().context("missing method")?.to_owned();
    let path = first
        .next()
        .context("missing path")?
        .split('?')
        .next()
        .unwrap_or_default()
        .to_owned();
    ensure!(
        matches!(first.next(), Some("HTTP/1.1" | "HTTP/1.0")) && first.next().is_none(),
        "invalid request line"
    );
    let mut length = None;
    for line in lines.filter(|l| !l.is_empty()) {
        let (key, value) = line.split_once(':').context("invalid header")?;
        ensure!(
            !key.eq_ignore_ascii_case("transfer-encoding"),
            "transfer encoding unsupported"
        );
        if key.eq_ignore_ascii_case("content-length") {
            ensure!(length.is_none(), "duplicate content-length");
            length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .context("invalid content-length")?,
            );
        }
    }
    let length = length.unwrap_or(0);
    ensure!(length <= MAX_BODY, "request body too large");
    let target = boundary + length;
    while bytes.len() < target {
        let n = stream
            .read(&mut chunk[..(target - bytes.len()).min(1024)])
            .await?;
        ensure!(n > 0, "EOF before body");
        bytes.extend_from_slice(&chunk[..n]);
    }
    Ok(Request {
        method,
        path,
        body: bytes[boundary..target].to_vec(),
    })
}
fn route(store: &Store, request: Request) -> Result<(u16, Value)> {
    if request.path == "/accounts" {
        return if request.method == "GET" {
            Ok((200, store.list()?))
        } else {
            Ok((405, json!({"error":"method not allowed"})))
        };
    }
    let parts: Vec<_> = request.path.split('/').collect();
    if parts.len() != 4 || parts[1] != "accounts" || !matches!(parts[3], "home" | "turns") {
        return Ok((404, json!({"error":"not found"})));
    }
    let id = match parts[2].parse::<u64>() {
        Ok(id) => id,
        Err(_) => return Ok((400, json!({"error":"invalid account id"}))),
    };
    if !store.contains(id)? {
        return Ok((404, json!({"error":"account not found"})));
    }
    match (request.method.as_str(), parts[3]) {
        ("GET", "turns") => Ok((200, store.turns(id)?)),
        ("PUT", "home") => {
            let home = serde_json::from_slice::<HomeData>(&request.body)
                .context("invalid home JSON")
                .and_then(|home| {
                    home.validate()?;
                    Ok(home)
                });
            match home {
                Ok(home) => {
                    store.replace_home(id, home)?;
                    Ok((200, json!({"id":id,"updated":true})))
                }
                Err(e) => Ok((400, json!({"error":format!("{e:#}")}))),
            }
        }
        _ => Ok((405, json!({"error":"method not allowed"}))),
    }
}
pub async fn serve(mut stream: TcpStream, store: Arc<Store>) -> Result<()> {
    let response = match timeout(Duration::from_secs(10), read(&mut stream)).await {
        Ok(Ok(request)) => match route(&store, request) {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(error=%e,"admin request failed");
                (500, json!({"error":"storage error"}))
            }
        },
        Ok(Err(e)) => (400, json!({"error":format!("{e:#}")})),
        Err(_) => (408, json!({"error":"request timeout"})),
    };
    let (code, value) = response;
    let reason = match code {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        500 => "Internal Server Error",
        _ => bail!("invalid admin status"),
    };
    let body = serde_json::to_vec(&value)?;
    let headers=format!("HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",body.len());
    timeout(Duration::from_secs(10), async {
        stream.write_all(headers.as_bytes()).await?;
        stream.write_all(&body).await?;
        stream.shutdown().await
    })
    .await??;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    async fn request(store: Arc<Store>, method: &str, path: &str, body: &[u8]) -> (u16, Value) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let task = tokio::spawn(async move {
            serve(listener.accept().await.unwrap().0, store)
                .await
                .unwrap();
        });
        let header = format!(
            "{method} {path} HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        client.write_all(header.as_bytes()).await.unwrap();
        client.write_all(body).await.unwrap();
        let mut bytes = Vec::new();
        client.read_to_end(&mut bytes).await.unwrap();
        task.await.unwrap();
        let pos = bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let status = std::str::from_utf8(&bytes[..pos])
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        (status, serde_json::from_slice(&bytes[pos + 4..]).unwrap())
    }
    #[test]
    fn loopback_only() {
        assert!(address("0.0.0.0:8081", false).is_err());
        assert!(address("[::1]:8081", false).is_ok());
        assert!(address("0.0.0.0:8081", true).is_ok());
    }
    #[tokio::test]
    async fn admin_routes() {
        let (store, dir) = crate::storage::test_store("admin");
        let account = store.login(&[0; 12]).unwrap();
        let (code, list) = request(store.clone(), "GET", "/accounts", b"").await;
        assert_eq!(code, 200);
        assert_eq!(list[0]["id"], account.id);
        assert!(list[0].get("pass_token").is_none());
        let path = format!("/accounts/{}/home", account.id);
        assert_eq!(request(store.clone(), "PUT", &path, b"{}").await.0, 400);
        let mut home = HomeData::default();
        home.c["Objects"] = json!([]);
        assert_eq!(
            request(
                store.clone(),
                "PUT",
                &path,
                &serde_json::to_vec(&home).unwrap()
            )
            .await
            .0,
            200
        );
        assert_eq!(store.home(account.id).unwrap(), home);
        store.capture_turn(account.id, &[0, 0, 0]).unwrap();
        let (code, turns) = request(
            store.clone(),
            "GET",
            &format!("/accounts/{}/turns", account.id),
            b"",
        )
        .await;
        assert_eq!(code, 200);
        assert_eq!(turns[0]["body"], "000000");
        assert_eq!(
            request(store.clone(), "GET", "/accounts/999/home", b"")
                .await
                .0,
            404
        );
        assert_eq!(
            request(store.clone(), "POST", "/accounts", b"").await.0,
            405
        );
        assert_eq!(request(store.clone(), "GET", "/missing", b"").await.0, 404);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
