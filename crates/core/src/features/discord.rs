//! Sending alerts to Discord: through a channel webhook (simplest: no bot needed), or through the user's own bot to
//! a channel or as a direct message.
//!
//! The webhook URL and bot token are secrets, so they live in their own file (`discord.json`, readable only by the
//! user) rather than in `settings.json`. Which alerts go to Discord is part of the alert settings
//! (`features::alerts`).

use super::alerts::{Condition, ALERT_TYPES};
use crate::{config, store};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const API: &str = "https://discord.com/api/v10";

/// Discord's API address (a local stand-in in tests).
fn api() -> String {
    #[cfg(test)]
    if let Ok(a) = std::env::var("PODSTACK_TEST_DISCORD_API") {
        return a;
    }
    API.to_string()
}
/// Discord allows at most this many embeds in one message.
const MAX_EMBEDS: usize = 10;

/// How messages get to Discord.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// A channel's webhook URL (Channel settings → Integrations → Webhooks).
    #[default]
    Webhook,
    /// The user's own bot, posting in a channel.
    BotChannel,
    /// The user's own bot, sending a direct message.
    BotDm,
}

impl Method {
    pub const ALL: [Method; 3] = [Method::Webhook, Method::BotChannel, Method::BotDm];
    pub fn label(self) -> &'static str {
        match self {
            Method::Webhook => "Channel webhook",
            Method::BotChannel => "Bot in a channel",
            Method::BotDm => "Bot direct message",
        }
    }
}

/// The last delivery, for the settings page.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LastResult {
    pub time: DateTime<Utc>,
    pub ok: bool,
    pub message: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DiscordConfig {
    pub method: Method,
    pub webhook_url: String,
    pub bot_token: String,
    /// Channel id for `BotChannel` (Developer Mode → right-click the channel → Copy Channel ID).
    pub channel_id: String,
    /// User id for `BotDm` (right-click yourself → Copy User ID).
    pub user_id: String,
    /// Put in front of every message, e.g. "@here" or "<@&role id>"; empty for none.
    pub mention: String,
    /// Shows the character's portrait next to each alert.
    pub portraits: bool,
    pub last: Option<LastResult>,
}

impl DiscordConfig {
    /// Whether enough is filled in to try sending.
    pub fn is_complete(&self) -> bool {
        match self.method {
            Method::Webhook => self.webhook_url.starts_with("https://") && self.webhook_url.contains("/api/webhooks/"),
            Method::BotChannel => !self.bot_token.trim().is_empty() && is_id(&self.channel_id),
            Method::BotDm => !self.bot_token.trim().is_empty() && is_id(&self.user_id),
        }
    }
}

fn is_id(s: &str) -> bool {
    let s = s.trim();
    s.len() >= 15 && s.chars().all(|c| c.is_ascii_digit())
}

fn path() -> std::path::PathBuf {
    store::data_dir().join("discord.json")
}

pub fn load() -> DiscordConfig {
    store::read_json(&path()).unwrap_or(DiscordConfig { portraits: true, ..Default::default() })
}

pub fn save(c: &DiscordConfig) {
    if let Err(e) = store::write_json(&path(), c, true) {
        tracing::error!("saving discord.json: {e}");
    }
}

/// The colour of an alert's embed: red for things that stop (training, extractors, couriers due), green for
/// things done, blue otherwise.
fn color(kind: &str) -> u32 {
    match kind {
        "queue_empty" | "omega_lapsed" | "courier_due" | "extractor_expired" | "login_expired" => 0xE5484D,
        "queue_low" | "calendar_event" | "contract_assigned" | "scopes_missing" => 0xF5A524,
        "injector_ready" | "industry_ready" | "contract_finished" | "market_order" | "fatigue_ended" => 0x30A46C,
        _ => 0x3E8ED0,
    }
}

fn label(kind: &str) -> &'static str {
    ALERT_TYPES.iter().find(|t| t.id == kind).map(|t| t.label).unwrap_or("Alert")
}

/// The embeds for a batch of alerts (the last one sums up any that don't fit).
pub fn embeds(alerts: &[Condition], portraits: bool, now: DateTime<Utc>) -> Vec<Value> {
    let embed = |c: &Condition| {
        let mut e = json!({
            "title": c.title,
            "description": c.body,
            "color": color(c.kind),
            "footer": { "text": format!("PodStack · {}", label(c.kind)) },
            "timestamp": now.to_rfc3339(),
        });
        if portraits && c.character_id > 0 {
            e["thumbnail"] = json!({ "url": config::character_portrait(c.character_id, 64) });
        }
        e
    };
    if alerts.len() <= MAX_EMBEDS {
        return alerts.iter().map(embed).collect();
    }
    let mut out: Vec<Value> = alerts[..MAX_EMBEDS - 1].iter().map(embed).collect();
    let rest = &alerts[MAX_EMBEDS - 1..];
    let lines: Vec<String> = rest.iter().take(20).map(|c| format!("• {}", c.title)).collect();
    let more = rest.len().saturating_sub(20);
    out.push(json!({
        "title": format!("{} more alerts", rest.len()),
        "description": format!("{}{}", lines.join("\n"), if more > 0 { format!("\n…and {more} more") } else { String::new() }),
        "color": 0x3E8ED0,
        "timestamp": now.to_rfc3339(),
    }));
    out
}

/// The message body: the mention, the embeds, and (for webhooks) PodStack's name and icon.
pub fn message(cfg: &DiscordConfig, embeds: Vec<Value>) -> Value {
    let mut m = json!({ "embeds": embeds, "allowed_mentions": { "parse": ["everyone", "roles", "users"] } });
    if !cfg.mention.trim().is_empty() {
        m["content"] = json!(cfg.mention.trim());
    }
    if cfg.method == Method::Webhook {
        m["username"] = json!(config::APP_NAME);
        m["avatar_url"] = json!(ICON_URL);
    }
    m
}

/// PodStack's icon where Discord can fetch it, for the webhook's avatar.
const ICON_URL: &str = "https://raw.githubusercontent.com/tylort69/PodStack/rust-rewrite/assets/icon.png";

/// Posts a message, waiting out one rate limit if Discord asks. Errors are messages for the user.
async fn post(http: &reqwest::Client, url: &str, bot: Option<&str>, body: &Value) -> Result<Value, String> {
    for attempt in 0..2 {
        let mut req = http.post(url).json(body);
        if let Some(token) = bot {
            req = req.header("Authorization", format!("Bot {}", token.trim()));
        }
        let res = req.send().await.map_err(|e| format!("Couldn't reach Discord: {e}"))?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if status.as_u16() == 429 && attempt == 0 {
            let wait = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["retry_after"].as_f64()).unwrap_or(1.0);
            tokio::time::sleep(std::time::Duration::from_secs_f64(wait.clamp(0.1, 10.0))).await;
            continue;
        }
        if status.is_success() {
            return Ok(serde_json::from_str(&text).unwrap_or(Value::Null));
        }
        let detail = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["message"].as_str().map(String::from)).unwrap_or(text);
        return Err(match status.as_u16() {
            401 => "Discord refused the bot token: check it in the Developer Portal (Bot → Reset Token).".into(),
            403 => format!("The bot isn't allowed to post there ({detail}). For a channel, give it Send Messages and Embed Links; for a DM, it must share a server with you and you must allow DMs from server members."),
            404 => "Discord can't find that webhook or channel: check the URL or id.".into(),
            _ => format!("Discord answered {status}: {detail}"),
        });
    }
    Err("Discord is rate limiting PodStack; it will try again with the next alerts.".into())
}

/// Sends a message to wherever the config points.
pub async fn send(http: &reqwest::Client, cfg: &DiscordConfig, body: &Value) -> Result<(), String> {
    if !cfg.is_complete() {
        return Err("Discord isn't set up: fill in the webhook URL, or the bot token and the channel or user id.".into());
    }
    match cfg.method {
        Method::Webhook => post(http, &format!("{}?wait=true", cfg.webhook_url.trim()), None, body).await.map(|_| ()),
        Method::BotChannel => post(http, &format!("{}/channels/{}/messages", api(), cfg.channel_id.trim()), Some(&cfg.bot_token), body).await.map(|_| ()),
        Method::BotDm => {
            let dm = post(http, &format!("{}/users/@me/channels", api()), Some(&cfg.bot_token), &json!({ "recipient_id": cfg.user_id.trim() })).await?;
            let channel = dm["id"].as_str().ok_or("Discord didn't open a DM channel.")?;
            post(http, &format!("{}/channels/{channel}/messages", api()), Some(&cfg.bot_token), body).await.map(|_| ())
        }
    }
}

/// Sends alerts and records how it went (shown in Settings).
pub async fn deliver(http: &reqwest::Client, alerts: &[Condition]) -> Result<(), String> {
    if alerts.is_empty() {
        return Ok(());
    }
    let mut cfg = load();
    let now = Utc::now();
    let body = message(&cfg, embeds(alerts, cfg.portraits, now));
    let r = send(http, &cfg, &body).await;
    let n = alerts.len();
    cfg.last = Some(LastResult {
        time: now,
        ok: r.is_ok(),
        message: match &r {
            Ok(()) => format!("Sent {n} alert{}", if n == 1 { "" } else { "s" }),
            Err(e) => e.clone(),
        },
    });
    // keep what the user may have changed meanwhile: only the result is ours to write
    let mut latest = load();
    latest.last = cfg.last;
    save(&latest);
    r
}

/// Settings → Alerts → Discord → Send test.
pub async fn test(http: &reqwest::Client, cfg: &DiscordConfig) -> Result<(), String> {
    let now = Utc::now();
    let embed = json!({
        "title": "PodStack alerts are working",
        "description": "Alerts you send to Discord will arrive here like this.",
        "color": 0x30A46C,
        "footer": { "text": "PodStack · test" },
        "timestamp": now.to_rfc3339(),
    });
    send(http, cfg, &message(cfg, vec![embed])).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cond(kind: &'static str, n: i64) -> Condition {
        Condition { key: format!("{kind}:{n}"), kind, character_id: 90000000 + n, title: format!("Alert {n}"), body: "Body".into() }
    }

    /// A stand-in for Discord: answers each request with the next canned (status, body) and records what came in.
    async fn fake_discord(replies: Vec<(u16, &'static str)>) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            for (status, body) in replies {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 65536];
                let mut req = String::new();
                // read headers and the body Content-Length announces
                loop {
                    let n = sock.read(&mut buf).await.unwrap();
                    req.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if let Some(h) = req.find("\r\n\r\n") {
                        let len = req[..h]
                            .lines()
                            .find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                            .unwrap_or(0);
                        if req.len() >= h + 4 + len {
                            break;
                        }
                    }
                    if n == 0 {
                        break;
                    }
                }
                log.lock().unwrap().push(req);
                let reply = format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                sock.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        (addr, seen)
    }

    #[tokio::test]
    async fn sends_through_a_webhook_and_waits_out_rate_limits() {
        let (addr, seen) = fake_discord(vec![(429, r#"{"retry_after":0.05}"#), (200, "{}")]).await;
        let http = reqwest::Client::new();
        let body = json!({"content": "hi"});
        post(&http, &format!("{addr}/api/webhooks/1/x?wait=true"), None, &body).await.unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "retried once after the rate limit");
        assert!(seen[1].starts_with("POST /api/webhooks/1/x?wait=true"));
        assert!(seen[1].contains(r#""content":"hi""#));
        assert!(!seen[1].to_lowercase().contains("authorization"));
    }

    #[tokio::test]
    async fn bot_dm_opens_a_channel_then_posts() {
        let (addr, seen) = fake_discord(vec![(200, r#"{"id":"555"}"#), (200, "{}"), (401, r#"{"message":"401: Unauthorized"}"#)]).await;
        // SAFETY: only this test reads the variable
        unsafe { std::env::set_var("PODSTACK_TEST_DISCORD_API", &addr) };
        let http = reqwest::Client::new();
        let cfg = DiscordConfig { method: Method::BotDm, bot_token: "tok".into(), user_id: "123456789012345678".into(), ..Default::default() };
        test(&http, &cfg).await.unwrap();
        {
            let seen = seen.lock().unwrap();
            assert!(seen[0].starts_with("POST /users/@me/channels") && seen[0].contains("123456789012345678"));
            assert!(seen[0].contains("Bot tok"));
            assert!(seen[1].starts_with("POST /channels/555/messages") && seen[1].contains("PodStack alerts are working"));
        }
        let bad = DiscordConfig { method: Method::BotChannel, channel_id: "123456789012345678".into(), ..cfg };
        let err = test(&http, &bad).await.unwrap_err();
        assert!(err.contains("bot token"), "{err}");
    }

    #[test]
    fn config_completeness() {
        let mut c = DiscordConfig::default();
        assert!(!c.is_complete());
        c.webhook_url = "https://discord.com/api/webhooks/123/abc".into();
        assert!(c.is_complete());
        c.method = Method::BotDm;
        assert!(!c.is_complete());
        c.bot_token = "token".into();
        c.user_id = "123456789012345678".into();
        assert!(c.is_complete());
        c.user_id = "not an id".into();
        assert!(!c.is_complete());
    }

    #[test]
    fn embeds_fit_discord_limits() {
        let now = Utc::now();
        let few: Vec<Condition> = (0..3).map(|n| cond("queue_empty", n)).collect();
        let e = embeds(&few, true, now);
        assert_eq!(e.len(), 3);
        assert_eq!(e[0]["color"], 0xE5484D);
        assert!(e[0]["thumbnail"]["url"].as_str().unwrap().contains("90000000"));
        assert_eq!(embeds(&few, false, now)[0].get("thumbnail"), None);

        let many: Vec<Condition> = (0..25).map(|n| cond("new_mail", n)).collect();
        let e = embeds(&many, true, now);
        assert_eq!(e.len(), MAX_EMBEDS);
        assert_eq!(e[9]["title"], "16 more alerts");

        let cfg = DiscordConfig { mention: "@here".into(), ..Default::default() };
        let m = message(&cfg, e);
        assert_eq!(m["content"], "@here");
        assert_eq!(m["username"], "PodStack");
        let bot = DiscordConfig { method: Method::BotChannel, ..Default::default() };
        assert_eq!(message(&bot, vec![]).get("username"), None);
    }
}
