//! EVE mail, mail labels and mailing lists.
//!
//! Headers, labels and lists come in one refresh (they share a scope and the headers need the other two for their
//! names). Bodies are fetched when a mail is opened ([`fetch_body`]) and kept in memory for the session.

use super::{Cx, DomainResult, DomainSpec};
use crate::domain;
use crate::esi::EsiError;
use crate::features::comms::{self, MailBody};
use crate::Core;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

pub const SCOPE: &str = "esi-mail.read_mail.v1";

pub const DOMAINS: &[DomainSpec] = &[domain!("mails", "EVE Mail", 3600, refresh_mail)];

/// Label 2 is the outbox: mail the character sent.
pub const LABEL_SENT: i64 = 2;
/// ESI gives at most 50 headers per request; older ones page with `last_mail_id`.
const BATCH: usize = 50;
const MAX_BATCHES: usize = 4;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Recipient {
    pub recipient_id: i64,
    /// "character", "corporation", "alliance" or "mailing_list"
    pub recipient_type: String,
    #[serde(default)]
    pub recipient_name: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Mail {
    pub mail_id: i64,
    pub from: i64,
    pub from_name: String,
    /// "character" unless the sender is a mailing list (list "welcome" mails).
    #[serde(default)]
    pub from_type: String,
    pub subject: String,
    pub timestamp: DateTime<Utc>,
    /// EVE's own read state.
    #[serde(default)]
    pub is_read: bool,
    #[serde(default)]
    pub labels: Vec<i64>,
    #[serde(default)]
    pub label_names: Vec<String>,
    #[serde(default)]
    pub recipients: Vec<Recipient>,
}

impl Mail {
    /// Sent by the character (in its outbox).
    pub fn is_sent(&self) -> bool {
        self.labels.contains(&LABEL_SENT)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct MailLabel {
    pub label_id: i64,
    pub name: String,
    /// "#rrggbb"
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub unread_count: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct MailingList {
    pub mailing_list_id: i64,
    pub name: String,
}

/// The newest mail headers (newest first), with the character's labels and mailing lists.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MailData {
    pub mails: Vec<Mail>,
    #[serde(default)]
    pub labels: Vec<MailLabel>,
    #[serde(default)]
    pub lists: Vec<MailingList>,
    #[serde(default)]
    pub total_unread: Option<i64>,
}

impl MailData {
    pub fn label(&self, id: i64) -> Option<&MailLabel> {
        self.labels.iter().find(|l| l.label_id == id)
    }
}

#[derive(Deserialize)]
struct RawRecipient {
    recipient_id: i64,
    recipient_type: String,
}

#[derive(Deserialize)]
struct RawMail {
    mail_id: i64,
    #[serde(default)]
    from: i64,
    #[serde(default)]
    subject: String,
    timestamp: DateTime<Utc>,
    #[serde(default)]
    is_read: bool,
    #[serde(default)]
    labels: Vec<i64>,
    #[serde(default)]
    recipients: Vec<RawRecipient>,
}

#[derive(Deserialize, Default)]
struct RawLabels {
    #[serde(default)]
    labels: Vec<MailLabel>,
    #[serde(default)]
    total_unread_count: Option<i64>,
}

/// Ids universe/names can resolve: senders and character/corporation/alliance recipients, but no mailing lists
/// (universe/names doesn't know them, and one unknown id fails a whole lookup).
fn nameable_ids(raw: &[RawMail], lists: &[MailingList]) -> HashSet<i64> {
    let list_ids: HashSet<i64> = lists
        .iter()
        .map(|l| l.mailing_list_id)
        .chain(raw.iter().flat_map(|m| m.recipients.iter().filter(|r| r.recipient_type == "mailing_list").map(|r| r.recipient_id)))
        .collect();
    raw.iter()
        .flat_map(|m| {
            std::iter::once(m.from).chain(
                m.recipients
                    .iter()
                    .filter(|r| matches!(r.recipient_type.as_str(), "character" | "corporation" | "alliance"))
                    .map(|r| r.recipient_id),
            )
        })
        .filter(|id| *id > 0 && !list_ids.contains(id))
        .collect()
}

fn build(raw: Vec<RawMail>, labels: RawLabels, lists: Vec<MailingList>, names: &HashMap<i64, String>) -> MailData {
    let list_name = |id: i64| lists.iter().find(|l| l.mailing_list_id == id).map(|l| l.name.clone());
    let mut mails: Vec<Mail> = raw
        .into_iter()
        .map(|m| {
            // a mailing list's welcome mail comes "from" the list itself
            let from_list = m.recipients.first().map(|r| r.recipient_type == "mailing_list" && r.recipient_id == m.from).unwrap_or(false)
                || list_name(m.from).is_some();
            let from_name = if from_list {
                list_name(m.from).unwrap_or_else(|| "Welcome ML".into())
            } else {
                names.get(&m.from).cloned().unwrap_or_else(|| "*Unknown*".into())
            };
            let recipients = m
                .recipients
                .into_iter()
                .map(|r| {
                    let recipient_name = match r.recipient_type.as_str() {
                        "mailing_list" => list_name(r.recipient_id).unwrap_or_else(|| "*Unknown Mailing List*".into()),
                        _ => names.get(&r.recipient_id).cloned().unwrap_or_else(|| "*Unknown*".into()),
                    };
                    Recipient { recipient_id: r.recipient_id, recipient_type: r.recipient_type, recipient_name }
                })
                .collect();
            let label_names = m.labels.iter().filter_map(|id| labels.labels.iter().find(|l| l.label_id == *id)).map(|l| l.name.clone()).collect();
            Mail {
                mail_id: m.mail_id,
                from: m.from,
                from_name,
                from_type: if from_list { "mailing_list".into() } else { "character".into() },
                subject: comms::decode_entities(&m.subject),
                timestamp: m.timestamp,
                is_read: m.is_read,
                labels: m.labels,
                label_names,
                recipients,
            }
        })
        .collect();
    mails.sort_by(|a, b| b.timestamp.cmp(&a.timestamp).then(b.mail_id.cmp(&a.mail_id)));
    mails.dedup_by_key(|m| m.mail_id);
    MailData { mails, labels: labels.labels, lists, total_unread: labels.total_unread_count }
}

async fn refresh_mail(cx: &Cx) -> DomainResult {
    let labels: RawLabels = cx.get(cx.char_path("mail/labels")).scope(SCOPE).json().await?;
    let lists: Vec<MailingList> = cx.get(cx.char_path("mail/lists")).scope(SCOPE).json().await?;
    let mut raw: Vec<RawMail> = Vec::new();
    for _ in 0..MAX_BATCHES {
        let mut req = cx.get(cx.char_path("mail")).scope(SCOPE);
        if let Some(last) = raw.iter().map(|m| m.mail_id).min() {
            req = req.query("last_mail_id", last);
        }
        let batch: Vec<RawMail> = req.json().await?;
        let full = batch.len() >= BATCH;
        raw.extend(batch);
        if !full {
            break;
        }
    }
    let names = cx.universe().names(cx.esi(), nameable_ids(&raw, &lists)).await;
    let data = build(raw, labels, lists, &names);
    Ok(Box::new(move |c| c.mail = Some(data)))
}

#[derive(Deserialize)]
struct RawBody {
    #[serde(default)]
    body: String,
}

static BODIES: LazyLock<Mutex<HashMap<i64, MailBody>>> = LazyLock::new(Default::default);

/// A mail's body, converted for display. Fetched once per session with the character's token.
pub async fn fetch_body(core: &Core, character_id: i64, mail_id: i64) -> Result<MailBody, EsiError> {
    if let Some(b) = BODIES.lock().get(&mail_id).cloned() {
        return Ok(b);
    }
    let auth = core.auth.auth(&core.esi.http, character_id, 60).await?;
    let raw: RawBody = core.esi.get(format!("characters/{character_id}/mail/{mail_id}")).auth(&auth).scope(SCOPE).json().await?;
    let body = comms::parse_mail_body(&raw.body);
    BODIES.lock().insert(mail_id, body.clone());
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_mail() {
        let raw: Vec<RawMail> = serde_json::from_str(
            r#"[
            {"from":90000001,"is_read":true,"labels":[1,16],"mail_id":7,
             "recipients":[{"recipient_id":90000002,"recipient_type":"character"},{"recipient_id":145000001,"recipient_type":"mailing_list"}],
             "subject":"Fish &amp; chips","timestamp":"2015-09-30T16:07:00Z"},
            {"from":145000001,"labels":[1],"mail_id":8,
             "recipients":[{"recipient_id":145000001,"recipient_type":"mailing_list"}],
             "subject":"Welcome","timestamp":"2015-10-01T16:07:00Z"},
            {"from":90000002,"is_read":true,"labels":[2],"mail_id":9,
             "recipients":[{"recipient_id":98000001,"recipient_type":"corporation"}],
             "subject":"Sent one","timestamp":"2015-09-29T16:07:00Z"}
        ]"#,
        )
        .unwrap();
        let labels: RawLabels = serde_json::from_str(
            r##"{"labels":[{"label_id":1,"name":"Inbox","unread_count":1},{"label_id":2,"name":"Sent"},
                          {"color":"#660066","label_id":16,"name":"PINK","unread_count":0}],"total_unread_count":1}"##,
        )
        .unwrap();
        let lists: Vec<MailingList> = serde_json::from_str(r#"[{"mailing_list_id":145000001,"name":"Fleet Pings"}]"#).unwrap();
        let ids = nameable_ids(&raw, &lists);
        assert!(ids.contains(&90000001) && ids.contains(&98000001));
        assert!(!ids.contains(&145000001), "mailing lists aren't looked up");

        let names: HashMap<i64, String> =
            [(90000001, "Alice"), (90000002, "Bob"), (98000001, "Corp")].into_iter().map(|(k, v)| (k, v.to_string())).collect();
        let data = build(raw, labels, lists, &names);
        assert_eq!(data.mails.iter().map(|m| m.mail_id).collect::<Vec<_>>(), vec![8, 7, 9], "newest first");
        let m7 = &data.mails[1];
        assert_eq!(m7.subject, "Fish & chips");
        assert_eq!(m7.from_name, "Alice");
        assert_eq!(m7.label_names, vec!["Inbox", "PINK"]);
        assert_eq!(m7.recipients[1].recipient_name, "Fleet Pings");
        let welcome = &data.mails[0];
        assert_eq!(welcome.from_name, "Fleet Pings");
        assert_eq!(welcome.from_type, "mailing_list");
        assert!(!welcome.is_read);
        assert!(data.mails[2].is_sent());
        assert_eq!(data.total_unread, Some(1));
        assert_eq!(data.label(16).and_then(|l| l.color.as_deref()), Some("#660066"));
    }
}
