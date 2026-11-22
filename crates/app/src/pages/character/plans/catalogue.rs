//! The skill catalogue: every skill by group, or a flat list while searching ("|" separates alternatives, e.g.
//! "cruiser|frigate"). Clicking a skill shows its prerequisites and training times and adds it to the open plan.

use super::{dur, P};
use crate::widgets::{self as w, Btn};
use gtk::{glib, prelude::*};
use podstack_core::features::plan_tools::{Item, Planner};
use podstack_core::sde::{self, SkillInfo};
use podstack_core::training::roman;

pub(super) fn card(p: &P) -> gtk::Widget {
    let body = w::vbox(8);
    let search = w::search_entry("Search skills…");
    search.set_margin_start(12);
    search.set_margin_end(12);
    if let Some(t) = p.ctx.recall("plans.catalogue.search") {
        search.set_text(&t);
    }
    body.append(&search);
    p.catalogue_hint.set_margin_start(16);
    body.append(&p.catalogue_hint);
    let list = w::vbox(0);
    list.set_margin_start(4);
    list.set_margin_end(4);
    list.set_margin_bottom(8);
    let sw = w::scrolled(&list);
    sw.set_min_content_height(240);
    body.append(&sw);

    let fill = {
        let (pp, list) = (p.clone(), list.clone());
        move |text: &str| {
            while let Some(c) = list.first_child() {
                list.remove(&c);
            }
            let filter = text.trim().to_lowercase();
            if filter.chars().count() > 2 {
                list.add_css_class("catalogue-flat");
                let parts: Vec<&str> = filter.split('|').map(|s| s.trim()).filter(|s| s.chars().count() > 1).collect();
                let mut found: Vec<&SkillInfo> =
                    sde::SKILLS.by_id.values().filter(|s| parts.iter().any(|f| s.name.to_lowercase().contains(f))).collect();
                found.sort_by(|a, b| a.name.cmp(&b.name));
                if found.is_empty() {
                    let l = w::text("No skills match.", &["body-medium", "dim"]);
                    l.set_margin_start(12);
                    l.set_margin_top(8);
                    list.append(&l);
                }
                for s in found {
                    list.append(&skill_row(&pp, s));
                }
            } else {
                list.remove_css_class("catalogue-flat");
                for g in &sde::SKILLS.groups {
                    let mut skills: Vec<&SkillInfo> = g.types.iter().filter_map(|id| sde::SKILLS.get(*id)).collect();
                    if skills.is_empty() {
                        continue;
                    }
                    skills.sort_by(|a, b| a.name.cmp(&b.name));
                    let ex = gtk::Expander::new(Some(&g.name.to_uppercase()));
                    ex.add_css_class("catalogue-group");
                    let inner = w::vbox(0);
                    ex.set_child(Some(&inner));
                    // rows are made on first opening
                    let pp = pp.clone();
                    let ids: Vec<i64> = skills.iter().map(|s| s.type_id).collect();
                    ex.connect_expanded_notify(move |ex| {
                        if ex.is_expanded() && inner.first_child().is_none() {
                            for id in &ids {
                                if let Some(s) = sde::SKILLS.get(*id) {
                                    inner.append(&skill_row(&pp, s));
                                }
                            }
                        }
                    });
                    list.append(&ex);
                }
            }
        }
    };
    fill(&search.text());
    {
        let ctx = p.ctx.clone();
        search.connect_search_changed(move |s| {
            ctx.remember("plans.catalogue.search", s.text().to_string());
            fill(&s.text());
        });
    }
    let c = w::card_with("Skill Catalogue", None, &[], &body);
    c.add_css_class("flush");
    if let Some(head) = c.first_child() {
        head.set_margin_start(16);
        head.set_margin_top(12);
    }
    c.upcast()
}

fn skill_row(p: &P, s: &SkillInfo) -> gtk::Widget {
    let trained = p.character().map(|c| c.trained_skill_level(s.type_id)).unwrap_or(0);
    let b = gtk::Button::new();
    b.add_css_class("flat");
    b.add_css_class("catalogue-skill");
    let row = w::hbox(8);
    let name = w::line(&s.name, &["body-medium"]);
    name.set_hexpand(true);
    row.append(&name);
    if trained > 0 {
        row.append(&w::line(roman(trained), &["label-large", "primary-text"]));
    }
    b.set_child(Some(&row));
    let (pp, id) = (p.clone(), s.type_id);
    b.connect_clicked(move |b| skill_popover(&pp, b, id));
    b.upcast()
}

/// The skill's details and the levels to add.
fn skill_popover(p: &P, anchor: &gtk::Button, id: i64) {
    let Some(s) = sde::SKILLS.get(id) else { return };
    let c = p.character();
    let trained = c.as_ref().map(|c| c.trained_skill_level(id)).unwrap_or(0);
    let injected = c.as_ref().map(|c| c.skill(id).is_some()).unwrap_or(false);
    let plan_open = p.st.borrow().open.is_some();

    let pop = gtk::Popover::new();
    pop.set_position(gtk::PositionType::Right);
    let bx = w::vbox(8);
    bx.set_margin_start(16);
    bx.set_margin_end(16);
    bx.set_margin_top(8);
    bx.set_margin_bottom(8);
    bx.set_size_request(340, -1);
    bx.append(&w::text(&s.name, &["title-medium"]));
    let group = sde::SKILLS.group_of(id).map(|g| g.name.clone()).unwrap_or_default();
    bx.append(&w::text(
        &format!(
            "Rank {} · {} / {} · {group}",
            s.training_time_multiplier,
            s.primary_attribute.name(),
            s.secondary_attribute.name()
        ),
        &["body-small", "dim"],
    ));
    bx.append(&w::text(
        &if trained > 0 {
            format!("Trained to level {}", roman(trained))
        } else if injected {
            "Injected, not trained".into()
        } else {
            "Not injected: needs its skillbook".into()
        },
        &["body-medium"],
    ));
    if !s.required_skills.is_empty() {
        bx.append(&w::text("Prerequisites", &["label-large", "dim"]));
        for r in &s.required_skills {
            let have = c.as_ref().map(|c| c.trained_skill_level(r.id)).unwrap_or(0);
            let row = w::hbox(8);
            let ok = have >= r.level;
            let i = w::icon_sized(if ok { "check_circle" } else { "radio_button_unchecked" }, 18);
            i.add_css_class(if ok { "success-text" } else { "dim" });
            row.append(&i);
            let n = sde::SKILLS.get(r.id).map(|x| x.name.clone()).unwrap_or_else(|| format!("Skill #{}", r.id));
            row.append(&w::line(&format!("{n} {}", roman(r.level)), &["body-medium"]));
            bx.append(&row);
        }
    }

    // training time to each level from now, prerequisites included
    let base = p.base();
    let grid = gtk::Grid::new();
    grid.set_column_spacing(8);
    grid.set_row_spacing(8);
    grid.set_column_homogeneous(true);
    let popw = pop.downgrade();
    let add = |label: String, sub: String, level: u8, prereq: Option<u8>, col: i32, row: i32, enabled: bool| {
        let b = gtk::Button::new();
        b.add_css_class(if enabled { "tonal" } else { "outlined" });
        let v = w::vbox(0);
        v.append(&gtk::Label::new(Some(&label)));
        let sl = w::line(&sub, &["body-small"]);
        sl.set_xalign(0.5);
        v.append(&sl);
        b.set_child(Some(&v));
        b.set_sensitive(enabled && plan_open);
        let (pp, popw) = (p.clone(), popw.clone());
        b.connect_clicked(move |_| {
            if let Some(pop) = popw.upgrade() {
                pop.popdown();
            }
            pp.edit(|pl| pl.plan_skill(id, level, prereq));
        });
        grid.attach(&b, col, row, 1, 1);
    };
    for l in 1..=5u8 {
        let t = Planner::with_items(&base, &[Item::wanted(id, l)]).time;
        let enabled = l > trained;
        add(format!("Level {}", roman(l)), if enabled { dur(t) } else { "trained".into() }, l, None, ((l - 1) % 3) as i32, ((l - 1) / 3) as i32, enabled);
    }
    if !s.required_skills.is_empty() {
        for (k, l) in [3u8, 4, 5].iter().enumerate() {
            let mut pl = Planner::new(&base);
            pl.plan_skill(id, 0, Some(*l));
            let n = pl.skill_count();
            add(
                format!("Prereqs {}", roman(*l)),
                if n == 0 { "done".into() } else { dur(pl.time) },
                0,
                Some(*l),
                k as i32,
                2,
                n > 0,
            );
        }
    }
    bx.append(&grid);
    if !plan_open {
        bx.append(&w::text("Open a plan to add skills.", &["body-small", "warning-text"]));
    }
    if !s.description.is_empty() {
        let d: String = s.description.chars().take(400).collect();
        bx.append(&w::text(&d, &["body-small", "dim"]));
    }
    let close = w::button("Close", Btn::Text);
    close.set_halign(gtk::Align::End);
    {
        let popw = pop.downgrade();
        close.connect_clicked(move |_| {
            if let Some(p) = popw.upgrade() {
                p.popdown();
            }
        });
    }
    bx.append(&close);
    pop.set_child(Some(&bx));
    pop.set_parent(anchor);
    pop.connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
    pop.popup();
}
