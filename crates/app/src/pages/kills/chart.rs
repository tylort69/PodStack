//! Paired bar charts for the Kills & Losses page: kills (primary) beside losses (tertiary) per slot, drawn like the
//! Wallet's income chart, with the hovered slot highlighted and its numbers in a tooltip card.

use crate::pages::character::wallet::source;
use crate::pages::contracts::overview::bar;
use crate::widgets as w;
use gtk::prelude::*;
use podstack_core::features::comms;
use podstack_core::fmt;
use std::cell::Cell;
use std::rc::Rc;

/// One bar pair: its axis label, the two values, and the tooltip's title and detail lines.
pub struct Slot {
    pub label: String,
    pub kills: f64,
    pub losses: f64,
    pub title: String,
    pub detail: String,
}

const PAD_L: f64 = 56.0;
const PAD_R: f64 = 12.0;
const PAD_T: f64 = 12.0;
const PAD_B: f64 = 26.0;

/// `isk` formats the axis as ISK, else as counts. Up to `max_labels` slot labels go under the axis, spread evenly.
pub fn pairs(slots: Vec<Slot>, height: i32, isk: bool, max_labels: usize) -> gtk::Widget {
    let n = slots.len().max(1);
    let top = slots.iter().map(|s| s.kills.max(s.losses)).fold(0.0f64, f64::max).max(if isk { 1.0 } else { 4.0 }) * 1.08;
    let ticks: Vec<f64> = comms::nice_ticks(0.0, top, 4).into_iter().filter(|t| isk || t.fract() == 0.0).collect();
    let top = ticks.last().copied().unwrap_or(top).max(top);
    let slots = Rc::new(slots);
    let hover: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));
    let plot_w = |w: f64| (w - PAD_L - PAD_R).max(1.0);
    let slot_w = move |w: f64| plot_w(w) / n as f64;
    let y_of = move |h: f64, v: f64| PAD_T + (1.0 - v / top) * (h - PAD_T - PAD_B).max(1.0);
    let axis = move |v: f64| if isk { fmt::isk_short(v) } else { fmt::int(v.round() as i64) };

    let area = gtk::DrawingArea::new();
    area.set_content_height(height);
    area.set_hexpand(true);
    {
        let (slots, hover) = (slots.clone(), hover.clone());
        area.set_draw_func(move |_, cr, width, height| {
            let (wd, ht) = (width as f64, height as f64);
            cr.select_font_face("Inter", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Normal);
            cr.set_font_size(11.0);
            for v in &ticks {
                let y = y_of(ht, *v).round() + 0.5;
                source(cr, "outline_variant", if *v == 0.0 { 1.0 } else { 0.5 });
                cr.set_line_width(1.0);
                cr.move_to(PAD_L, y);
                cr.line_to(PAD_L + plot_w(wd), y);
                let _ = cr.stroke();
                let text = axis(*v);
                if let Ok(ext) = cr.text_extents(&text) {
                    source(cr, "on_surface_variant", 1.0);
                    cr.move_to(PAD_L - 8.0 - ext.width() - ext.x_bearing(), y + ext.height() / 2.0);
                    let _ = cr.show_text(&text);
                }
            }
            let s = slot_w(wd);
            let count = ((plot_w(wd) / 56.0).floor() as usize).clamp(2, max_labels.max(2)).min(slots.len());
            for k in 0..count {
                let i = if count <= 1 { 0 } else { k * (slots.len() - 1) / (count - 1) };
                let text = &slots[i].label;
                if let Ok(ext) = cr.text_extents(text) {
                    let x = (PAD_L + (i as f64 + 0.5) * s - ext.width() / 2.0).clamp(PAD_L, (PAD_L + plot_w(wd) - ext.width()).max(PAD_L));
                    source(cr, "on_surface_variant", 1.0);
                    cr.move_to(x, ht - 8.0);
                    let _ = cr.show_text(text);
                }
            }
            if let Some(i) = hover.get() {
                source(cr, "on_surface", 0.06);
                cr.rectangle(PAD_L + i as f64 * s, PAD_T, s, ht - PAD_T - PAD_B);
                let _ = cr.fill();
            }
            let zero = y_of(ht, 0.0);
            let bw = (s * 0.36).clamp(1.0, 18.0);
            for (i, b) in slots.iter().enumerate() {
                let cx = PAD_L + (i as f64 + 0.5) * s;
                let dim = hover.get().is_some_and(|h| h != i);
                for (v, role, x) in [(b.kills, "primary", cx - bw - 1.0), (b.losses, "tertiary", cx + 1.0)] {
                    if v <= 0.0 {
                        continue;
                    }
                    let y = y_of(ht, v);
                    source(cr, role, if dim { 0.45 } else { 1.0 });
                    bar(cr, x, y, bw, (zero - y).max(1.0));
                    let _ = cr.fill();
                }
            }
        });
    }

    let tip = w::vbox(2);
    tip.add_css_class("comms-chart-tip");
    tip.set_halign(gtk::Align::Start);
    tip.set_valign(gtk::Align::Start);
    tip.set_can_target(false);
    tip.set_visible(false);
    let tip_title = w::line("", &["body-small", "dim"]);
    let tip_detail = w::line("", &["body-small"]);
    tip.append(&tip_title);
    tip.append(&tip_detail);
    let motion = gtk::EventControllerMotion::new();
    {
        let (area, slots, hover, tip) = (area.clone(), slots.clone(), hover.clone(), tip.clone());
        motion.connect_motion(move |_, px, _| {
            let wd = area.width() as f64;
            let s = slot_w(wd);
            let i = ((px - PAD_L) / s).floor();
            if i < 0.0 || i as usize >= slots.len() {
                return;
            }
            let i = i as usize;
            if hover.get() == Some(i) {
                return;
            }
            hover.set(Some(i));
            tip_title.set_text(&slots[i].title);
            tip_detail.set_text(&slots[i].detail);
            let x = PAD_L + (i as f64 + 0.5) * s;
            let left = if x + 240.0 > wd { x - 232.0 } else { x + 12.0 };
            tip.set_margin_start(left.max(0.0) as i32);
            tip.set_margin_top(PAD_T as i32);
            tip.set_visible(true);
            area.queue_draw();
        });
    }
    {
        let (area, hover, tip) = (area.clone(), hover.clone(), tip.clone());
        motion.connect_leave(move |_| {
            hover.set(None);
            tip.set_visible(false);
            area.queue_draw();
        });
    }
    area.add_controller(motion);
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&area));
    overlay.add_overlay(&tip);
    overlay.upcast()
}

/// The two series' colors, for a chart's subtitle row.
pub fn legend(kills: &str, losses: &str) -> gtk::Widget {
    let row = w::hbox(16);
    row.set_valign(gtk::Align::Center);
    for (role, label) in [("kl-swatch-kill", kills), ("kl-swatch-loss", losses)] {
        let item = w::hbox(6);
        let dot = w::hbox(0);
        dot.add_css_class("kl-swatch");
        dot.add_css_class(role);
        dot.set_valign(gtk::Align::Center);
        item.append(&dot);
        let l = w::line(label, &["body-small", "dim"]);
        l.set_ellipsize(gtk::pango::EllipsizeMode::None);
        item.append(&l);
        row.append(&item);
    }
    row.upcast()
}
