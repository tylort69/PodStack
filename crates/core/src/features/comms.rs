//! Helpers behind the mail, wallet, market and contracts pages: mail bodies (EVE's HTML → Pango markup and plain
//! text), the mail unread state kept in PodStack, contract status and wording, order expiry, and the wallet
//! balance chart's numbers.

use crate::character::Character;
use crate::domains::contracts::Contract;
use crate::domains::mail::Mail;
use crate::domains::wallet::{JournalEntry, MarketOrder};
use crate::fmt;
use chrono::{DateTime, Duration, Utc};
use regex::Regex;
use std::collections::HashSet;
use std::sync::{Arc, LazyLock};

// ---------- mail ----------

/// `Character::local` key: ids of mails marked read in PodStack (ESI can't change EVE's read state without
/// another scope, so "Mark all read" remembers which mails were seen; a new one counts again).
pub const MAIL_SEEN_KEY: &str = "mail.seen";

/// Unread in EVE, not sent by the character, and not marked read in PodStack.
pub fn is_mail_unread(mail: &Mail, seen: &[i64]) -> bool {
    !mail.is_read && !mail.is_sent() && !seen.contains(&mail.mail_id)
}

pub fn seen_mails(c: &Character) -> Vec<i64> {
    c.local::<Vec<i64>>(MAIL_SEEN_KEY)
}

/// Unread mail received (for badges and the mail page).
pub fn unread_mail_count(c: &Character) -> usize {
    let seen = seen_mails(c);
    c.mail.as_ref().map(|m| m.mails.iter().filter(|m| is_mail_unread(m, &seen)).count()).unwrap_or(0)
}

/// "Mark all read": remembers every mail EVE still has as unread.
pub fn mark_all_mail_read(c: &mut Character) {
    let ids: Vec<i64> = c.mail.as_ref().map(|m| m.mails.iter().filter(|m| !m.is_read).map(|m| m.mail_id).collect()).unwrap_or_default();
    c.set_local(MAIL_SEEN_KEY, &ids);
}

/// Opening a mail in PodStack counts as reading it.
pub fn mark_mail_read(c: &mut Character, mail_id: i64) {
    let mut seen = seen_mails(c);
    if !seen.contains(&mail_id) {
        seen.push(mail_id);
        c.set_local(MAIL_SEEN_KEY, &seen);
    }
}

/// A mail body ready to show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MailBody {
    /// Pango markup: bold/italic/underline kept, web links as `<a href>`, EVE's font sizes and colors dropped.
    pub markup: String,
    pub text: String,
    /// Web links in the order they appear (no duplicates).
    pub links: Vec<String>,
}

/// Decodes the HTML entities EVE mail uses (`&amp;`, `&lt;`, `&#39;`, `&#x27;`, `&nbsp;`, …).
pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest[1..].find(|c: char| c == ';' || c == '&' || c.is_whitespace()).map(|j| j + 1);
        let decoded = end.filter(|&j| rest.as_bytes()[j] == b';').and_then(|j| {
            let name = &rest[1..j];
            let ch = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                _ if name.starts_with("#x") || name.starts_with("#X") => u32::from_str_radix(&name[2..], 16).ok().and_then(char::from_u32),
                _ if name.starts_with('#') => name[1..].parse::<u32>().ok().and_then(char::from_u32),
                _ => None,
            };
            ch.map(|c| (c, j + 1))
        });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[derive(Clone, Default)]
struct Run {
    text: String,
    bold: bool,
    italic: bool,
    underline: bool,
    link: Option<String>,
}

static HREF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)href\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#).unwrap());
static BARE_URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"https?://[^\s<>"']+"#).unwrap());

fn is_web(url: &str) -> bool {
    let l = url.to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://")
}

/// Converts an EVE mail body (HTML-ish: `<font>`, `<br>`, `<b>`, `<a href="showinfo:…">`) for display. Like the
/// old MailBodyHelper, only line breaks and web links really matter; in-game links (showinfo:, fitting:, …) keep
/// their text but can't be followed outside the client.
pub fn parse_mail_body(html: &str) -> MailBody {
    let mut runs: Vec<Run> = Vec::new();
    let mut links: Vec<String> = Vec::new();
    let (mut bold, mut italic, mut underline) = (0u32, 0u32, 0u32);
    let mut link_stack: Vec<Option<String>> = Vec::new();

    let push_text = |runs: &mut Vec<Run>, links: &mut Vec<String>, raw: &str, b: u32, i: u32, u: u32, link: Option<String>| {
        let text = decode_entities(raw);
        if text.is_empty() {
            return;
        }
        let base = Run { text: String::new(), bold: b > 0, italic: i > 0, underline: u > 0, link: link.clone() };
        if link.is_some() {
            runs.push(Run { text, ..base });
            return;
        }
        // bare web addresses become links too
        let mut last = 0;
        for m in BARE_URL.find_iter(&text) {
            let url = m.as_str().trim_end_matches(['.', ',', ')', ';', ':', '!', '?']);
            let end = m.start() + url.len();
            if m.start() > last {
                runs.push(Run { text: text[last..m.start()].to_string(), ..base.clone() });
            }
            if !links.iter().any(|l| l == url) {
                links.push(url.to_string());
            }
            runs.push(Run { text: url.to_string(), link: Some(url.to_string()), ..base.clone() });
            last = end;
        }
        if last < text.len() {
            runs.push(Run { text: text[last..].to_string(), ..base });
        }
    };

    let mut rest = html;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            push_text(&mut runs, &mut links, rest, bold, italic, underline, link_stack.last().cloned().flatten());
            break;
        };
        if lt > 0 {
            push_text(&mut runs, &mut links, &rest[..lt], bold, italic, underline, link_stack.last().cloned().flatten());
        }
        let after = &rest[lt + 1..];
        // "<" not starting a tag ("3 < 5") is text
        if !after.starts_with(|c: char| c.is_ascii_alphabetic() || c == '/' || c == '!') {
            push_text(&mut runs, &mut links, "<", bold, italic, underline, link_stack.last().cloned().flatten());
            rest = after;
            continue;
        }
        let Some(gt) = after.find('>') else {
            // a lone "<" is text
            push_text(&mut runs, &mut links, &rest[lt..], bold, italic, underline, link_stack.last().cloned().flatten());
            break;
        };
        let tag = after[..gt].trim();
        rest = &after[gt + 1..];

        let closing = tag.starts_with('/');
        let body = tag.trim_start_matches('/');
        let name: String = body.chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        let dec = |n: &mut u32| *n = n.saturating_sub(1);
        match (name.as_str(), closing) {
            ("br", _) => push_text(&mut runs, &mut links, "\n", 0, 0, 0, None),
            ("p" | "div", true) => push_text(&mut runs, &mut links, "\n", 0, 0, 0, None),
            ("li", false) => push_text(&mut runs, &mut links, "\n• ", 0, 0, 0, None),
            ("b" | "strong", false) => bold += 1,
            ("b" | "strong", true) => dec(&mut bold),
            ("i" | "em", false) => italic += 1,
            ("i" | "em", true) => dec(&mut italic),
            ("u", false) => underline += 1,
            ("u", true) => dec(&mut underline),
            ("a", false) => {
                let href = HREF.captures(body).and_then(|c| c.get(1).or(c.get(2)).or(c.get(3))).map(|m| decode_entities(m.as_str()));
                let web = href.filter(|h| is_web(h));
                if let Some(h) = &web {
                    if !links.iter().any(|l| l == h) {
                        links.push(h.clone());
                    }
                }
                link_stack.push(web);
            }
            ("a", true) => {
                link_stack.pop();
            }
            _ => {}
        }
    }

    // runs with the same formatting become one
    let mut merged: Vec<Run> = Vec::with_capacity(runs.len());
    for r in runs {
        match merged.last_mut() {
            Some(m) if (m.bold, m.italic, m.underline, &m.link) == (r.bold, r.italic, r.underline, &r.link) => m.text.push_str(&r.text),
            _ => merged.push(r),
        }
    }
    let mut runs = merged;

    // trim whitespace at both ends of the body
    while runs.first().map(|r| r.text.trim_start().is_empty()).unwrap_or(false) {
        runs.remove(0);
    }
    if let Some(r) = runs.first_mut() {
        r.text = r.text.trim_start().to_string();
    }
    while runs.last().map(|r| r.text.trim_end().is_empty()).unwrap_or(false) {
        runs.pop();
    }
    if let Some(r) = runs.last_mut() {
        r.text = r.text.trim_end().to_string();
    }

    let mut markup = String::new();
    let mut text = String::new();
    let mut open: Option<&str> = None;
    for r in &runs {
        if open != r.link.as_deref() {
            if open.is_some() {
                markup.push_str("</a>");
            }
            if let Some(l) = &r.link {
                markup.push_str(&format!("<a href=\"{}\">", fmt::escape(l)));
            }
            open = r.link.as_deref();
        }
        let mut t = fmt::escape(&r.text);
        if r.underline {
            t = format!("<u>{t}</u>");
        }
        if r.italic {
            t = format!("<i>{t}</i>");
        }
        if r.bold {
            t = format!("<b>{t}</b>");
        }
        markup.push_str(&t);
        text.push_str(&r.text);
    }
    if open.is_some() {
        markup.push_str("</a>");
    }
    MailBody { markup, text, links }
}

// ---------- contracts ----------

/// Statuses of contracts that are over.
pub const CONTRACT_COMPLETED_STATUSES: &[&str] =
    &["finished_issuer", "finished_contractor", "finished", "cancelled", "rejected", "failed", "deleted", "reversed"];

pub fn is_contract_completed(status: &str) -> bool {
    CONTRACT_COMPLETED_STATUSES.contains(&status)
}

/// How a status reads at a glance (the UI maps it to its status colors).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Good,
    Warn,
    Bad,
    Info,
    Neutral,
}

pub fn contract_status_tone(status: &str) -> Tone {
    match status {
        "outstanding" => Tone::Info,
        "in_progress" => Tone::Warn,
        "finished" | "finished_issuer" | "finished_contractor" => Tone::Good,
        "failed" | "rejected" | "deleted" | "reversed" => Tone::Bad,
        _ => Tone::Neutral,
    }
}

/// "item_exchange" → "Item Exchange", "player_donation" → "Player Donation".
pub fn title_case(key: &str) -> String {
    key.split('_')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The table's "Details": a courier's route and volume, else the contract's title.
pub fn contract_details(c: &Contract) -> String {
    if c.kind == "courier" {
        let sys = |p: &Option<crate::universe::Place>| {
            p.as_ref().map(|p| p.system_name.clone().unwrap_or_else(|| p.name.clone())).unwrap_or_else(|| "Unknown".into())
        };
        format!("{} » {} ({} m³)", sys(&c.start_location), sys(&c.end_location), fmt::int(c.volume.unwrap_or(0.0) as i64))
    } else if !c.title.trim().is_empty() {
        c.title.clone()
    } else if !c.items.is_empty() {
        let first = &c.items[0];
        if c.items.len() == 1 {
            format!("{} × {}", first.name, fmt::int(first.quantity))
        } else {
            format!("{} and {} more", first.name, c.items.len() - 1)
        }
    } else {
        String::new()
    }
}

/// Who issued it: the corporation when on its behalf.
pub fn contract_from(c: &Contract) -> &str {
    if c.for_corporation {
        &c.issuer_corporation_name
    } else {
        &c.issuer_name
    }
}

/// Who took it, else who it's for, else "Public".
pub fn contract_to(c: &Contract) -> &str {
    c.acceptor_name.as_deref().or(c.assignee_name.as_deref()).unwrap_or("Public")
}

/// The headline ISK figure: a courier's reward, an auction's opening bid, an exchange's price (negative when the
/// issuer pays).
pub fn contract_value(c: &Contract) -> Option<f64> {
    match c.kind.as_str() {
        "courier" => c.reward,
        _ => c.price.filter(|p| *p != 0.0).or(c.reward.filter(|r| *r != 0.0).map(|r| -r)),
    }
}

/// Every character's contracts (each once: a contract between two of the user's characters shows up for both),
/// optionally only open (`Some(false)`) or completed (`Some(true)`) ones. Paired with the character that has it.
pub fn all_contracts(chars: &[Arc<Character>], complete: Option<bool>) -> Vec<(i64, Contract)> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for ch in chars {
        let Some(d) = &ch.contracts else { continue };
        for c in &d.contracts {
            if complete.map(|want| c.is_completed() != want).unwrap_or(false) {
                continue;
            }
            if seen.insert(c.contract_id) {
                out.push((ch.id, c.clone()));
            }
        }
    }
    out.sort_by_key(|(_, c)| std::cmp::Reverse(c.date_issued));
    out
}

// ---------- market orders ----------

pub fn order_expires_at(o: &MarketOrder) -> DateTime<Utc> {
    o.issued + Duration::days(o.duration)
}

/// Still open but running out within a day, so worth relisting.
pub fn is_order_expiring_soon(o: &MarketOrder, now: DateTime<Utc>) -> bool {
    if !o.is_active() {
        return false;
    }
    let left = order_expires_at(o) - now;
    left > Duration::zero() && left < Duration::days(1)
}

/// Every character's orders with their character id, newest first.
pub fn all_orders(chars: &[Arc<Character>]) -> Vec<(i64, MarketOrder)> {
    let mut out: Vec<(i64, MarketOrder)> =
        chars.iter().flat_map(|c| c.market_orders.iter().flat_map(|d| d.orders.iter()).map(|o| (c.id, o.clone()))).collect();
    out.sort_by_key(|(_, o)| std::cmp::Reverse(o.issued));
    out
}

/// The market page's nav badge: open orders with under a day left.
pub fn count_expiring_soon(chars: &[Arc<Character>]) -> usize {
    let now = Utc::now();
    chars.iter().flat_map(|c| c.market_orders.iter().flat_map(|d| d.orders.iter())).filter(|o| is_order_expiring_soon(o, now)).count()
}

/// "station" → "Station", "solarsystem" → "Solar system", "5" → "5 jumps".
pub fn order_range_label(range: &str) -> String {
    match range {
        "station" => "Station".into(),
        "solarsystem" => "Solar system".into(),
        "region" => "Region".into(),
        "1" => "1 jump".into(),
        n if n.parse::<u32>().is_ok() => format!("{n} jumps"),
        other => title_case(other),
    }
}

// ---------- wallet ----------

/// (income, expenses) of a journal; expenses are negative.
pub fn journal_totals(entries: &[JournalEntry]) -> (f64, f64) {
    let income = entries.iter().filter(|e| e.amount > 0.0).map(|e| e.amount).sum();
    let expenses = entries.iter().filter(|e| e.amount < 0.0).map(|e| e.amount).sum();
    (income, expenses)
}

/// One step of the balance chart.
#[derive(Clone, Debug, PartialEq)]
pub struct BalancePoint {
    pub time: DateTime<Utc>,
    pub balance: f64,
    /// Index into the journal entries; None for the closing "now" point.
    pub entry: Option<usize>,
}

/// The balance after each journal entry, oldest first, ending with the current balance at `now` (the balance only
/// changes with an entry, so the chart draws steps).
pub fn balance_points(entries: &[JournalEntry], balance: Option<f64>, now: DateTime<Utc>) -> Vec<BalancePoint> {
    let mut points: Vec<BalancePoint> =
        entries.iter().enumerate().filter_map(|(i, e)| e.balance.map(|b| BalancePoint { time: e.date, balance: b, entry: Some(i) })).collect();
    let id = |p: &BalancePoint| p.entry.map(|i| entries[i].id).unwrap_or(0);
    points.sort_by(|a, b| a.time.cmp(&b.time).then(id(a).cmp(&id(b))));
    if let (Some(last), Some(b)) = (points.last(), balance) {
        if now >= last.time {
            points.push(BalancePoint { time: now, balance: b, entry: None });
        }
    }
    points
}

/// About `count` round values covering min..max, for chart axes.
pub fn nice_ticks(min: f64, max: f64, count: usize) -> Vec<f64> {
    if max <= min || !min.is_finite() || !max.is_finite() {
        return vec![min];
    }
    let raw = (max - min) / count.max(1) as f64;
    let magnitude = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0].iter().map(|m| m * magnitude).find(|s| *s >= raw).unwrap_or(10.0 * magnitude);
    let mut ticks = Vec::new();
    let mut v = (min / step).ceil() * step;
    while v <= max + step * 1e-9 {
        ticks.push(v);
        v += step;
    }
    ticks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::mail::MailData;

    #[test]
    fn entities() {
        assert_eq!(decode_entities("a &amp; b &lt;c&gt; &quot;d&quot; &#39;e&#x27; f&nbsp;g"), "a & b <c> \"d\" 'e' f g");
        assert_eq!(decode_entities("AT&T & co &bogus;"), "AT&T & co &bogus;");
        assert_eq!(decode_entities("plain"), "plain");
    }

    #[test]
    fn mail_body() {
        let html = r##"<font size="12" color="#bfffffff">Hi <b>there</b> &amp; welcome!<br>Fleet at <a href="showinfo:5//30000142">Jita</a>.<br/>
Read <a href="https://example.com/a?x=1&amp;y=2">the guide</a> or see https://wiki.example.org/page.</font><br /><br>"##;
        let b = parse_mail_body(html);
        assert_eq!(b.links, vec!["https://example.com/a?x=1&y=2", "https://wiki.example.org/page"]);
        assert_eq!(b.text, "Hi there & welcome!\nFleet at Jita.\n\nRead the guide or see https://wiki.example.org/page.");
        assert!(b.markup.starts_with("Hi <b>there</b> &amp; welcome!\nFleet at Jita."));
        assert!(b.markup.contains("<a href=\"https://example.com/a?x=1&amp;y=2\">the guide</a>"));
        assert!(b.markup.contains("<a href=\"https://wiki.example.org/page\">https://wiki.example.org/page</a>."));
        assert!(!b.markup.ends_with('\n'), "trailing breaks trimmed");
    }

    #[test]
    fn mail_body_is_always_balanced() {
        // unbalanced and stray tags must still give valid markup
        let b = parse_mail_body("</b><i>open <u>never closed < 5 and 3 > 2");
        assert_eq!(b.text, "open never closed < 5 and 3 > 2");
        assert_eq!(b.markup, "<i>open </i><i><u>never closed &lt; 5 and 3 &gt; 2</u></i>");
        assert_eq!(parse_mail_body("").markup, "");
    }

    fn mail(id: i64, read: bool, labels: Vec<i64>) -> Mail {
        Mail { mail_id: id, is_read: read, labels, ..Default::default() }
    }

    #[test]
    fn unread_state() {
        let mut c = Character::new(1);
        c.mail = Some(MailData { mails: vec![mail(1, false, vec![1]), mail(2, true, vec![1]), mail(3, false, vec![2])], ..Default::default() });
        assert_eq!(unread_mail_count(&c), 1, "sent mail doesn't count");
        mark_mail_read(&mut c, 1);
        assert_eq!(unread_mail_count(&c), 0);
        c.mail.as_mut().unwrap().mails.push(mail(4, false, vec![1]));
        assert_eq!(unread_mail_count(&c), 1, "new mail counts again");
        mark_all_mail_read(&mut c);
        assert_eq!(unread_mail_count(&c), 0);
        assert_eq!(seen_mails(&c), vec![1, 3, 4]);
    }

    #[test]
    fn contract_helpers() {
        assert!(is_contract_completed("finished"));
        assert!(!is_contract_completed("outstanding"));
        assert_eq!(contract_status_tone("in_progress"), Tone::Warn);
        assert_eq!(title_case("item_exchange"), "Item Exchange");
        let mut c = Contract { kind: "courier".into(), volume: Some(12000.5), reward: Some(5.0), ..Default::default() };
        c.start_location = Some(crate::universe::Place { name: "Jita IV".into(), system_name: Some("Jita".into()), ..Default::default() });
        assert_eq!(contract_details(&c), "Jita » Unknown (12,000 m³)");
        assert_eq!(contract_to(&c), "Public");
        assert_eq!(contract_value(&c), Some(5.0));
        c.assignee_name = Some("Bob".into());
        assert_eq!(contract_to(&c), "Bob");
        c.acceptor_name = Some("Carol".into());
        assert_eq!(contract_to(&c), "Carol");
    }

    #[test]
    fn contracts_once_each() {
        let contract = |id: i64, status: &str| Contract { contract_id: id, status: status.into(), ..Default::default() };
        let mut a = Character::new(1);
        a.contracts = Some(crate::domains::contracts::ContractsData { contracts: vec![contract(10, "outstanding"), contract(11, "finished")], slots_used: 1 });
        let mut b = Character::new(2);
        b.contracts = Some(crate::domains::contracts::ContractsData { contracts: vec![contract(10, "outstanding")], slots_used: 0 });
        let chars = vec![Arc::new(a), Arc::new(b)];
        assert_eq!(all_contracts(&chars, None).len(), 2);
        assert_eq!(all_contracts(&chars, Some(false)).len(), 1);
        assert_eq!(all_contracts(&chars, Some(true))[0].1.contract_id, 11);
    }

    #[test]
    fn order_expiry() {
        let now = Utc::now();
        let order = |issued: DateTime<Utc>, state: &str| MarketOrder { issued, duration: 30, state: state.into(), ..Default::default() };
        let soon = order(now - Duration::days(30) + Duration::hours(5), "active");
        assert_eq!(order_expires_at(&soon), soon.issued + Duration::days(30));
        assert!(is_order_expiring_soon(&soon, now));
        assert!(!is_order_expiring_soon(&order(now - Duration::days(2), "active"), now));
        assert!(!is_order_expiring_soon(&order(now - Duration::days(31), "active"), now), "already expired");
        assert!(!is_order_expiring_soon(&MarketOrder { state: "cancelled".into(), ..soon.clone() }, now));
        let mut c = Character::new(1);
        c.market_orders = Some(crate::domains::wallet::MarketOrdersData { orders: vec![soon] });
        assert_eq!(count_expiring_soon(&[Arc::new(c)]), 1);
        assert_eq!(order_range_label("solarsystem"), "Solar system");
        assert_eq!(order_range_label("5"), "5 jumps");
    }

    #[test]
    fn wallet_numbers() {
        let t0 = Utc::now() - Duration::days(2);
        let e = |id: i64, h: i64, amount: f64, balance: Option<f64>| JournalEntry {
            id,
            date: t0 + Duration::hours(h),
            amount,
            balance,
            ..Default::default()
        };
        // newest first, as stored
        let entries = vec![e(3, 10, -50.0, Some(150.0)), e(2, 5, 100.0, Some(200.0)), e(1, 1, 7.0, None)];
        assert_eq!(journal_totals(&entries), (107.0, -50.0));
        let now = Utc::now();
        let p = balance_points(&entries, Some(175.0), now);
        assert_eq!(p.iter().map(|p| p.balance).collect::<Vec<_>>(), vec![200.0, 150.0, 175.0]);
        assert_eq!(p[0].entry, Some(1));
        assert_eq!(p[2].entry, None);
        assert_eq!(nice_ticks(0.0, 100.0, 4), vec![0.0, 25.0, 50.0, 75.0, 100.0]);
        assert_eq!(nice_ticks(5.0, 5.0, 4), vec![5.0]);
        assert_eq!(nice_ticks(1.2e9, 1.9e9, 4), vec![1.2e9, 1.4e9, 1.6e9, 1.8e9]);
    }
}
