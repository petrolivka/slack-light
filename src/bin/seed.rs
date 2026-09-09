//! Spike A, part two — populate the test workspace, then capture what Slack
//! actually returns.
//!
//! The fixture corpus committed in `fixtures/messages.json` is hand-written
//! from the documentation. That is enough to build a parser against and not
//! enough to trust it: the shapes that break clients are the ones nobody
//! documents. This posts a known corpus into a channel of a workspace created
//! for the purpose, then captures the real responses with tracking fields and
//! personal data stripped, so the parsers can be asserted against reality.
//!
//!     seed --post                # create #fixtures and fill it
//!     seed --dump                # capture responses into fixtures/real/
//!     seed --post --dump         # both, in order
//!
//! Refuses to run against a workspace whose domain does not look like a test
//! one, because the whole point is that this writes a lot of messages.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use slack_light::auth;
use std::path::Path;

struct Api {
    http: reqwest::Client,
    base: String,
    token: String,
    cookie: String,
}

impl Api {
    async fn call(&self, method: &str, form: &[(&str, &str)]) -> Result<Value> {
        let mut body = form.to_vec();
        body.push(("token", &self.token));
        let v: Value = self
            .http
            .post(format!("{}/{method}", self.base))
            .header("Cookie", format!("d={}", self.cookie))
            .form(&body)
            .send()
            .await?
            .json()
            .await?;
        Ok(v)
    }

    async fn ok_call(&self, method: &str, form: &[(&str, &str)]) -> Result<Value> {
        let v = self.call(method, form).await?;
        if v.get("ok").and_then(Value::as_bool) != Some(true) {
            bail!(
                "{method}: {}",
                v.get("error").and_then(Value::as_str).unwrap_or("?")
            );
        }
        Ok(v)
    }

    /// Slack's posting limit is about one message per second per channel, and
    /// this posts a few dozen. Going slower than the limit is the difference
    /// between a client and something that looks like a load test.
    async fn post(
        &self,
        channel: &str,
        text: &str,
        blocks: Option<&Value>,
        thread: Option<&str>,
    ) -> Result<String> {
        let blocks_s = blocks.map(|b| b.to_string());
        let mut form: Vec<(&str, &str)> = vec![("channel", channel), ("text", text)];
        if let Some(b) = &blocks_s {
            form.push(("blocks", b));
        }
        if let Some(t) = thread {
            form.push(("thread_ts", t));
        }
        let v = self.ok_call("chat.postMessage", &form).await?;
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        Ok(v["ts"].as_str().unwrap_or_default().to_string())
    }
}

/// Everything that is either a per-session identifier or somebody's personal
/// data. Stripped before anything is written to disk, because these files are
/// committed and read by strangers.
const STRIP: &[&str] = &[
    "client_msg_id",
    "team",
    "user_team",
    "source_team",
    "user_profile",
    "profile",
    "email",
    "phone",
    "skype",
    "real_name",
    "real_name_normalized",
    "display_name_normalized",
    "image_24",
    "image_32",
    "image_48",
    "image_72",
    "image_192",
    "image_512",
    "image_1024",
    "image_original",
    "avatar_hash",
    "enterprise_user",
    "who_can_share_contact_card",
    "url_private",
    "url_private_download",
    "permalink",
    "permalink_public",
    "thumb_64",
    "thumb_80",
    "thumb_160",
    "thumb_360",
    "thumb_480",
    "thumb_720",
    "thumb_960",
    "thumb_1024",
    "thumb_tiny",
    "edit_link",
    "external_id",
];

fn scrub(v: &mut Value) {
    match v {
        Value::Object(map) => {
            map.retain(|k, _| !STRIP.contains(&k.as_str()));
            for (_, x) in map.iter_mut() {
                scrub(x);
            }
        }
        Value::Array(xs) => xs.iter_mut().for_each(scrub),
        _ => {}
    }
}

async fn seed_channel(api: &Api, channel: &str) -> Result<()> {
    println!("posting the corpus into {channel} (about a minute, one message per 1.2 s)\n");

    // The text-shaped half: every mrkdwn construct, one message each, so a
    // parser failure points at exactly one feature.
    let plain = [
        "*bold* _italic_ ~struck~ `code` and *_bold italic_*",
        "cc <!here> — see <https://example.com/a?x=1&amp;y=2|the ticket>",
        "logs: <https://grafana.internal.example.com/d/abcd1234/service-overview?orgId=1&from=now-6h&to=now&var-cluster=prod-eu-west-1&var-namespace=payments&var-pod=All&refresh=30s>",
        "the failing bit:\n```\nfn main() {\n    let very_long_identifier_name = compute(with, several, arguments);\n}\n```\nthoughts?",
        "> we should roll back\n> and re-run tomorrow\nagreed",
        "nice :tada: :+1::skin-tone-3: :thinking_face: :this_is_not_a_real_emoji:",
        "freeze starts <!date^1725840000^{date_short} at {time}|Sep 9 at 12:00 PM>",
        "デプロイが完了しました。マイグレーションも含まれています。",
        "OK 好的 확인 ＡＢＣ done ✅ (mixed widths on one line)",
        "widths: 👍 👍🏽 👨‍👩‍👧‍👦 ❤️ ☺ 🇨🇿 🏳️‍🌈 1️⃣ ✅ ⚠️",
        "5 &lt; 6 &amp;&amp; 7 &gt; 6",
        "• one\n• two\n• three",
    ];
    for t in plain {
        api.post(channel, t, None, None).await?;
    }
    println!("  {} text messages", plain.len());

    // The Block Kit half, posted as an app would.
    let deploy = json!([
        { "type": "header", "text": { "type": "plain_text", "text": "Build #4412 passed" } },
        { "type": "section",
          "text": { "type": "mrkdwn", "text": "*payments-service* on `main`" },
          "fields": [
            { "type": "mrkdwn", "text": "*Duration*\n3m 12s" },
            { "type": "mrkdwn", "text": "*Coverage*\n87.4% (+0.3)" },
            { "type": "mrkdwn", "text": "*Tests*\n1249 passed" },
            { "type": "mrkdwn", "text": "*Artifacts*\n3" }
          ] },
        { "type": "context", "elements": [
            { "type": "mrkdwn", "text": "commit `a3c8d28`" },
            { "type": "plain_text", "text": "eu-west-1" } ] },
        { "type": "divider" },
        { "type": "actions", "elements": [
            { "type": "button", "text": { "type": "plain_text", "text": "Open build" },
              "url": "https://example.com/4412", "style": "primary" },
            { "type": "button", "text": { "type": "plain_text", "text": "Rerun" }, "action_id": "rerun" } ] }
    ]);
    api.post(channel, "Build #4412 passed", Some(&deploy), None)
        .await?;
    println!("  1 Block Kit message");

    // A thread with replies and a reaction, so reply_count, reply_users,
    // latest_reply and the reactions array all have real values to parse.
    let parent = api
        .post(
            channel,
            "Deploy of v2.4 finished :white_check_mark: — details in the thread",
            None,
            None,
        )
        .await?;
    for r in [
        "was the migration included?",
        "yes, it ran in step 3",
        "thanks!",
    ] {
        api.post(channel, r, None, Some(&parent)).await?;
    }
    for name in ["tada", "eyes", "rocket", "+1"] {
        let _ = api
            .call(
                "reactions.add",
                &[("channel", channel), ("timestamp", &parent), ("name", name)],
            )
            .await;
    }
    println!("  1 thread with 3 replies and 4 reactions");

    // An edited message, because `message_changed` and the `edited` field are
    // their own shape.
    let ts = api
        .post(channel, "the retry limit is 3", None, None)
        .await?;
    api.ok_call(
        "chat.update",
        &[
            ("channel", channel),
            ("ts", &ts),
            ("text", "the retry limit is now *5*, not 3"),
        ],
    )
    .await?;
    println!("  1 edited message");

    println!("\ndone. Run with --dump to capture the responses.");
    Ok(())
}

async fn dump(api: &Api, channel: &str, out: &Path) -> Result<()> {
    std::fs::create_dir_all(out)?;
    println!("capturing into {}\n", out.display());

    let targets: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("auth.test", vec![]),
        ("client.userBoot", vec![("_x_reason", "initial-data")]),
        ("client.counts", vec![("thread_counts_by_channel", "true")]),
        ("conversations.info", vec![("channel", channel)]),
        (
            "conversations.history",
            vec![("channel", channel), ("limit", "50")],
        ),
        ("users.list", vec![("limit", "30")]),
        ("emoji.list", vec![]),
        (
            "search.messages",
            vec![("query", "deploy"), ("count", "10")],
        ),
        ("drafts.list", vec![]),
        ("saved.list", vec![]),
        ("users.prefs.get", vec![]),
        ("dnd.info", vec![]),
        ("bookmarks.list", vec![("channel_id", channel)]),
    ];

    let mut thread_ts = None;
    for (method, form) in &targets {
        let v = api.call(method, form).await?;
        let good = v.get("ok").and_then(Value::as_bool) == Some(true);
        if method == &"conversations.history" {
            thread_ts = v.get("messages").and_then(Value::as_array).and_then(|ms| {
                ms.iter()
                    .find(|m| m.get("reply_count").and_then(Value::as_u64).unwrap_or(0) > 0)
                    .and_then(|m| m.get("ts"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
        }
        let mut scrubbed = v.clone();
        scrub(&mut scrubbed);
        let path = out.join(format!("{}.json", method.replace('.', "_")));
        std::fs::write(&path, serde_json::to_string_pretty(&scrubbed)?)?;
        let size = std::fs::metadata(&path)?.len();
        println!(
            "  {:<24} {}  {:>7} bytes{}",
            method,
            if good { "ok  " } else { "FAIL" },
            size,
            if good {
                String::new()
            } else {
                format!(
                    "  ({})",
                    v.get("error").and_then(Value::as_str).unwrap_or("?")
                )
            }
        );
    }

    if let Some(t) = thread_ts {
        let v = api
            .call("conversations.replies", &[("channel", channel), ("ts", &t)])
            .await?;
        let mut scrubbed = v.clone();
        scrub(&mut scrubbed);
        std::fs::write(
            out.join("conversations_replies.json"),
            serde_json::to_string_pretty(&scrubbed)?,
        )?;
        println!("  {:<24} ok", "conversations.replies");
    }

    println!(
        "\nEvery file was stripped of {} identifier and profile fields.",
        STRIP.len()
    );
    println!("Read them before committing: a capture from a signed-in session is personal data.");
    Ok(())
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
    let args: Vec<String> = std::env::args().collect();
    let do_post = args.iter().any(|a| a == "--post");
    let do_dump = args.iter().any(|a| a == "--dump");
    if !do_post && !do_dump {
        println!("usage: seed [--post] [--dump]\n  --post  create #fixtures and fill it with the corpus\n  --dump  capture responses into fixtures/real/");
        return Ok(());
    }

    let c = auth::load()?;
    // This writes dozens of messages. It must not be pointed at a workspace
    // where that would be somebody's Tuesday.
    if !c.team.contains("dev")
        && !c.team.contains("test")
        && !args.iter().any(|a| a == "--i-mean-it")
    {
        bail!(
            "team {:?} does not look like a test workspace.\n\
             This posts dozens of messages and is meant for a workspace created for the purpose.\n\
             Pass --i-mean-it if you are certain.",
            c.team
        );
    }

    let api = Api {
        http: reqwest::Client::builder()
            .user_agent("slack-light/0.0 (+https://github.com/petrolivka/slack-light) M0-spike")
            .gzip(true)
            .timeout(std::time::Duration::from_secs(30))
            .build()?,
        base: format!("https://{}.slack.com/api", c.team),
        token: c.token.clone(),
        cookie: c.cookie.clone(),
    };

    let who = api
        .ok_call("auth.test", &[])
        .await
        .context("auth.test failed - check the credentials")?;
    println!(
        "slack-light spike A2 - seed\n  team {} user {} token {}\n",
        who["team"].as_str().unwrap_or("?"),
        who["user"].as_str().unwrap_or("?"),
        redact(&c.token)
    );

    // Find or create the channel.
    // A channel may be named on the command line; otherwise #fixtures is found
    // or created.
    let named = std::env::args()
        .skip(1)
        .find(|a| a.starts_with('C') && a.len() > 8);
    let channel = match named {
        Some(ch) => ch,
        None => {
            let list = api
                .ok_call(
                    "conversations.list",
                    &[("types", "public_channel"), ("limit", "200")],
                )
                .await?;
            let found = list
                .get("channels")
                .and_then(Value::as_array)
                .and_then(|cs| {
                    cs.iter()
                        .find(|c| c.get("name").and_then(Value::as_str) == Some("fixtures"))
                        .and_then(|c| c.get("id"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                });
            match found {
                Some(id) => {
                    println!("using existing #fixtures ({id})");
                    id
                }
                None => {
                    let v = api
                        .ok_call("conversations.create", &[("name", "fixtures")])
                        .await?;
                    let id = v["channel"]["id"].as_str().unwrap_or_default().to_string();
                    println!("created #fixtures ({id})");
                    id
                }
            }
        }
    };
    // Posting to a channel you are not in fails with not_in_channel.
    let _ = api
        .call("conversations.join", &[("channel", &channel)])
        .await;

    if do_post {
        seed_channel(&api, &channel).await?;
    }
    if do_dump {
        dump(&api, &channel, Path::new("fixtures/real")).await?;
    }
    Ok(())
}
