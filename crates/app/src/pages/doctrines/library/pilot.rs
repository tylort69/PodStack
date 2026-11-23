//! One character against a doctrine: each fit's standing, what's missing for one fit (or every fit together), and
//! the skill plan, to copy in EVE's format or put into a PodStack skill plan.

use super::doctrine::{check_pill, ship_line};
use super::{plural, ship_icon, Lib};
use crate::pages::skill_browser::shared;
use crate::widgets::{self as w, BannerKind, Btn};
use gtk::prelude::*;
use podstack_core::features::doctrines::{self as dc, DoctrineFit};
use podstack_core::features::fitting::{self, PlannedLevel};
use podstack_core::features::plans;
use podstack_core::{config, fmt, training::roman, Character};
use std::rc::Rc;
use std::sync::Arc;

/// The levels a character needs: for one fit in the fit planner's order (hull first), for several each level once.
fn levels(c: &Character, fits: &[&DoctrineFit]) -> Vec<PlannedLevel> {
    match fits {
        [one] => dc::plan_for(c, one),
        _ => dc::plan_for_all(c, fits),
    }
}

fn character(lib: &Lib, id: i64) -> Option<Arc<Character>> {
    let c = lib.ctx.core().character(id);
    if c.as_ref().is_none_or(|c| c.skills.is_none()) {
        lib.ctx.toast("That character's skills haven't loaded yet");
        return None;
    }
    c
}

/// Copies the skill plan in EVE's format (the Skills window imports it from the clipboard).
pub(super) fn copy_plan(lib: &Lib, character_id: i64, fits: &[&DoctrineFit]) {
    let Some(c) = character(lib, character_id) else { return };
    let l = levels(&c, fits);
    if l.is_empty() {
        lib.ctx.toast(&format!("{} already has every skill for that", c.name()));
        return;
    }
    lib.ctx.window().clipboard().set_text(&dc::eve_plan_text(&l));
    lib.ctx.toast(&format!("Copied {} for {}: paste it in EVE's skill plans", plural(l.len(), "skill level", "skill levels"), c.name()));
}

/// Creates a PodStack skill plan called `name` with the levels the character needs.
pub(super) fn add_plan(lib: &Lib, character_id: i64, name: &str, fits: &[&DoctrineFit]) {
    let Some(c) = character(lib, character_id) else { return };
    let l = levels(&c, fits);
    if l.is_empty() {
        lib.ctx.toast(&format!("{} already has every skill for that", c.name()));
        return;
    }
    let core = lib.ctx.core();
    let plan = plans::create(core, character_id, name);
    let wanted: Vec<(i64, u8)> = l.iter().map(|x| (x.skill_id, x.level)).collect();
    let added = plans::add_skills(core, character_id, &plan, &wanted);
    lib.ctx.toast(&format!("New plan \u{201c}{name}\u{201d} for {} with {}", c.name(), plural(added, "skill level", "skill levels")));
}

/// What the character is missing for a doctrine. `fit_id` picks the fit to start on (else their closest).
pub(super) fn dialog(lib: &Rc<Lib>, character_id: i64, doctrine_id: &str, fit_id: Option<&str>) {
    let doc = lib.doc();
    let Some((path, _, d)) = doc.doctrine(doctrine_id) else { return };
    let Some(c) = character(lib, character_id) else { return };
    let fits: Vec<DoctrineFit> = d.fits.clone();
    if fits.is_empty() {
        return;
    }
    // one character against one doctrine is quick: check now rather than rely on the page's (maybe older) checks
    let refs: Vec<&DoctrineFit> = fits.iter().collect();
    let checks = dc::matrix(std::slice::from_ref(&c), &refs);
    let start = fit_id
        .and_then(|id| fits.iter().position(|f| f.id == id))
        .or_else(|| dc::closest_fit(&checks, c.id, &refs).and_then(|(f, _)| fits.iter().position(|x| x.id == f.id)))
        .unwrap_or(0);

    let (win, content, actions) = w::dialog(&lib.ctx, &format!("{} \u{00b7} {}", c.name(), d.name), 600);
    win.set_default_height(640);

    let head = w::hbox(16);
    head.append(&w::avatar(&config::character_portrait(c.id, 128), 56));
    let texts = w::vbox(2);
    texts.set_valign(gtk::Align::Center);
    texts.append(&w::line(&c.name(), &["title-large"]));
    texts.append(&w::line(&format!("{} \u{203a} {}", path.join(" \u{203a} "), d.name), &["body-medium", "dim"]));
    head.append(&texts);
    content.append(&head);

    let mut labels: Vec<String> = fits
        .iter()
        .map(|f| {
            let state = checks.get(&(c.id, f.id.clone())).map(|x| x.readiness.label()).unwrap_or("");
            format!("{} ({}) \u{2014} {state}", f.name, f.ship_name)
        })
        .collect();
    labels.push(format!("Every fit of the doctrine ({})", fits.len()));
    let dd = shared::dropdown(&labels, start as u32);
    dd.set_hexpand(true);
    content.append(&super::dialogs::labelled("Skill plan for", &dd));

    let body = w::vbox(12);
    let scroll = w::scrolled(&body);
    scroll.set_min_content_height(320);
    content.append(&scroll);

    let show = {
        let (lib, body, c, fits) = (lib.clone(), body.clone(), c.clone(), fits.clone());
        let doctrine = d.name.clone();
        move |index: usize| {
            shared::clear(&body);
            let chosen: Vec<&DoctrineFit> = match fits.get(index) {
                Some(f) => vec![f],
                None => fits.iter().collect(),
            };
            render(&lib, &body, &c, &doctrine, &chosen);
        }
    };
    show(start);
    dd.connect_selected_notify(move |dd| show(dd.selected() as usize));

    let close = w::button("Close", Btn::Text);
    let wc = win.clone();
    close.connect_clicked(move |_| wc.close());
    actions.append(&close);
    win.present();
}

fn render(lib: &Rc<Lib>, body: &gtk::Box, c: &Character, doctrine: &str, fits: &[&DoctrineFit]) {
    // each fit's standing
    let list = w::vbox(4);
    for f in fits {
        let x = dc::check(c, f);
        let row = w::hbox(12);
        row.append(&ship_icon(f.ship_id, 32));
        let texts = w::vbox(0);
        texts.set_hexpand(true);
        texts.append(&w::line(&f.name, &["body-medium"]));
        texts.append(&w::line(&ship_line(f), &["body-small", "dim"]));
        row.append(&texts);
        let pill = check_pill(&x);
        pill.set_tooltip_text(Some(&super::doctrine::check_tooltip(&x)));
        row.append(&pill);
        list.append(&row);
    }
    body.append(&list);

    // for one fit, the items it can't use yet and why
    if let [f] = fits {
        let a = fitting::analyse(c, &f.requirements());
        let missing: Vec<_> = a.items.iter().filter(|i| !i.usable()).collect();
        if !missing.is_empty() {
            body.append(&w::text("Can't use yet", &["title-small"]));
            let items = w::vbox(6);
            for i in missing {
                let row = w::hbox(12);
                row.append(&w::eve_image(&config::type_icon(i.item.type_id, 64), 28));
                let name = w::line(&i.item.name, &["body-medium"]);
                name.set_hexpand(true);
                row.append(&name);
                let need: Vec<String> = i
                    .missing
                    .iter()
                    .map(|m| format!("{} {} (have {})", fitting::skill_name(m.skill_id), roman(m.level), if m.trained == 0 { "\u{2014}" } else { roman(m.trained) }))
                    .collect();
                let n = w::text(&need.join("\n"), &["body-small", "warning-text"]);
                n.set_xalign(1.0);
                n.set_justify(gtk::Justification::Right);
                row.append(&n);
                items.append(&row);
            }
            body.append(&items);
        }
    }

    let levels = levels(c, fits);
    if levels.is_empty() {
        let msg = match fits {
            [f] => format!("{} can fly {} as it is.", c.name(), f.name),
            _ => format!("{} can fly every fit of {doctrine}.", c.name()),
        };
        body.append(&w::banner("check_circle", &msg, BannerKind::Info));
        return;
    }

    let total = fitting::total_time(&levels);
    let head = w::hbox(8);
    let t = w::text("Skills to train", &["title-small"]);
    t.set_hexpand(true);
    head.append(&t);
    head.append(&w::line(&format!("{} \u{00b7} {}", plural(levels.len(), "level", "levels"), fmt::duration(total, 3)), &["body-medium", "dim", "numeric"]));
    body.append(&head);
    body.append(&shared::levels_list(&levels));

    let copy = w::button_icon("content_copy", "Copy skill plan", Btn::Tonal);
    copy.set_tooltip_text(Some("EVE's skill plan format: in EVE, Skills \u{203a} Skill plans \u{203a} Import from clipboard"));
    copy.set_halign(gtk::Align::Start);
    let ctx = lib.ctx.clone();
    let text = dc::eve_plan_text(&levels);
    copy.connect_clicked(move |_| w::copy_to_clipboard(&ctx, &text));
    body.append(&copy);

    let plan_name = match fits {
        [f] => format!("{doctrine} \u{2014} {}", f.name),
        _ => doctrine.to_string(),
    };
    let note = Some(format!("For {plan_name}"));
    body.append(&shared::add_to_plan_row(&lib.ctx, c.id, &plan_name, note, &levels));
}
