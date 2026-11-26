//! Game TCP connection: drives a [`Session`] and records traffic.
//!
//! Per connection three files are written under the dump directory:
//! `-c2s.bin` / `-s2c.bin` (exact raw bytes, preserved even when framing
//! fails) and `-plain.jsonl` (one decrypted message per line).

use std::{collections::VecDeque, path::Path};

use anyhow::Result;
use serde_json::json;
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::{sleep_until, timeout, Instant},
};

use crate::protocol::scframe::Frame;
use crate::{
    protocol::catalogue,
    net::session::{Config, Direction, Record, Session},
};

pub const MAX_PAYLOAD: usize = 1024 * 1024;

pub struct Dumps {
    c2s: File,
    s2c: File,
    plain: File,
    start: Instant,
    scenario: String,
}

impl Dumps {
    /// `stem` is the path prefix; suffixes are appended.
    pub async fn create(stem: &Path, scenario: &str) -> Result<Self> {
        async fn open(stem: &Path, suffix: &str) -> std::io::Result<File> {
            let mut name = stem.as_os_str().to_owned();
            name.push(suffix);
            tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(name)
                .await
        }
        Ok(Self {
            c2s: open(stem, "-c2s.bin").await?,
            s2c: open(stem, "-s2c.bin").await?,
            plain: open(stem, "-plain.jsonl").await?,
            start: Instant::now(),
            scenario: scenario.into(),
        })
    }

    async fn line(&mut self, mut value: serde_json::Value) -> Result<()> {
        value["time"] = json!(chrono::Utc::now().to_rfc3339());
        value["t_ms"] = json!(self.start.elapsed().as_millis() as u64);
        value["scenario"] = json!(self.scenario);
        self.plain
            .write_all(format!("{value}\n").as_bytes())
            .await?;
        Ok(())
    }

    /// A non-message event (connection start/close, configuration).
    pub async fn event(&mut self, event: &str, details: serde_json::Value) -> Result<()> {
        self.line(json!({ "event": event, "details": details }))
            .await
    }

    async fn record(&mut self, record: &Record) -> Result<()> {
        self.line(json!({
            "dir": match record.direction {
                Direction::ClientToServer => "c2s",
                Direction::ServerToClient => "s2c",
            },
            "type": record.msg_type,
            "label": catalogue::label(record.msg_type),
            "version": record.version,
            "encrypted": record.encrypted,
            "len": record.body.len(),
            "body": hex(&record.body),
        }))
        .await
    }

    async fn send(&mut self, stream: &mut TcpStream, record: &Record, frame: &Frame) -> Result<()> {
        let wire = frame.to_wire();
        self.s2c.write_all(&wire).await?;
        stream.write_all(&wire).await?;
        tracing::debug!(direction="s2c",label=%catalogue::label(frame.msg_type),wire_bytes=wire.len(),body_bytes=record.body.len(),"server message written to TCP socket and capture");
        self.record(record).await
    }

    async fn flush(&mut self) -> Result<()> {
        self.c2s.flush().await?;
        self.s2c.flush().await?;
        self.plain.flush().await?;
        Ok(())
    }
}

pub async fn serve(mut stream: TcpStream, mut dumps: Dumps, config: &Config) -> Result<()> {
    let mut session = Session::new(config);
    let mut chunk = [0; 8192];
    let mut pending = Vec::new();
    let mut framing = true;
    let mut total = 0;
    let mut scheduled: VecDeque<(Instant, crate::net::session::Outgoing)> = VecDeque::new();
    let mut reason = "client closed";
    let result = async {
        loop {
            let next = scheduled.front().map(|(at, ..)| *at);
            let n = tokio::select! {
                read = timeout(config.idle, stream.read(&mut chunk)) => match read {
                    Ok(result) => result?,
                    Err(_) => {
                        reason = "idle timeout";
                        tracing::info!("closing idle connection");
                        break;
                    }
                },
                _ = sleep_until(next.unwrap_or_else(Instant::now)), if next.is_some() => {
                    let (_, out) = scheduled.pop_front().unwrap();
                    let (record, frame) = session.seal_deferred(&out)?;
                    tracing::info!(label = %catalogue::label(frame.msg_type), "sent deferred message");
                    dumps.send(&mut stream, &record, &frame).await?;
                    continue;
                }
            };
            if n == 0 {
                break;
            }
            dumps.c2s.write_all(&chunk[..n]).await?;
            total += n;
            if framing {
                pending.extend_from_slice(&chunk[..n]);
                loop {
                    match crate::protocol::scframe::try_parse(&mut pending, MAX_PAYLOAD) {
                        Ok(Some(frame)) => {
                            // These types have a sampled plaintext log in Session;
                            // logging every encrypted frame here would defeat it.
                            if !matches!(frame.msg_type, 16368 | 11292) {
                                tracing::info!(
                                    label = %catalogue::label(frame.msg_type),
                                    version = frame.version,
                                    len = frame.payload.len(),
                                    "client frame"
                                );
                            }
                            let step = session.handle(&frame)?;
                            if step.clear_deferred {
                                scheduled.clear();
                            }
                            // Inbound record(s) first, then replies in order.
                            let inbound = step.records.len() - step.replies.len();
                            for record in &step.records[..inbound] {
                                dumps.record(record).await?;
                            }
                            for (record, reply) in step.records[inbound..].iter().zip(&step.replies) {
                                dumps.send(&mut stream, record, reply).await?;
                            }
                            if step.close {
                                reason = "development friend action; reload account";
                                stream.shutdown().await?;
                                return anyhow::Ok(());
                            }
                            let at = Instant::now() + config.after_ready_delay;
                            if step.replies.is_empty() && step.deferred.is_empty() {
                                tracing::debug!(label=%catalogue::label(frame.msg_type),"client message handled without a server reply");
                            }
                            scheduled.extend(step.deferred.into_iter().map(|out| (at, out)));
                        }
                        Ok(None) => break,
                        Err(error) => {
                            tracing::warn!(%error, "unrecognized framing; preserving raw capture");
                            framing = false;
                            pending.clear();
                            break;
                        }
                    }
                }
            }
        }
        anyhow::Ok(())
    }
    .await;
    let error = result.as_ref().err().map(|e| format!("{e:#}"));
    dumps
        .event(
            "close",
            json!({
                "reason": if error.is_some() { "error" } else { reason },
                "error": error,
                "client_bytes": total,
                "unparsed": pending.len(),
                "unsent_deferred": scheduled.len(),
                "post_login": session.post_login_counts(),
                "client_types_seen": session.seen_counts(),
            }),
        )
        .await?;
    dumps.flush().await?;
    tracing::info!(
        total,
        unparsed = pending.len(),
        post_login = ?session.post_login_counts(),
        "game connection closed"
    );
    result
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::session::tests::{config, TestClient};
    use std::time::Duration;
    use tokio::net::TcpListener;

    fn stem(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "hd-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    async fn cleanup(stem: &Path) -> (Vec<u8>, Vec<u8>, String) {
        let mut out = Vec::new();
        for suffix in ["-c2s.bin", "-s2c.bin", "-plain.jsonl"] {
            let mut name = stem.as_os_str().to_owned();
            name.push(suffix);
            out.push(tokio::fs::read(&name).await.unwrap());
            tokio::fs::remove_file(name).await.unwrap();
        }
        let plain = String::from_utf8(out.pop().unwrap()).unwrap();
        let s2c = out.pop().unwrap();
        (out.pop().unwrap(), s2c, plain)
    }

    async fn read(client: &mut TcpStream) -> Frame {
        use tokio::io::AsyncReadExt;
        let mut pending = Vec::new();
        timeout(Duration::from_secs(2), async {
            loop {
                if let Some(frame) = crate::protocol::scframe::try_parse(&mut pending, MAX_PAYLOAD).unwrap() {
                    return frame;
                }
                let mut chunk = [0u8; 4096];
                let n = client.read(&mut chunk).await.unwrap();
                assert!(n > 0, "connection closed before a full frame arrived");
                pending.extend_from_slice(&chunk[..n]);
            }
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn reset_sends_home_and_replays_on_the_same_tcp_connection() {
        use crate::protocol::{
            messages::{OwnHomeData, VisitPlayer},
            Message,
        };
        let (old_store, dir) = crate::storage::test_store("tcp-reset");
        drop(old_store);
        let initial = crate::game::starter::new_home().unwrap();
        let store = std::sync::Arc::new(
            crate::storage::open_file(dir.join("accounts.json"), initial.clone()).unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let path = stem("reset-home");
        let dumps = Dumps::create(&path, "test").await.unwrap();
        let server_store = store.clone();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let cfg = crate::net::session::Config {
                accounts: Some(server_store),
                account_home: true,
                dev_friends: true,
                after_ready: crate::net::session::Config::default_after_ready(),
                after_ready_delay: Duration::from_secs(60),
                ..config()
            };
            serve(stream, dumps, &cfg).await.unwrap();
        });
        let test = TestClient::new(&config().secret);
        client
            .write_all(&TestClient::hello().to_wire())
            .await
            .unwrap();
        let hello = read(&mut client).await;
        client
            .write_all(&test.login(&hello).to_wire())
            .await
            .unwrap();
        let ok = read(&mut client).await;
        let (account, mut ciphers) = test.login_ok(&ok);
        let mut changed = initial.clone();
        changed.avatar_stats.insert(1900000, 9999);
        changed.tutorial_words = [u32::MAX; 6];
        store.replace_home(account.account_id, changed).unwrap();
        // Queue an old home before reset; reset must clear this deferred send.
        client
            .write_all(&ciphers.send(10366, &[]).to_wire())
            .await
            .unwrap();
        let visit = VisitPlayer {
            selector: 0,
            target_account_id: crate::game::dev_friends::RESET_ID,
        };
        client
            .write_all(&ciphers.send(18485, &visit.to_bytes()).to_wire())
            .await
            .unwrap();
        let reply = read(&mut client).await;
        assert_eq!(reply.msg_type, 26017);
        assert_eq!(
            OwnHomeData::from_bytes(&ciphers.receive(&reply)).unwrap(),
            crate::replay::orders::prepare(initial.clone())
                .unwrap()
                .message()
        );
        assert_eq!(store.home(account.account_id).unwrap(), initial);
        // Same cipher and socket: tick 1/field 400000 belong to the fresh world.
        let turn = crate::replay::tests::turn(304, &[vec![2, 400000, 1]]);
        client
            .write_all(&ciphers.send(11292, &turn).to_wire())
            .await
            .unwrap();
        client
            .write_all(&ciphers.send(18485, &visit.to_bytes()).to_wire())
            .await
            .unwrap();
        let retry = read(&mut client).await;
        assert_eq!(retry.msg_type, 26017);
        let current = store.home(account.account_id).unwrap();
        assert_eq!(
            current.avatar_stats[&400001],
            initial.avatar_stats.get(&400001).copied().unwrap_or(0) + 2
        );
        assert_eq!(
            OwnHomeData::from_bytes(&ciphers.receive(&retry)).unwrap(),
            current.message()
        );
        let grant = VisitPlayer {
            selector: 0,
            target_account_id: crate::game::dev_friends::RESOURCES_ID,
        };
        for count in 1..=2 {
            client
                .write_all(&ciphers.send(18485, &grant.to_bytes()).to_wire())
                .await
                .unwrap();
            let reply = read(&mut client).await;
            assert_eq!(reply.msg_type, 26017);
            let home = store.home(account.account_id).unwrap();
            for stat in [1900000, 1900001] {
                assert_eq!(
                    home.avatar_stats[&stat],
                    current.avatar_stats.get(&stat).copied().unwrap_or(0) + count * 10000
                );
            }
            assert_eq!(home.c, current.c);
            assert_eq!(
                OwnHomeData::from_bytes(&ciphers.receive(&reply)).unwrap(),
                home.message()
            );
        }
        client.shutdown().await.unwrap();
        server.await.unwrap();
        let (_, _, plain) = cleanup(&path).await;
        let sent: Vec<u64> = plain
            .lines()
            .filter_map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).unwrap();
                v["type"]
                    .as_u64()
                    .filter(|t| [26017, 25892, 29950].contains(t))
            })
            .collect();
        assert_eq!(sent, [26017, 26017, 26017, 26017]);
        tokio::fs::remove_dir_all(dir).await.unwrap();
    }
    #[tokio::test]
    async fn tcp_login_post_login_and_capture() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let path = stem("login");
        let dumps = Dumps::create(&path, "test").await.unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            serve(stream, dumps, &config()).await.unwrap();
        });
        let test = TestClient::new(&config().secret);
        client
            .write_all(&TestClient::hello().to_wire())
            .await
            .unwrap();
        let hello = read(&mut client).await;
        client
            .write_all(&test.login(&hello).to_wire())
            .await
            .unwrap();
        let ok = read(&mut client).await;
        let (_, mut ciphers) = test.login_ok(&ok);
        client
            .write_all(&ciphers.send(10366, &[]).to_wire())
            .await
            .unwrap();
        let ready = read(&mut client).await;
        assert_eq!(ready.msg_type, 29997);
        assert!(ciphers.peek(&ready).is_empty());
        client
            .write_all(&ciphers.send(10099, &[]).to_wire())
            .await
            .unwrap();
        client.shutdown().await.unwrap();
        server.await.unwrap();

        let (_, s2c, plain) = cleanup(&path).await;
        let mut expected = hello.to_wire();
        expected.extend(ok.to_wire());
        expected.extend(ready.to_wire());
        assert_eq!(s2c, expected);
        let types: Vec<u64> = plain
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["type"].as_u64())
            .collect();
        assert_eq!(types, [10100, 20100, 10101, 29501, 10366, 29997, 10099]);
    }

    #[tokio::test]
    async fn capture_preserves_partial_and_nonframed_bytes() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let path = stem("capture");
        let dumps = Dumps::create(&path, "test").await.unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            serve(stream, dumps, &config()).await.unwrap();
        });
        // Synthetic bytes sent across separate writes, then developer-mode
        // AUTH bytes (which do not frame): neither framing failure nor EOF
        // may discard bytes. TCP may coalesce the writes into one read.
        let hello = b"synthetic hello bytes";
        client.write_all(&hello[..4]).await.unwrap();
        tokio::task::yield_now().await;
        client.write_all(&hello[4..]).await.unwrap();
        client.write_all(b"\0AUTH\r\n").await.unwrap();
        client.shutdown().await.unwrap();
        server.await.unwrap();
        let (c2s, ..) = cleanup(&path).await;
        let mut expected = hello.to_vec();
        expected.extend_from_slice(b"\0AUTH\r\n");
        assert_eq!(c2s, expected);
    }
}
