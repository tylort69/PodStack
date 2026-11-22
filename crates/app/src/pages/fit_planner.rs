//! Fit planner: paste an EFT/in-game fitting (or EVE's XML export, or a DNA string) and see the skills it needs,
//! what the character is missing, and a plan that gets the hull flyable first, then the rest of the fit.
//!
//! The Plans tab embeds [`panel`]; adding goes through its `add_to_plan` callback (the plan open there).

use crate::ctx::Ctx;
use crate::pages::skill_browser::shared;
use crate::rt;
use crate::widgets::{self as w, BannerKind, Btn, Status};
use gtk::{gio, glib, prelude::*};
use podstack_core::features::fitting::{self, Analysis, ParsedFit, Requirements};
use podstack_core::{config, fmt, training::roman};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const CSS: &str = "
.fit-input-frame { border-radius: 12px; background-color: @surface_container_highest; }
.fit-input-frame textview, .fit-input-frame textview text { background-color: transparent; font-family: monospace; font-size: 13px; }
.fit-input-frame textview { padding: 12px; }
.fit-placeholder { color: @on_surface_variant; font-family: monospace; font-size: 13px; padding: 12px; }
.fit-step-number { background-color: @primary_container; color: @on_primary_container; border-radius: 999px; min-width: 28px; min-height: 28px; font-weight: 500; font-size: 13px; }
";

const PLACEHOLDER: &str = "[Rifter, My Rifter]\nDamage Control I\n200mm AutoCannon I, EMP S\n…\n\nor EVE fitting XML, or a DNA string like 587:2048;1::";

/// What the Plans tab hands the panel: adds (skill, level)s to the plan open there.
pub type AddToPlan = Rc<dyn Fn(Vec<(i64, u8)>)>;

struct FitState {
    ctx: Ctx,
    character_id: i64,
    add_to_plan: AddToPlan,
    buffer: gtk::TextBuffer,
    check: gtk::Button,
    picker: gtk::Box,
    result: gtk::Box,
    fits: RefCell<Vec<ParsedFit>>,
    /// Bumped on every lookup so a slow earlier one doesn't overwrite a newer result.
    generation: Cell<u32>,
}

impl FitState {
    fn text_key(&self) -> String {
        format!("fit_planner.{}.text", self.character_id)
    }

    fn text(&self) -> String {
        let (a, b) = self.buffer.bounds();
        self.buffer.text(&a, &b, false).to_string()
    }
}

/// The fit planner panel for a character. `add_to_plan` is called with the (skill, level) list the user chose to
/// add to the open plan.
pub fn panel(ctx: &Ctx, character_id: i64, add_to_plan: AddToPlan) -> gtk::Widget {
    crate::theme::add_css(shared::CSS);
    crate::theme::add_css(CSS);

    let root = w::vbox(16);
    let buffer = gtk::TextBuffer::new(None);
    let state = Rc::new(FitState {
        ctx: ctx.clone(),
        character_id,
        add_to_plan,
        buffer: buffer.clone(),
        check: w::button_icon("fact_check", "Check fit", Btn::Filled),
        picker: w::hbox(8),
        result: w::vbox(16),
        fits: RefCell::default(),
        generation: Cell::new(0),
    });

    // the input
    let view = gtk::TextView::with_buffer(&buffer);
    view.set_monospace(true);
    view.set_wrap_mode(gtk::WrapMode::None);
    view.set_accepts_tab(false);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_child(Some(&view));
    scroll.set_min_content_height(180);
    scroll.set_max_content_height(320);
    scroll.set_propagate_natural_height(true);
    let placeholder = w::text(PLACEHOLDER, &["fit-placeholder"]);
    placeholder.set_valign(gtk::Align::Start);
    placeholder.set_can_target(false);
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&scroll));
    overlay.add_overlay(&placeholder);
    overlay.add_css_class("fit-input-frame");
    overlay.set_overflow(gtk::Overflow::Hidden);

    let input = w::vbox(12);
    input.append(&overlay);
    let buttons = w::hbox(8);
    buttons.append(&state.check);
    let paste = w::button_icon("content_paste", "Paste", Btn::Tonal);
    paste.set_tooltip_text(Some("Paste from the clipboard and check"));
    buttons.append(&paste);
    let open = w::button_icon("folder_open", "Open file…", Btn::Tonal);
    buttons.append(&open);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    buttons.append(&spacer);
    buttons.append(&state.picker);
    input.append(&buttons);
    root.append(&w::card_with("Ship fitting", Some("EFT (EVE's fitting window, Pyfa), EVE XML or DNA"), &[], &input));
    root.append(&state.result);

    if let Some(t) = ctx.recall(&state.text_key()) {
        buffer.set_text(&t);
    }
    placeholder.set_visible(buffer.char_count() == 0);
    state.check.set_sensitive(buffer.char_count() > 0);
    let s = state.clone();
    buffer.connect_changed(move |b| {
        placeholder.set_visible(b.char_count() == 0);
        s.check.set_sensitive(b.char_count() > 0);
        s.ctx.remember(&s.text_key(), s.text());
    });

    let s = state.clone();
    state.check.connect_clicked(move |_| check(&s));
    let s = state.clone();
    paste.connect_clicked(move |_| {
        let s = s.clone();
        s.ctx.window().clipboard().read_text_async(None::<&gio::Cancellable>, move |r| match r {
            Ok(Some(t)) if !t.trim().is_empty() => {
                s.buffer.set_text(&t);
                check(&s);
            }
            _ => s.ctx.toast("The clipboard has no text"),
        });
    });
    let s = state.clone();
    open.connect_clicked(move |_| open_file(&s));

    // a rebuild of the Plans tab recreates the panel: check the remembered fit again (types are cached)
    if !state.text().trim().is_empty() {
        let s = state.clone();
        glib::idle_add_local_once(move || check(&s));
    }
    root.upcast()
}

fn open_file(state: &Rc<FitState>) {
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    let fits = gtk::FileFilter::new();
    fits.set_name(Some("Ship fittings"));
    for ext in ["xml", "txt", "eft", "cfg"] {
        fits.add_suffix(ext);
    }
    filters.append(&fits);
    let all = gtk::FileFilter::new();
    all.set_name(Some("All files"));
    all.add_pattern("*");
    filters.append(&all);
    let dialog = gtk::FileDialog::builder().title("Open a fitting").modal(true).filters(&filters).build();
    let s = state.clone();
    dialog.open(Some(&state.ctx.window()), None::<&gio::Cancellable>, move |r| {
        let Ok(file) = r else { return };
        let s = s.clone();
        file.load_contents_async(None::<&gio::Cancellable>, move |r| match r {
            Ok((bytes, _)) => {
                s.buffer.set_text(&String::from_utf8_lossy(&bytes));
                check(&s);
            }
            Err(e) => {
                tracing::warn!("reading fit file: {e}");
                show_error(&s, "Couldn't read that file.");
            }
        });
    });
}

fn show_error(state: &FitState, msg: &str) {
    shared::clear(&state.result);
    state.result.append(&w::banner("error_outline", msg, BannerKind::Error));
}

fn check(state: &Rc<FitState>) {
    shared::clear(&state.picker);
    match fitting::parse_fits(&state.text()) {
        Err(e) => {
            state.fits.borrow_mut().clear();
            show_error(state, &e);
        }
        Ok(fits) => {
            if fits.len() > 1 {
                let labels: Vec<String> = fits
                    .iter()
                    .map(|f| format!("{} ({})", f.fit_name.clone().unwrap_or_default(), f.ship.label()))
                    .collect();
                let dd = shared::dropdown(&labels, 0);
                dd.set_tooltip_text(Some(&format!("{} fits in this file", fits.len())));
                let s = state.clone();
                dd.connect_selected_notify(move |d| look_up(&s, d.selected() as usize));
                state.picker.append(&dd);
            }
            *state.fits.borrow_mut() = fits;
            look_up(state, 0);
        }
    }
}

fn look_up(state: &Rc<FitState>, index: usize) {
    let Some(fit) = state.fits.borrow().get(index).cloned() else { return };
    let generation = state.generation.get() + 1;
    state.generation.set(generation);
    shared::clear(&state.result);
    let spinner = w::hbox(12);
    spinner.set_halign(gtk::Align::Center);
    spinner.set_margin_top(16);
    let sp = gtk::Spinner::new();
    sp.start();
    spinner.append(&sp);
    spinner.append(&w::text("Looking up the fit…", &["body-medium", "dim"]));
    state.result.append(&spinner);
    state.check.set_sensitive(false);

    let core = state.ctx.core().clone();
    let s = state.clone();
    let format = fit.format;
    rt::run(async move { fitting::look_up(&core, &fit).await }, move |r| {
        if s.generation.get() != generation {
            return;
        }
        s.check.set_sensitive(s.buffer.char_count() > 0);
        match r {
            Err(e) => show_error(&s, &e),
            Ok(req) => render(&s, format, &req),
        }
    });
}

fn render(state: &Rc<FitState>, format: &str, req: &Requirements) {
    shared::clear(&state.result);
    let Some(c) = state.ctx.core().character(state.character_id) else {
        show_error(state, "This character is gone.");
        return;
    };
    if c.skills.is_none() {
        state.result.append(&w::banner("hourglass_empty", "This character's skills haven't loaded yet.", BannerKind::Notice));
        return;
    }
    let a = fitting::analyse(&c, req);
    state.result.append(&summary(req, format, &a));
    if !req.unknown.is_empty() {
        state.result.append(&w::banner("help_outline", &format!("Not recognised, skipped: {}", req.unknown.join(", ")), BannerKind::Notice));
    }
    state.result.append(&items_card(&a));
    if !a.milestones.is_empty() {
        state.result.append(&plan_card(state, &a));
    }
}

fn summary(req: &Requirements, format: &str, a: &Analysis) -> gtk::Widget {
    let hero = w::hbox(20);
    hero.add_css_class("browser-hero");
    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.add_css_class("browser-render");
    frame.set_overflow(gtk::Overflow::Hidden);
    frame.set_valign(gtk::Align::Center);
    frame.append(&w::eve_image(&config::type_render(req.ship_id, 128), 96));
    hero.append(&frame);

    let texts = w::vbox(6);
    texts.set_valign(gtk::Align::Center);
    texts.set_hexpand(true);
    texts.append(&w::text(&req.ship_name, &["headline-small"]));
    let usable = a.items.iter().filter(|i| i.usable()).count();
    texts.append(&w::text(
        &format!("{} \u{00b7} {format} \u{00b7} {usable} of {} items usable now", req.fit_name, a.items.len()),
        &["body-medium", "dim"],
    ));
    let mut chips: Vec<gtk::Widget> = vec![];
    chips.push(if a.hull_usable() {
        w::status("Can fly hull", Status::Good).upcast()
    } else {
        w::status(&format!("Hull in {}", fmt::duration(a.hull_time(&req.ship_name), 3)), Status::Warn).upcast()
    });
    chips.push(if a.milestones.is_empty() {
        w::status("Ready to fly", Status::Good).upcast()
    } else {
        w::status(&format!("Full fit in {}", fmt::duration(a.total, 3)), Status::Warn).upcast()
    });
    texts.append(&shared::pill_row(chips));
    hero.append(&texts);
    hero.upcast()
}

fn items_card(a: &Analysis) -> gtk::Widget {
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("m3-list");
    for item in &a.items {
        let icon = w::eve_image(&config::type_icon(item.item.type_id, 64), 32);
        let name = if item.item.quantity > 1 { format!("{} \u{00d7}{}", item.item.name, fmt::int(item.item.quantity as i64)) } else { item.item.name.clone() };
        let trailing: gtk::Widget = if item.usable() {
            let b = w::hbox(4);
            let i = w::icon_sized("check", 18);
            i.add_css_class("success-text");
            b.append(&i);
            b.append(&w::line("Ready", &["body-medium", "success-text"]));
            b.upcast()
        } else {
            let b = w::vbox(2);
            for m in &item.missing {
                let have = if m.trained == 0 { "\u{2014}" } else { roman(m.trained) };
                let l = w::markup(
                    &format!(
                        "<span>{} {}</span> <span alpha=\"70%\">(have {have})</span>",
                        fmt::escape(&fitting::skill_name(m.skill_id)),
                        roman(m.level)
                    ),
                    &["body-medium", "warning-text"],
                );
                l.set_xalign(1.0);
                b.append(&l);
            }
            b.upcast()
        };
        list.append(&w::list_row(Some(icon.upcast_ref()), &name, Some(&item.item.category), Some(&trailing)));
    }
    w::card_with("Items", Some("Each item's own skill requirements"), &[], &list).upcast()
}

fn plan_card(state: &Rc<FitState>, a: &Analysis) -> gtk::Widget {
    let body = w::vbox(12);
    body.append(&w::text(
        "Hull first, then the rest of the fit in the order it unlocks fastest. Within each step the quickest skills come first, with prerequisites always ahead of the skills that need them.",
        &["body-medium", "dim"],
    ));

    let steps = gtk::ListBox::new();
    steps.set_selection_mode(gtk::SelectionMode::None);
    steps.add_css_class("m3-list");
    for (i, m) in a.milestones.iter().enumerate() {
        let n = gtk::Label::new(Some(&(i + 1).to_string()));
        n.add_css_class("fit-step-number");
        let time = w::line(&fmt::duration(m.reached, 3), &["body-medium", "numeric", "dim"]);
        let levels = m.levels.len();
        let row = w::list_row(
            Some(n.upcast_ref()),
            &m.label,
            Some(&format!("{levels} level{} \u{00b7} {}", if levels == 1 { "" } else { "s" }, fmt::duration(fitting::total_time(&m.levels), 3))),
            Some(time.upcast_ref()),
        );
        row.set_tooltip_text(Some(&m.items.join("\n")));
        steps.append(&row);
    }
    body.append(&steps);

    let queue = a.queue();
    let exp = gtk::Expander::new(Some(&format!("All {} skill levels", queue.len())));
    exp.set_child(Some(&shared::levels_list(&queue)));
    body.append(&exp);

    let actions = w::hbox(8);
    actions.set_halign(gtk::Align::End);
    let add = w::button_icon("playlist_add", "Add missing skills to plan", Btn::Filled);
    let s = state.clone();
    let wanted: Vec<(i64, u8)> = queue.iter().map(|l| (l.skill_id, l.level)).collect();
    add.connect_clicked(move |_| (s.add_to_plan)(wanted.clone()));
    actions.append(&add);
    body.append(&actions);

    let subtitle = format!("{} levels \u{00b7} {}", queue.len(), fmt::duration(a.total, 3));
    w::card_with("Generated plan", Some(&subtitle), &[], &body).upcast()
}
