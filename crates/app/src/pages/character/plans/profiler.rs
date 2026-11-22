//! The implant profiler: what's shown (the open plan or the training queue) under each implant set, with an
//! optional cerebral accelerator. The picked setup is compared skill by skill in the plan editor.

use super::{dur, P};
use crate::widgets::{self as w, Btn};
use gtk::prelude::*;
use podstack_core::features::plan_tools::{
    accelerator,
    profile::{self, Implants, Profile},
    sim::ATTRIBUTE_ORDER,
};
use podstack_core::{fmt, sde};
use std::cell::Cell;
use std::rc::Rc;

const CUSTOM_BONUSES: [i32; 9] = [1, 2, 3, 4, 5, 6, 8, 10, 12];
const DAY_MS: f64 = 86_400_000.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Choice {
    None,
    Item(i64),
    Custom,
}

/// The profiler's inputs.
#[derive(Clone, Debug)]
pub(super) struct ProfilerState {
    /// the implant row picked for comparison in the plan
    pub selected: Implants,
    pub choice: Choice,
    pub bonus: i32,
    pub days: f64,
}

impl ProfilerState {
    /// Starts with the character's active accelerator, if any, for the time plans count it.
    pub fn new(p: &P) -> ProfilerState {
        let mut s = ProfilerState { selected: Implants::Current, choice: Choice::None, bonus: 0, days: 7.0 };
        s.use_active(p);
        s
    }

    fn use_active(&mut self, p: &P) {
        if let Some((st, remaining)) = active(p) {
            self.bonus = st.bonus;
            self.days = (remaining / DAY_MS * 10.0).round() / 10.0;
            self.choice = match st.item {
                Some(a) => Choice::Item(a.type_id),
                None => Choice::Custom,
            };
        }
    }

    /// The setup to compare in the plan, or None when it's the current clone without an accelerator.
    pub fn profile(&self) -> Option<Profile> {
        let has_accel = self.bonus > 0 && self.days > 0.0;
        if self.selected == Implants::Current && !has_accel {
            return None;
        }
        Some(Profile { implants: self.selected, accelerator: has_accel.then_some((self.bonus as f64, self.days)) })
    }
}

/// The active accelerator and how long plans count it (ms), as EVE's skill queue does.
fn active(p: &P) -> Option<(accelerator::Status, f64)> {
    let c = p.character()?;
    let now = chrono::Utc::now();
    let st = accelerator::status(&c, now)?;
    let window = accelerator::plan_window(&c, now)?;
    Some((st, window.remaining))
}

fn options() -> Vec<(Choice, String)> {
    let mut v = vec![(Choice::None, "None".to_string())];
    for a in sde::ACCELERATORS {
        v.push((Choice::Item(a.type_id), format!("{} +{} ({}d)", a.name, a.bonus, a.days)));
    }
    v.push((Choice::Custom, "Custom…".into()));
    v
}

/// The panel and a function redrawing its results (after the plan or the inputs change).
pub(super) fn panel(p: &P) -> (gtk::Widget, Rc<dyn Fn()>) {
    let body = w::vbox(12);
    let attrs_box = w::hbox(8);
    body.append(&attrs_box);
    let active_box = w::vbox(0);
    body.append(&active_box);

    // accelerator inputs
    let controls = w::hbox(12);
    let opts = options();
    let labels: Vec<&str> = opts.iter().map(|o| o.1.as_str()).collect();
    let dd = gtk::DropDown::from_strings(&labels);
    dd.set_enable_search(true);
    dd.set_search_match_mode(gtk::StringFilterMatchMode::Substring);
    dd.set_expression(Some(gtk::PropertyExpression::new(gtk::StringObject::static_type(), None::<&gtk::Expression>, "string")));
    dd.set_tooltip_text(Some("Cerebral accelerator to compare (type to search)"));
    dd.set_hexpand(true);
    let accel_col = w::vbox(4);
    accel_col.append(&w::line("Cerebral accelerator", &["label-medium", "dim"]));
    accel_col.append(&dd);
    accel_col.set_hexpand(true);
    controls.append(&accel_col);
    let bonus_labels: Vec<String> = CUSTOM_BONUSES.iter().map(|b| format!("+{b}")).collect();
    let bonus_refs: Vec<&str> = bonus_labels.iter().map(|s| s.as_str()).collect();
    let bonus_dd = gtk::DropDown::from_strings(&bonus_refs);
    let bonus_col = w::vbox(4);
    bonus_col.append(&w::line("Bonus", &["label-medium", "dim"]));
    bonus_col.append(&bonus_dd);
    controls.append(&bonus_col);
    let days = gtk::SpinButton::with_range(0.0, 60.0, 0.5);
    days.set_digits(1);
    let days_col = w::vbox(4);
    days_col.append(&w::line("Lasts (days)", &["label-medium", "dim"]));
    days_col.append(&days);
    controls.append(&days_col);
    body.append(&controls);

    let results = w::vbox(2);
    body.append(&results);
    body.append(&w::text(
        "Pick a row and/or an accelerator to compare it skill by skill in the plan below. Base attributes are EVE's \
         values minus your current implants (shown as +N) and any active accelerator. Times assume training starts now. \
         An active accelerator is picked for you with the time it has left; check it on the Summary tab.",
        &["body-small", "dim"],
    ));

    let close = w::icon_button("close", "Close");
    {
        let pp = p.clone();
        close.connect_clicked(move |_| {
            pp.st.borrow_mut().profiler = None;
            pp.render_right();
        });
    }
    let subtitle = w::line("", &["body-medium", "dim"]);
    let card = w::card_with("Implants & Accelerators", None, &[close.upcast()], &body);
    if let Some(titles) = card.first_child().and_then(|h| h.first_child()).and_then(|t| t.downcast::<gtk::Box>().ok()) {
        titles.append(&subtitle);
    }

    // sets the inputs from the state without triggering their handlers
    let syncing = Rc::new(Cell::new(false));
    let sync = {
        let syncing = syncing.clone();
        let (pp, dd, bonus_dd, days, bonus_col, days_col) = (p.clone(), dd.clone(), bonus_dd.clone(), days.clone(), bonus_col.clone(), days_col.clone());
        let opts = opts.clone();
        Rc::new(move || {
            let Some(s) = pp.st.borrow().profiler.clone() else { return };
            syncing.set(true);
            let idx = opts.iter().position(|o| o.0 == s.choice).unwrap_or(0) as u32;
            if dd.selected() != idx {
                dd.set_selected(idx);
            }
            let bi = CUSTOM_BONUSES.iter().position(|b| *b == s.bonus).unwrap_or(3) as u32;
            if bonus_dd.selected() != bi {
                bonus_dd.set_selected(bi);
            }
            if (days.value() - s.days).abs() > 0.01 {
                days.set_value(s.days);
            }
            bonus_col.set_visible(s.choice == Choice::Custom);
            days_col.set_visible(s.bonus > 0);
            syncing.set(false);
        })
    };

    let refresh: Rc<dyn Fn()> = {
        let (pp, results, attrs_box, active_box, subtitle, sync) = (p.clone(), results.clone(), attrs_box.clone(), active_box.clone(), subtitle.clone(), sync.clone());
        Rc::new(move || {
            sync();
            fill(&pp, &results, &attrs_box, &active_box, &subtitle);
        })
    };

    let changed = {
        let (pp, refresh, syncing) = (p.clone(), refresh.clone(), syncing.clone());
        Rc::new(move |f: &dyn Fn(&mut ProfilerState)| {
            if syncing.get() {
                return;
            }
            if let Some(s) = pp.st.borrow_mut().profiler.as_mut() {
                f(s);
            }
            refresh();
            pp.render_editor();
        })
    };
    {
        let changed = changed.clone();
        let opts = opts.clone();
        dd.connect_selected_notify(move |dd| {
            let Some((choice, _)) = opts.get(dd.selected() as usize).cloned() else { return };
            changed(&|s: &mut ProfilerState| {
                if s.choice == choice {
                    return;
                }
                s.choice = choice;
                match choice {
                    Choice::None => s.bonus = 0,
                    Choice::Custom => {
                        if s.bonus == 0 {
                            s.bonus = 4;
                        }
                    }
                    Choice::Item(id) => {
                        if let Some(a) = sde::ACCELERATORS.iter().find(|a| a.type_id == id) {
                            s.bonus = a.bonus as i32;
                            s.days = a.days;
                        }
                    }
                }
            });
        });
    }
    {
        let changed = changed.clone();
        bonus_dd.connect_selected_notify(move |d| {
            let b = CUSTOM_BONUSES.get(d.selected() as usize).copied().unwrap_or(4);
            changed(&|s: &mut ProfilerState| {
                if s.choice == Choice::Custom {
                    s.bonus = b;
                }
            });
        });
    }
    {
        let changed = changed.clone();
        days.connect_value_changed(move |sp| {
            let v = sp.value();
            changed(&|s: &mut ProfilerState| s.days = v);
        });
    }
    refresh();
    (card.upcast(), refresh)
}

/// Redraws the attributes, the active accelerator line and the results table.
fn fill(p: &P, results: &gtk::Box, attrs_box: &gtk::Box, active_box: &gtk::Box, subtitle: &gtk::Label) {
    for b in [results, attrs_box, active_box] {
        while let Some(c) = b.first_child() {
            b.remove(&c);
        }
    }
    let Some(c) = p.character() else { return };
    let Some(state) = p.st.borrow().profiler.clone() else { return };
    subtitle.set_text(&{
        let st = p.st.borrow();
        if st.open.is_some() {
            st.name.clone()
        } else {
            "Training Queue".into()
        }
    });

    let bonuses = profile::implant_bonuses(&c);
    let base_attrs = profile::base_attributes(&c);
    for a in ATTRIBUTE_ORDER {
        let t = w::vbox(2);
        t.add_css_class("profiler-attr");
        t.append(&w::line(a.name(), &["label-medium", "dim"]));
        let b = bonuses.get(a);
        let m = if b > 0.0 { format!("{} <span size=\"small\">+{}</span>", base_attrs.get(a), b) } else { format!("{}", base_attrs.get(a)) };
        t.append(&w::markup(&m, &["title-medium", "numeric"]));
        attrs_box.append(&t);
    }

    if let Some((act, remaining)) = active(p) {
        let using = state.bonus == act.bonus && act.item.map(|i| state.choice == Choice::Item(i.type_id)).unwrap_or(true);
        let row = w::hbox(8);
        row.add_css_class("profiler-active");
        row.append(&w::icon_sized("bolt", 20));
        let name = act.item.map(|i| i.name.to_string()).unwrap_or_else(|| "Cerebral accelerator".into());
        let l = w::markup(
            &format!(
                "Active now: <b>{} +{}</b> · {} left{}",
                fmt::escape(&name),
                act.bonus,
                fmt::duration(chrono::Duration::milliseconds(remaining as i64), 2),
                if act.estimated { " (estimated)" } else { "" }
            ),
            &["body-medium"],
        );
        l.set_hexpand(true);
        row.append(&l);
        if !using {
            let b = w::button("Use it", Btn::Text);
            let pp = p.clone();
            b.connect_clicked(move |_| {
                let mut s = pp.st.borrow().profiler.clone();
                if let Some(s) = s.as_mut() {
                    s.use_active(&pp);
                }
                pp.st.borrow_mut().profiler = s;
                pp.render_right();
            });
            row.append(&b);
        }
        active_box.append(&row);
    }

    let st = p.st.borrow();
    let queue = st.planner.queue.clone();
    drop(st);
    if !queue.iter().any(|i| i.is_skill()) {
        results.append(&w::text("This plan has no skills to train.", &["body-medium", "dim"]));
        return;
    }
    let base = p.base();
    let accel = (state.bonus > 0 && state.days > 0.0).then_some((state.bonus as f64, state.days));
    let times: Vec<(Implants, f64)> =
        profile::scenarios().into_iter().map(|i| (i, profile::simulate(&c, &base, &queue, &Profile { implants: i, accelerator: accel }))).collect();
    let current = times.first().map(|t| t.1).unwrap_or(0.0);

    let head = w::hbox(12);
    head.set_margin_start(12);
    head.set_margin_end(12);
    let h = w::line("Clone", &["label-medium", "dim"]);
    h.set_hexpand(true);
    head.append(&h);
    for (t, wdt) in [("Training time", 120), ("Finishes", 110), ("vs current", 110)] {
        let l = w::line(t, &["label-medium", "dim"]);
        l.set_xalign(1.0);
        l.set_size_request(wdt, -1);
        head.append(&l);
    }
    results.append(&head);

    let names: Vec<String> = c.implants.as_deref().unwrap_or(&[]).iter().filter(|i| i.bonuses.iter().any(|b| b.1 > 0)).map(|i| i.name.clone()).collect();
    let now = chrono::Utc::now();
    for (imp, time) in times {
        let b = gtk::Button::new();
        b.add_css_class("flat");
        b.add_css_class("profiler-row");
        if imp == state.selected {
            b.add_css_class("selected");
        }
        b.set_tooltip_text(Some("Compare this setup in the plan"));
        let row = w::hbox(12);
        row.append(&w::icon_sized(if imp == state.selected { "radio_button_checked" } else { "radio_button_unchecked" }, 20));
        let texts = w::vbox(2);
        texts.set_hexpand(true);
        let label = match imp {
            Implants::Current => "Current clone".to_string(),
            Implants::Set(0) => "No implants".to_string(),
            Implants::Set(n) => sde::IMPLANT_GRADES.iter().find(|g| g.bonus as i32 == n).map(|g| format!("{} implants +{n}", g.grade)).unwrap_or_default(),
        };
        texts.append(&w::line(&label, &["body-medium"]));
        let detail = match imp {
            Implants::Current => names.join(" · "),
            Implants::Set(0) => String::new(),
            Implants::Set(n) => sde::IMPLANT_SLOTS.iter().map(|s| sde::implant_name(s.2, n as u8)).collect::<Vec<_>>().join(" · "),
        };
        if !detail.is_empty() {
            let d = w::line(&detail, &["body-small", "dim"]);
            d.set_tooltip_text(Some(&detail));
            texts.append(&d);
        }
        row.append(&texts);
        let t = w::line(&dur(time), &["numeric", "body-medium"]);
        t.set_xalign(1.0);
        t.set_size_request(120, -1);
        row.append(&t);
        let f = w::line(&fmt::date(now + chrono::Duration::milliseconds(time as i64)), &["numeric", "body-medium", "dim"]);
        f.set_xalign(1.0);
        f.set_size_request(110, -1);
        row.append(&f);
        let delta = time - current;
        let (dt, cls) = if delta.abs() < 60_000.0 {
            ("—".to_string(), "dim")
        } else {
            (format!("{}{}", if delta < 0.0 { "−" } else { "+" }, dur(delta.abs())), if delta < 0.0 { "success-text" } else { "warning-text" })
        };
        let d = w::line(&dt, &["numeric", "body-medium", cls]);
        d.set_xalign(1.0);
        d.set_size_request(110, -1);
        row.append(&d);
        b.set_child(Some(&row));
        let pp = p.clone();
        b.connect_clicked(move |_| {
            if let Some(s) = pp.st.borrow_mut().profiler.as_mut() {
                s.selected = imp;
            }
            pp.render_right();
        });
        results.append(&b);
    }
}
