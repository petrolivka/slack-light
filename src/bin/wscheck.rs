//! Spike B — the web client's websocket.
//!
//! This is the single most fragile thing the project depends on and the one
//! with no documentation at all, so it gets its own probe. It answers: does
//! the handshake form still work, what does the event stream actually carry,
//! and does a reconnect need a fresh URL?
//!
//!     export SLKTUI_TEAM=myworkspace SLKTUI_TOKEN=xoxc-… SLKTUI_COOKIE=xoxd-…
//!     wscheck [seconds]        # default 90
//!
//! While it runs, do things in Slack on your phone or in the browser: post a
//! message, edit it, react, start typing, open a thread, mark a channel read.
//! Every event type is counted and one sample of each is summarised - never
//! printed whole, because these payloads are full of real message text.

use anyhow::{bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use slack_light::auth;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

/// What a payload is, without quoting what it says.
fn describe(v: &Value) -> String {
    let ty = v.get("type").and_then(Value::as_str).unwrap_or("?");
    let sub = v.get("subtype").and_then(Value::as_str);
    let ch = v.get("channel").and_then(Value::as_str).unwrap_or("-");
    let text_len = v
        .get("text")
        .and_then(Value::as_str)
        .map(str::len)
        .unwrap_or(0);
    let has_blocks = v.get("blocks").is_some();
    match sub {
        Some(s) => format!("{ty}/{s} channel={ch} text_len={text_len} blocks={has_blocks}"),
        None => format!("{ty} channel={ch} text_len={text_len} blocks={has_blocks}"),
    }
}

/// The team id for the socket URL, and our own user id for `presence_sub`.
async fn team_id(token: &str, cookie: &str, team: &str) -> Result<(String, String)> {
    let http = reqwest::Client::builder()
        .user_agent("slack-light/0.0 M0-spike")
        .build()?;
    let v: Value = http
        .post(format!("https://{team}.slack.com/api/auth.test"))
        .header("Cookie", format!("d={cookie}"))
        .form(&[("token", token)])
        .send()
        .await?
        .json()
        .await?;
    if v.get("ok").and_then(Value::as_bool) != Some(true) {
        bail!(
            "auth.test failed: {}",
            v.get("error").and_then(Value::as_str).unwrap_or("?")
        );
    }
    Ok((
        v["team_id"].as_str().unwrap_or_default().to_string(),
        v["user_id"].as_str().unwrap_or_default().to_string(),
    ))
}

/// Never print a credential: this output is meant to be pasteable.
fn redact(s: &str) -> String {
    if s.len() <= 12 {
        "…".into()
    } else {
        format!("{}…{}", &s[..8], &s[s.len() - 3..])
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let c = auth::load()?;
    let (team, token, cookie) = (c.team, c.token, c.cookie);
    let secs: u64 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(90);

    println!("slack-light spike B - the web client websocket");
    println!("  team {team}   token {}\n", redact(&token));

    let (tid, self_id) = team_id(&token, &cookie, &team).await?;
    let channel = std::env::var("SLKTUI_CHANNEL").unwrap_or_default();
    println!("B0   team_id={tid} self={self_id}");

    // The handshake form wee-slack uses. Every parameter is load-bearing:
    // `batch_presence_aware` is what makes presence_sub work at all.
    let url = format!(
        "wss://wss-primary.slack.com/?token={token}&gateway_server={tid}-1&slack_client=desktop&batch_presence_aware=1"
    );
    let mut req = url.as_str().into_client_request()?;
    req.headers_mut()
        .insert("Cookie", format!("d={cookie}").parse()?);
    req.headers_mut()
        .insert("Origin", format!("https://{team}.slack.com").parse()?);
    req.headers_mut()
        .insert("User-Agent", "slack-light/0.0 M0-spike".parse()?);

    let t = Instant::now();
    let (mut ws, res) = tokio_tungstenite::connect_async(req)
        .await
        .context("websocket handshake failed - if this is a 4xx the URL form has changed")?;
    println!(
        "B1   connected in {} ms, HTTP {}",
        t.elapsed().as_millis(),
        res.status()
    );

    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut samples: BTreeMap<String, String> = BTreeMap::new();
    let mut hello = false;
    let mut pongs = 0usize;
    let mut ping_id = 1000i64;
    let mut ping_sent: Option<Instant> = None;
    let mut rtt = Vec::new();

    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut ping = tokio::time::interval(Duration::from_secs(10));
    println!("     listening {secs}s - go and do things in Slack now\n");

    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline.into()) => break,
            _ = ping.tick() => {
                // B2. emacs-slack pings every 10s with a 20s timeout; a silent
                // socket that is still "open" is the normal failure mode here.
                ping_id += 1;
                ping_sent = Some(Instant::now());
                ws.send(Message::Text(format!(r#"{{"id":{ping_id},"type":"ping"}}"#).into())).await?;
            }
            msg = ws.next() => {
                let Some(msg) = msg else { println!("\n!! socket closed by the server"); break };
                match msg? {
                    Message::Text(t) => {
                        let Ok(v) = serde_json::from_str::<Value>(&t) else { continue };
                        let ty = v.get("type").and_then(Value::as_str).unwrap_or_else(|| {
                            if v.get("reply_to").is_some() { "«reply»" } else { "«untyped»" }
                        }).to_string();
                        let key = match v.get("subtype").and_then(Value::as_str) {
                            Some(s) => format!("{ty}/{s}"),
                            None => ty.clone(),
                        };
                        *kinds.entry(key.clone()).or_default() += 1;
                        samples.entry(key).or_insert_with(|| describe(&v));

                        if ty == "hello" && !hello {
                            hello = true;
                            println!("B1   hello received - the stream is live");
                            // B7. Presence is one of the parity claims the
                            // official route cannot make, so it gets asked for
                            // explicitly rather than hoped for.
                            let sub = format!(r#"{{"type":"presence_sub","ids":["{self_id}"]}}"#);
                            let _ = ws.send(Message::Text(sub.into())).await;
                            // And a typing frame, to see whether the server
                            // echoes it back to the sender's other clients.
                            let _ = ws
                                .send(Message::Text(
                                    format!(r#"{{"id":9001,"type":"typing","channel":"{channel}"}}"#).into(),
                                ))
                                .await;
                        }
                        if ty == "pong" || v.get("reply_to").and_then(Value::as_i64) == Some(ping_id) {
                            pongs += 1;
                            if let Some(s) = ping_sent.take() {
                                rtt.push(s.elapsed().as_millis());
                            }
                        }
                    }
                    Message::Ping(p) => ws.send(Message::Pong(p)).await?,
                    Message::Close(c) => { println!("\n!! close frame: {c:?}"); break }
                    _ => {}
                }
            }
        }
    }

    println!("\n== event types seen in {secs}s ==");
    for (k, n) in &kinds {
        println!(
            "  {n:>4}  {k:<32} {}",
            samples.get(k).map(String::as_str).unwrap_or("")
        );
    }
    if kinds.is_empty() {
        println!("  (none - if you did not touch Slack while it ran, that is expected)");
    }
    let avg = if rtt.is_empty() {
        0
    } else {
        rtt.iter().sum::<u128>() / rtt.len() as u128
    };
    println!("\n  hello={hello}  pongs={pongs}  ping rtt avg={avg} ms");

    // B8: the URL is single-use. Reusing it is the mistake a naive reconnect
    // makes, and it fails in a way that looks like an auth problem.
    println!("\nB8   reusing the same URL for a second connection...");
    let mut req2 = url.as_str().into_client_request()?;
    req2.headers_mut()
        .insert("Cookie", format!("d={cookie}").parse()?);
    match tokio_tungstenite::connect_async(req2).await {
        Ok(_) => println!("     accepted - the URL is reusable"),
        Err(e) => println!("     rejected ({e}) - a reconnect must fetch a fresh URL"),
    }

    Ok(())
}
