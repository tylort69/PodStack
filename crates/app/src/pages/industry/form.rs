//! Small form pieces for the Industry tabs: labelled fields that wrap, number entries, an entry with suggestions,
//! and security status labels.

use crate::widgets as w;
use gtk::prelude::*;
use podstack_core::sde::{SecurityClass, MAP};
use std::cell::RefCell;
use std::rc::Rc;

/// "1,234.5", "12k", "1.5m", "2b".
pub fn parse_amount(s: &str) -> Option<f64> {
    let s = s.trim().to_lowercase().replace([',', ' ', '\u{a0}'], "");
    let (num, mult) = match s.chars().last()? {
        'k' => (&s[..s.len() - 1], 1e3),
        'm' => (&s[..s.len() - 1], 1e6),
        'b' => (&s[..s.len() - 1], 1e9),
        _ => (s.as_str(), 1.0),
    };
    num.parse::<f64>().ok().filter(|n| n.is_finite()).map(|n| n * mult)
}

/// A fraction as a percent without trailing zeros: 0.0425 → "4.25".
pub fn percent_text(fraction: f64) -> String {
    let s = format!("{:.4}", fraction * 100.0);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

pub fn labelled(label: &str, child: &impl IsA<gtk::Widget>) -> gtk::Box {
    let b = w::vbox(4);
    b.append(&w::line(label, &["label-medium", "dim"]));
    b.append(child);
    b
}

/// Form fields that wrap onto more lines in a narrow window.
pub fn fields(children: Vec<gtk::Widget>) -> gtk::FlowBox {
    let f = gtk::FlowBox::new();
    f.set_selection_mode(gtk::SelectionMode::None);
    f.set_column_spacing(12);
    f.set_row_spacing(12);
    f.set_max_children_per_line(8);
    f.set_homogeneous(false);
    for c in children {
        c.set_valign(gtk::Align::End);
        f.insert(&c, -1);
    }
    f
}

pub fn number_entry(text: &str, width: i32) -> gtk::Entry {
    let e = gtk::Entry::new();
    e.set_text(text);
    e.set_input_purpose(gtk::InputPurpose::Number);
    gtk::prelude::EditableExt::set_alignment(&e, 1.0);
    e.set_width_chars(width);
    e
}

/// An entry that suggests matches as you type. `search` returns (id, title, detail) for the text; `on_pick` gets
/// the chosen id (Enter picks the first).
pub fn suggest_entry(placeholder: &str, search: impl Fn(&str) -> Vec<(i64, String, Option<String>)> + 'static, on_pick: impl Fn(i64, &str) + 'static) -> gtk::SearchEntry {
    let entry = w::search_entry(placeholder);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("ind-suggestions");
    let pop = gtk::Popover::new();
    pop.set_autohide(true);
    pop.set_has_arrow(false);
    pop.set_can_focus(false);
    pop.set_position(gtk::PositionType::Bottom);
    pop.set_child(Some(&list));
    pop.set_parent(&entry);
    let hits: Rc<RefCell<Vec<(i64, String)>>> = Rc::default();
    let on_pick = Rc::new(on_pick);
    {
        let (list, pop, hits) = (list.clone(), pop.clone(), hits.clone());
        entry.connect_search_changed(move |e| {
            while let Some(c) = list.first_child() {
                list.remove(&c);
            }
            let found = search(&e.text());
            *hits.borrow_mut() = found.iter().map(|h| (h.0, h.1.clone())).collect();
            for (_, title, detail) in &found {
                let row = w::hbox(8);
                row.set_margin_start(8);
                row.set_margin_end(8);
                row.set_margin_top(4);
                row.set_margin_bottom(4);
                let name = gtk::Label::new(Some(title));
                name.set_xalign(0.0);
                name.set_hexpand(true);
                row.append(&name);
                if let Some(d) = detail {
                    row.append(&w::line(d, &["dim"]));
                }
                list.append(&row);
            }
            if found.is_empty() {
                pop.popdown();
            } else {
                pop.popup();
            }
        });
    }
    {
        let (hits, pop, entry2, on_pick) = (hits.clone(), pop.clone(), entry.clone(), on_pick.clone());
        list.connect_row_activated(move |_, row| {
            let picked = hits.borrow().get(row.index() as usize).cloned();
            if let Some((id, name)) = picked {
                pop.popdown();
                entry2.set_text(&name);
                pop.popdown();
                on_pick(id, &name);
            }
        });
    }
    {
        let (hits, pop) = (hits.clone(), pop.clone());
        entry.connect_activate(move |e| {
            let picked = hits.borrow().first().cloned();
            if let Some((id, name)) = picked {
                e.set_text(&name);
                pop.popdown();
                on_pick(id, &name);
            }
        });
    }
    entry
}

/// Systems for `suggest_entry`: name, with security and region.
pub fn search_systems(q: &str) -> Vec<(i64, String, Option<String>)> {
    MAP.search_systems(q, 10)
        .into_iter()
        .map(|s| (s.id, s.name.clone(), Some(format!("{:.1} · {}", s.display_security(), MAP.region_name(s.region).unwrap_or("")))))
        .collect()
}

/// A system's security, colored like EVE does it roughly (high, low, null).
pub fn security_label(system: i64) -> gtk::Label {
    let Some(s) = MAP.system(system) else { return w::line("?", &["dim"]) };
    let class = match s.security_class() {
        SecurityClass::High => "success-text",
        SecurityClass::Low => "warning-text",
        SecurityClass::Null => "error-text",
    };
    w::line(&format!("{:.1}", s.display_security()), &[class])
}

pub fn system_name(system: i64) -> String {
    MAP.system(system).map(|s| s.name.clone()).unwrap_or_else(|| format!("System #{system}"))
}
