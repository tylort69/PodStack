//! Character tab: EVE mail. Headers come with the character's refresh; a mail's body is fetched when it's opened.
//! EVE's read state can't be changed through ESI with PodStack's scopes, so "Mark all read" (and opening a mail)
//! is remembered in PodStack.

use crate::ctx::{Ctx, Page};
use crate::pages::market_orders::{scope_failed, set_child};
use crate::rt;
use crate::widgets::{self as w, Btn, Status, Table};
use gtk::{glib, prelude::*};
use podstack_core::domains::mail::{self, Mail, MailData, Recipient};
use podstack_core::features::comms;
use podstack_core::{config, fmt};
use std::cell::RefCell;
use std::rc::Rc;

const CSS: &str = "
.comms-filter-bar { padding: 4px 0; }
.comms-unread { font-weight: 600; }
.comms-person { border: 1px solid @outline_variant; border-radius: 999px; padding: 2px 12px 2px 2px; }
.comms-person.no-avatar { padding-left: 8px; }
.comms-mail-body { background-color: @surface_container_low; border-radius: 16px; padding: 20px; }
.comms-mail-body label { line-height: 1.4; }
.comms-link-row { padding: 2px 0 2px 4px; }
";

/// Label filter: everything, unread only, or one label.
#[derive(Clone, PartialEq)]
enum Filter {
    All,
    Unread,
    Label(i64),
}

impl Filter {
    fn key(&self) -> String {
        match self {
            Filter::All => "all".into(),
            Filter::Unread => "unread".into(),
            Filter::Label(id) => id.to_string(),
        }
    }
    fn parse(s: &str) -> Filter {
        match s {
            "unread" => Filter::Unread,
            other => other.parse().map(Filter::Label).unwrap_or(Filter::All),
        }
    }
}

#[derive(Clone)]
struct Row {
    mail: Mail,
    unread: bool,
}

pub fn build(ctx: &Ctx, id: i64) -> Page {
    crate::theme::add_css(CSS);
    let body = w::vbox(16);
    let Some(c) = ctx.core().character(id) else { return Page::new("Mail", body) };
    let Some(data) = &c.mail else {
        if scope_failed(&c, "mails") {
            body.append(&w::scope_notice(mail::SCOPE));
        } else {
            body.append(&w::empty_state("mail", "Mail not loaded yet", "The newest mail appears after the next refresh."));
        }
        return Page::new("Mail", body);
    };

    let seen = comms::seen_mails(&c);
    let rows: Rc<Vec<Row>> = Rc::new(data.mails.iter().map(|m| Row { mail: m.clone(), unread: comms::is_mail_unread(m, &seen) }).collect());
    let unread = rows.iter().filter(|r| r.unread).count();

    // heading: counts and "Mark all read"
    let head = w::hbox(16);
    let titles = w::vbox(2);
    titles.set_hexpand(true);
    titles.append(&w::text("EVE Mail", &["title-large"]));
    let mut sub = vec![format!("{} mails", rows.len())];
    if let Some(t) = c.refreshes.get("mails").and_then(|r| r.last) {
        sub.push(format!("updated {}", fmt::relative(t)));
    }
    titles.append(&w::text(&sub.join(" · "), &["body-medium", "dim"]));
    head.append(&titles);
    if unread > 0 {
        head.append(&w::status(&format!("{unread} unread"), Status::Info));
    }
    let mark = w::button_icon("done_all", "Mark all read", Btn::Tonal);
    mark.set_valign(gtk::Align::Center);
    mark.set_sensitive(unread > 0);
    {
        let ctx = ctx.clone();
        mark.connect_clicked(move |_| {
            ctx.core().update_character(id, comms::mark_all_mail_read);
            ctx.toast("All mail marked read");
        });
    }
    head.append(&mark);
    body.append(&head);

    let filter = Rc::new(RefCell::new((
        Filter::parse(&ctx.recall("mail.filter").unwrap_or_default()),
        ctx.recall("mail.search").unwrap_or_default(),
    )));
    let holder = w::vbox(0);
    holder.set_vexpand(true);
    let refill = {
        let (ctx, holder, rows, filter) = (ctx.clone(), holder.clone(), rows.clone(), filter.clone());
        Rc::new(move || {
            let (f, query) = filter.borrow().clone();
            let q = query.trim().to_lowercase();
            let shown: Vec<Row> = rows
                .iter()
                .filter(|r| match &f {
                    Filter::All => true,
                    Filter::Unread => r.unread,
                    Filter::Label(l) => r.mail.labels.contains(l),
                })
                .filter(|r| {
                    q.is_empty()
                        || r.mail.subject.to_lowercase().contains(&q)
                        || r.mail.from_name.to_lowercase().contains(&q)
                        || r.mail.recipients.iter().any(|x| x.recipient_name.to_lowercase().contains(&q))
                })
                .cloned()
                .collect();
            let empty = if q.is_empty() && f == Filter::All { "No mail found." } else { "Nothing matches." };
            set_child(&holder, &mail_table(&ctx, id, shown, empty));
        })
    };

    body.append(&filter_bar(ctx, data, &rows, filter.clone(), refill.clone()));
    body.append(&holder);
    refill();
    Page::new("Mail", body).no_scroll()
}

fn filter_bar(ctx: &Ctx, data: &MailData, rows: &[Row], filter: Rc<RefCell<(Filter, String)>>, refill: Rc<dyn Fn()>) -> gtk::Box {
    let bar = w::hbox(12);
    bar.add_css_class("comms-filter-bar");
    let chips_box = w::hbox(8);
    let mut options: Vec<(Filter, String)> = vec![
        (Filter::All, "All".into()),
        (Filter::Unread, format!("Unread ({})", rows.iter().filter(|r| r.unread).count())),
    ];
    let mut labels = data.labels.clone();
    labels.sort_by_key(|l| l.label_id);
    for l in labels {
        let n = rows.iter().filter(|r| r.mail.labels.contains(&l.label_id)).count();
        if n > 0 {
            options.push((Filter::Label(l.label_id), format!("{} ({n})", l.name)));
        }
    }
    let current = filter.borrow().0.clone();
    let chips: Rc<RefCell<Vec<(Filter, gtk::ToggleButton)>>> = Rc::new(RefCell::new(vec![]));
    for (f, label) in options {
        let chip = w::chip(&label, f == current);
        let (ctx, filter, refill, chips2, f2) = (ctx.clone(), filter.clone(), refill.clone(), chips.clone(), f.clone());
        chip.connect_clicked(move |b| {
            // chips act as a single choice: the clicked one stays on, the rest go off
            if filter.borrow().0 == f2 {
                b.set_active(true);
                return;
            }
            filter.borrow_mut().0 = f2.clone();
            ctx.remember("mail.filter", f2.key());
            for (other, c) in chips2.borrow().iter() {
                if *other != f2 {
                    c.set_active(false);
                }
            }
            refill();
        });
        chips_box.append(&chip);
        chips.borrow_mut().push((f, chip));
    }
    let sw = gtk::ScrolledWindow::new();
    sw.set_vscrollbar_policy(gtk::PolicyType::Never);
    sw.set_propagate_natural_height(true);
    sw.set_child(Some(&chips_box));
    sw.set_hexpand(true);
    bar.append(&sw);

    let search = w::search_entry("Search subject, sender or recipients");
    search.set_text(&filter.borrow().1);
    search.set_hexpand(false);
    search.set_max_width_chars(36);
    {
        let ctx = ctx.clone();
        search.connect_search_changed(move |s| {
            ctx.remember("mail.search", s.text().as_str());
            filter.borrow_mut().1 = s.text().to_string();
            refill();
        });
    }
    bar.append(&search);
    bar
}

fn mail_table(ctx: &Ctx, char_id: i64, rows: Vec<Row>, empty: &str) -> gtk::Widget {
    let open = {
        let ctx = ctx.clone();
        Rc::new(move |m: &Mail| open_mail(&ctx, char_id, m))
    };
    let open2 = open.clone();
    Table::new(rows)
        .widget_sorted(
            "",
            |r| {
                let i = w::icon_sized(if r.unread { "mail" } else { "drafts" }, 18);
                i.add_css_class(if r.unread { "primary-text" } else { "dim" });
                i.set_tooltip_text(Some(if r.mail.is_sent() {
                    "Sent"
                } else if r.unread {
                    "Unread"
                } else {
                    "Read"
                }));
                i.upcast()
            },
            |a, b| a.unread.cmp(&b.unread),
        )
        .width(56)
        .widget_sorted(
            "Date",
            |r| {
                let l = w::line(&fmt::datetime(r.mail.timestamp), &["dim", "numeric"]);
                l.set_tooltip_text(Some(&fmt::relative(r.mail.timestamp)));
                l.upcast()
            },
            |a, b| a.mail.timestamp.cmp(&b.mail.timestamp),
        )
        .widget_sorted("From", |r| person(r.mail.from, &r.mail.from_type, &r.mail.from_name, false), |a, b| {
            a.mail.from_name.to_lowercase().cmp(&b.mail.from_name.to_lowercase())
        })
        .width(220)
        .widget_sorted(
            "Subject",
            |r| {
                let l = w::line(if r.mail.subject.is_empty() { "(no subject)" } else { &r.mail.subject }, if r.unread { &["comms-unread"] } else { &[] });
                l.upcast()
            },
            |a, b| a.mail.subject.to_lowercase().cmp(&b.mail.subject.to_lowercase()),
        )
        .expand()
        .text("Labels", |r| r.mail.label_names.join(", "))
        .width(180)
        .widget("", move |r| {
            let b = w::icon_button("chevron_right", "Open mail");
            let (open, m) = (open.clone(), r.mail.clone());
            b.connect_clicked(move |_| open(&m));
            b.upcast()
        })
        .sort_by(1, true)
        .on_activate(move |r| open2(&r.mail))
        .empty_text(empty)
        .build()
}

/// A sender or recipient: portrait/logo (or a list icon) and name. `chip` draws it as an input chip.
fn person(id: i64, kind: &str, name: &str, chip: bool) -> gtk::Widget {
    let row = w::hbox(8);
    let url = match kind {
        "character" => Some(config::character_portrait(id, 64)),
        "corporation" => Some(config::corporation_logo(id, 64)),
        "alliance" => Some(config::alliance_logo(id, 64)),
        _ => None,
    };
    match url {
        Some(u) => row.append(&w::avatar(&u, 24)),
        None => {
            let i = w::icon_sized(if kind == "mailing_list" { "list" } else { "help_outline" }, 20);
            i.add_css_class("dim");
            row.append(&i);
            if chip {
                row.add_css_class("no-avatar");
            }
        }
    }
    let l = w::line(name, &[]);
    l.set_valign(gtk::Align::Center);
    row.append(&l);
    if chip {
        row.add_css_class("comms-person");
        row.set_halign(gtk::Align::Start);
    }
    row.upcast()
}

fn meta_row(grid: &gtk::Grid, row: i32, key: &str, value: &impl IsA<gtk::Widget>) {
    let k = w::line(key, &["body-medium", "dim"]);
    k.set_valign(gtk::Align::Center);
    grid.attach(&k, 0, row, 1, 1);
    value.set_hexpand(true);
    grid.attach(value, 1, row, 1, 1);
}

/// Recipients as chips: in a row, or wrapping in a grid when there are many.
fn recipients_box(list: &[Recipient]) -> gtk::Widget {
    if list.len() <= 4 {
        let b = w::hbox(6);
        for r in list {
            b.append(&person(r.recipient_id, &r.recipient_type, &r.recipient_name, true));
        }
        return b.upcast();
    }
    let f = gtk::FlowBox::new();
    f.set_selection_mode(gtk::SelectionMode::None);
    f.set_column_spacing(6);
    f.set_row_spacing(6);
    f.set_max_children_per_line(4);
    for r in list {
        f.append(&person(r.recipient_id, &r.recipient_type, &r.recipient_name, true));
    }
    f.upcast()
}

/// Whether GTK will take the body's markup: Pango's parser checks everything but the `<a href>` links GTK adds.
fn markup_ok(m: &str) -> bool {
    static LINK: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r#"<a href="[^"]*">|</a>"#).unwrap());
    pango::parse_markup(&LINK.replace_all(m, ""), '\0').is_ok()
}

/// The mail dialog: sender, recipients, labels, then the body once it's fetched, and its web links.
fn open_mail(ctx: &Ctx, char_id: i64, m: &Mail) {
    let core = ctx.core().clone();
    let subject = if m.subject.is_empty() { "(no subject)".to_string() } else { m.subject.clone() };
    let (win, content, actions) = w::dialog(ctx, &subject, 760);
    win.set_default_height(680);

    let grid = gtk::Grid::new();
    grid.set_column_spacing(24);
    grid.set_row_spacing(10);
    meta_row(&grid, 0, "From", &person(m.from, &m.from_type, &m.from_name, true));
    meta_row(&grid, 1, "To", &recipients_box(&m.recipients));
    meta_row(&grid, 2, "Sent", &w::line(&format!("{} ({})", fmt::datetime(m.timestamp), fmt::relative(m.timestamp)), &["body-medium"]));
    if !m.label_names.is_empty() {
        let labels = w::hbox(6);
        for l in &m.label_names {
            labels.append(&w::status(l, Status::Neutral));
        }
        meta_row(&grid, 3, "Labels", &labels);
    }
    content.append(&grid);

    let body_box = w::vbox(12);
    body_box.add_css_class("comms-mail-body");
    let spinner = gtk::Spinner::new();
    spinner.start();
    spinner.set_halign(gtk::Align::Center);
    spinner.set_size_request(32, 32);
    body_box.append(&spinner);
    let links_box = w::vbox(4);
    let page = w::vbox(16);
    page.append(&body_box);
    page.append(&links_box);
    let sw = w::scrolled(&page);
    sw.set_min_content_height(280);
    content.append(&sw);

    let copy = w::button_icon("content_copy", "Copy text", Btn::Text);
    copy.set_sensitive(false);
    let close = w::button("Close", Btn::Text);
    let wc = win.clone();
    close.connect_clicked(move |_| wc.close());
    actions.append(&copy);
    actions.append(&close);
    win.present();

    if !m.is_read {
        let mail_id = m.mail_id;
        core.update_character(char_id, move |c| comms::mark_mail_read(c, mail_id));
    }

    let mail_id = m.mail_id;
    let ctx = ctx.clone();
    let core2 = core.clone();
    rt::run(async move { mail::fetch_body(&core2, char_id, mail_id).await.map_err(|e| e.to_string()) }, move |result| {
        while let Some(child) = body_box.first_child() {
            body_box.remove(&child);
        }
        match result {
            Ok(b) => {
                let label = w::text("", &["body-large"]);
                label.set_selectable(true);
                label.set_xalign(0.0);
                label.set_valign(gtk::Align::Start);
                if b.text.trim().is_empty() {
                    label.set_text("(This mail has no text.)");
                    label.add_css_class("dim");
                } else if markup_ok(&b.markup) {
                    label.set_markup(&b.markup);
                } else {
                    label.set_text(&b.text);
                }
                {
                    let ctx = ctx.clone();
                    label.connect_activate_link(move |_, uri| {
                        w::open_url(&ctx, uri);
                        glib::Propagation::Stop
                    });
                }
                body_box.append(&label);
                if !b.links.is_empty() {
                    links_box.append(&w::text(&format!("Links ({})", b.links.len()), &["title-small"]));
                    for (i, link) in b.links.iter().enumerate() {
                        let open = w::icon_button("open_in_new", "Open in browser");
                        let (ctx, l) = (ctx.clone(), link.clone());
                        open.connect_clicked(move |_| w::open_url(&ctx, &l));
                        let row = w::hbox(12);
                        row.add_css_class("comms-link-row");
                        row.append(&w::line(&(i + 1).to_string(), &["label-large", "dim"]));
                        let l = w::line(link, &["body-medium"]);
                        l.set_hexpand(true);
                        l.set_tooltip_text(Some(link));
                        row.append(&l);
                        row.append(&open);
                        links_box.append(&row);
                    }
                }
                let text = b.text.clone();
                let ctx = ctx.clone();
                copy.connect_clicked(move |_| w::copy_to_clipboard(&ctx, &text));
                copy.set_sensitive(true);
            }
            Err(e) => {
                body_box.append(&w::banner("error_outline", &format!("Couldn't load this mail: {e}"), w::BannerKind::Error));
            }
        }
    });
}
