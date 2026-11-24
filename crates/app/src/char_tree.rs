//! The navigation drawer's character list: characters in the player's own order and folders (nested to any depth),
//! rearranged by dragging. Folder actions are on a right-click or the folder's "⋮" button; the "Characters" heading
//! has a "⋮" menu for a new folder and for filling folders by account or corporation.
//!
//! The drawer is rebuilt when data changes, so everything here is built from the stored layout
//! (`features::char_folders`, collapsed folders included), and the shell holds rebuilds off while a drag is going on.

use crate::ctx::{CharTab, Ctx, Route};
use crate::widgets::{self as w, Btn};
use gtk::{gdk, glib, prelude::*};
use podstack_core::features::char_folders::{self as folders, Folder, Item, Layout, Node, Target};
use podstack_core::Core;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const CSS: &str = "
/* GTK outlines a hovered drop target by default; the line or highlight below says where the drop goes instead. */
.chartree-row:drop(active), .chartree-end:drop(active) { box-shadow: none; }
.chartree-row { border-top: 2px solid transparent; border-bottom: 2px solid transparent; }
.chartree-row.drop-above { border-top-color: @primary; }
.chartree-row.drop-below { border-bottom-color: @primary; }
.chartree-row.drop-into button.nav-item { background-color: alpha(@primary, 0.16); box-shadow: inset 0 0 0 2px @primary; }
.chartree-end { min-height: 8px; border-top: 2px solid transparent; }
.chartree-end.drop-above { border-top-color: @primary; }
button.nav-item.chartree-folder { min-height: 40px; padding: 0 8px 0 12px; }
button.nav-item.chartree-folder.has-active { color: @primary; }
.chartree-more { min-width: 32px; min-height: 32px; padding: 0; }
.chartree-count { color: @on_surface_variant; }
popover button.chartree-menu-item { border-radius: 0; min-height: 40px; padding: 0 16px; color: @on_surface; font-weight: 400; background: none; box-shadow: none; border: none; }
popover button.chartree-menu-item:hover { background-color: alpha(@on_surface, 0.08); }
popover button.chartree-menu-item.danger { color: @error; }
";

/// What the drawer gives the list: how to open a character, rebuild the drawer and tell it a drag is going on.
pub struct Hooks {
    pub navigate: Box<dyn Fn(Route)>,
    /// Rebuilds the drawer (after a change made here, without waiting for the document event).
    pub rebuild: Box<dyn Fn()>,
    /// Set while a row is being dragged; the shell holds drawer rebuilds off meanwhile.
    pub dragging: Rc<Cell<bool>>,
    /// Called when a drag ends, so a held-off rebuild can run.
    pub drag_ended: Box<dyn Fn()>,
}

struct St {
    ctx: Ctx,
    layout: Layout,
    current: Route,
    hooks: Rc<Hooks>,
    dragged: RefCell<Option<Item>>,
}

#[derive(Clone, Copy, PartialEq)]
enum Drop {
    Above,
    Below,
    Into,
}

/// The "Characters" heading and the list under it.
pub fn build(ctx: &Ctx, current: Route, hooks: Rc<Hooks>) -> gtk::Box {
    crate::theme::add_css(CSS);
    let layout = folders::layout(ctx.core());
    let st = Rc::new(St { ctx: ctx.clone(), layout, current, hooks, dragged: RefCell::new(None) });

    let out = w::vbox(0);
    let head = w::hbox(0);
    let label = w::line("Characters", &["nav-section-label"]);
    label.set_hexpand(true);
    head.append(&label);
    let more = w::icon_button("more_vert", "Organize characters");
    more.add_css_class("chartree-more");
    more.set_margin_top(10);
    more.set_margin_end(8);
    {
        let st = st.clone();
        more.connect_clicked(move |b| list_menu(&st, b));
    }
    head.append(&more);
    out.append(&head);

    let list = w::vbox(0);
    append_nodes(&st, &list, &st.layout.root, None, 0);
    out.append(&list);

    // Dropping below the last row puts it at the end of the top level.
    let end = w::vbox(0);
    end.add_css_class("chartree-end");
    drop_target(&st, &end, None, |_| Some(Target::root_end()), |_, _| Some(Drop::Above));
    out.append(&end);
    out
}

fn append_nodes(st: &Rc<St>, into: &gtk::Box, nodes: &[Node], parent: Option<&str>, depth: i32) {
    for (i, n) in nodes.iter().enumerate() {
        match n {
            Node::Character { id } => {
                if let Some(row) = character_row(st, *id, parent, i, depth) {
                    into.append(&row);
                }
            }
            Node::Folder(f) => into.append(&folder(st, f, parent, i, depth)),
        }
    }
}

fn character_row(st: &Rc<St>, id: i64, parent: Option<&str>, index: usize, depth: i32) -> Option<gtk::Box> {
    let core: &Core = st.ctx.core();
    let c = core.character(id)?;
    let route = Route::Character(id, match st.current {
        Route::Character(_, tab) => tab,
        _ => CharTab::Summary,
    });
    let btn = gtk::Button::new();
    btn.add_css_class("nav-item");
    btn.add_css_class("compact");
    btn.set_hexpand(true);
    let row = w::hbox(12);
    row.append(&w::avatar(&c.portrait_url(64), 28));
    let name = w::line(&c.name(), &[]);
    name.set_hexpand(true);
    row.append(&name);
    if let Some((icon, class, tip)) = warning(core, id) {
        let warn = w::icon_sized(icon, 18);
        warn.add_css_class(class);
        warn.set_tooltip_text(Some(tip));
        row.append(&warn);
    }
    btn.set_child(Some(&row));
    if st.current.same_destination(&route) {
        btn.add_css_class("active");
    }
    {
        let st = st.clone();
        btn.connect_clicked(move |_| (st.hooks.navigate)(route));
    }

    let wrap = w::hbox(0);
    wrap.add_css_class("chartree-row");
    wrap.set_margin_start(depth * 20);
    wrap.append(&btn);
    drag_source(st, &btn, Item::Character(id));
    let parent_owned = parent.map(str::to_string);
    drop_target(
        st,
        &wrap,
        None,
        move |d| {
            Some(Target { parent: parent_owned.clone(), index: if d == Drop::Below { index + 1 } else { index } })
        },
        |h, y| Some(if y < h / 2.0 { Drop::Above } else { Drop::Below }),
    );
    {
        let st = st.clone();
        let anchor = btn.clone();
        let parent = parent.map(str::to_string);
        context_click(&btn, move || character_menu(&st, &anchor, id, parent.as_deref()));
    }
    Some(wrap)
}

/// The badge a character's row shows: a login that stopped working, or an idle skill queue.
fn warning(core: &Core, id: i64) -> Option<(&'static str, &'static str, &'static str)> {
    let c = core.character(id)?;
    if core.auth.get(id).map(|a| a.needs_reauth()).unwrap_or(false) {
        Some(("error", "error-text", "Login expired"))
    } else if c.skill_queue.is_some() && !c.is_training() {
        Some(("pause_circle", "warning-text", "Not training"))
    } else {
        None
    }
}

fn chars_in(nodes: &[Node], out: &mut Vec<i64>) {
    for n in nodes {
        match n {
            Node::Character { id } => out.push(*id),
            Node::Folder(f) => chars_in(&f.children, out),
        }
    }
}

fn folder(st: &Rc<St>, f: &Folder, parent: Option<&str>, index: usize, depth: i32) -> gtk::Box {
    let mut members = Vec::new();
    chars_in(&f.children, &mut members);

    let btn = gtk::Button::new();
    btn.add_css_class("nav-item");
    btn.add_css_class("chartree-folder");
    btn.set_hexpand(true);
    let row = w::hbox(8);
    let chevron = w::icon_sized(if f.collapsed { "chevron_right" } else { "expand_more" }, 20);
    row.append(&chevron);
    row.append(&w::icon_sized(if f.collapsed { "folder" } else { "folder_open" }, 20));
    let name = w::line(&f.name, &["label-large"]);
    name.set_hexpand(true);
    row.append(&name);
    // A collapsed folder shows the worst badge of the characters inside, so nothing goes unnoticed.
    if f.collapsed {
        let core: &Core = st.ctx.core();
        let warn = members.iter().filter_map(|id| warning(core, *id)).min_by_key(|(icon, _, _)| *icon != "error");
        if let Some((icon, class, tip)) = warn {
            let l = w::icon_sized(icon, 18);
            l.add_css_class(class);
            l.set_tooltip_text(Some(tip));
            row.append(&l);
        }
        if members.iter().any(|id| matches!(st.current, Route::Character(c, _) if c == *id)) {
            btn.add_css_class("has-active");
        }
    }
    let count = gtk::Label::new(Some(&members.len().to_string()));
    for c in ["label-medium", "chartree-count", "numeric"] {
        count.add_css_class(c);
    }
    row.append(&count);
    btn.set_child(Some(&row));
    btn.set_tooltip_text(Some(if f.collapsed { "Expand" } else { "Collapse" }));

    let more = w::icon_button("more_vert", "Folder actions");
    more.add_css_class("chartree-more");

    let wrap = w::hbox(0);
    wrap.add_css_class("chartree-row");
    wrap.set_margin_start(depth * 20);
    wrap.append(&btn);
    wrap.append(&more);

    let children = w::vbox(0);
    append_nodes(st, &children, &f.children, Some(&f.id), depth + 1);
    let revealer = gtk::Revealer::new();
    revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    revealer.set_child(Some(&children));
    revealer.set_reveal_child(!f.collapsed);

    {
        let st = st.clone();
        let id = f.id.clone();
        let revealer = revealer.clone();
        btn.connect_clicked(move |b| {
            let collapse = revealer.reveals_child();
            revealer.set_reveal_child(!collapse);
            chevron.set_text(if collapse { "chevron_right" } else { "expand_more" });
            b.set_tooltip_text(Some(if collapse { "Expand" } else { "Collapse" }));
            folders::set_collapsed(st.ctx.core(), &id, collapse);
        });
    }
    {
        let (st, id, anchor) = (st.clone(), f.id.clone(), more.clone());
        more.connect_clicked(move |_| folder_menu(&st, &anchor, &id));
    }
    {
        let (st, id, anchor) = (st.clone(), f.id.clone(), more.clone());
        context_click(&btn, move || folder_menu(&st, &anchor, &id));
    }

    drag_source(st, &btn, Item::Folder(f.id.clone()));
    let parent_owned = parent.map(str::to_string);
    let fid = f.id.clone();
    // "Below" an open folder with something in it looks like the top of its contents, so it goes there.
    let below_inside = !f.collapsed && !f.children.is_empty();
    drop_target(
        st,
        &wrap,
        Some(f.id.clone()),
        move |d| {
            Some(match d {
                Drop::Above => Target { parent: parent_owned.clone(), index },
                Drop::Below if below_inside => Target { parent: Some(fid.clone()), index: 0 },
                Drop::Below => Target { parent: parent_owned.clone(), index: index + 1 },
                Drop::Into => Target { parent: Some(fid.clone()), index: usize::MAX },
            })
        },
        |h, y| {
            Some(if y < h * 0.25 {
                Drop::Above
            } else if y > h * 0.75 {
                Drop::Below
            } else {
                Drop::Into
            })
        },
    );

    let out = w::vbox(0);
    out.append(&wrap);
    out.append(&revealer);
    out
}

/// Makes `widget` draggable as `item`. The controller runs in the capture phase so it gets the press before the
/// button does: a click stays a click, and moving past the drag threshold starts a drag instead.
fn drag_source(st: &Rc<St>, widget: &impl IsA<gtk::Widget>, item: Item) {
    let drag = gtk::DragSource::new();
    drag.set_actions(gdk::DragAction::MOVE);
    drag.set_propagation_phase(gtk::PropagationPhase::Capture);
    let key = match &item {
        Item::Character(id) => format!("c:{id}"),
        Item::Folder(id) => format!("f:{id}"),
    };
    drag.connect_prepare(move |_, _, _| Some(gdk::ContentProvider::for_value(&key.to_value())));
    {
        let st = st.clone();
        let widget = widget.as_ref().clone();
        drag.connect_drag_begin(move |src, _| {
            let paint = gtk::WidgetPaintable::new(Some(&widget));
            src.set_icon(Some(&paint), 24, 20);
            *st.dragged.borrow_mut() = Some(item.clone());
            st.hooks.dragging.set(true);
        });
    }
    {
        let st = st.clone();
        drag.connect_drag_end(move |_, _, _| {
            st.dragged.borrow_mut().take();
            st.hooks.dragging.set(false);
            (st.hooks.drag_ended)();
        });
    }
    widget.add_controller(drag);
}

/// Makes `row` a drop target. `zone(height, y)` says where in the row the pointer is and `target` turns that into a
/// place in the tree. `folder` is the folder the row stands for, so a folder can't be dropped into itself.
fn drop_target(
    st: &Rc<St>,
    row: &impl IsA<gtk::Widget>,
    folder: Option<String>,
    target: impl Fn(Drop) -> Option<Target> + 'static,
    zone: impl Fn(f64, f64) -> Option<Drop> + 'static,
) {
    let row = row.as_ref().clone();
    let drop = gtk::DropTarget::new(String::static_type(), gdk::DragAction::MOVE);
    let clear = {
        let row = row.clone();
        move || {
            for c in ["drop-above", "drop-below", "drop-into"] {
                row.remove_css_class(c);
            }
        }
    };
    let target = Rc::new(target);
    let zone = Rc::new(zone);
    // Where a drop here would go, if it may go there at all.
    let resolve = {
        let (st, row, target, zone) = (st.clone(), row.clone(), target.clone(), zone.clone());
        move |y: f64| -> Option<(Drop, Target)> {
            let d = zone(row.height() as f64, y)?;
            let t = target(d)?;
            let dragged = st.dragged.borrow().clone()?;
            if let Item::Folder(moving) = &dragged {
                // not into itself or anything inside it
                let inside = t.parent.as_deref().is_some_and(|p| st.layout.is_within(p, moving));
                if inside || (folder.as_deref() == Some(moving.as_str()) && d == Drop::Into) {
                    return None;
                }
            }
            Some((d, t))
        }
    };
    let resolve = Rc::new(resolve);
    {
        let (row, resolve, clear) = (row.clone(), resolve.clone(), clear.clone());
        drop.connect_motion(move |_, _, y| {
            clear();
            match resolve(y) {
                Some((d, _)) => {
                    row.add_css_class(match d {
                        Drop::Above => "drop-above",
                        Drop::Below => "drop-below",
                        Drop::Into => "drop-into",
                    });
                    gdk::DragAction::MOVE
                }
                None => gdk::DragAction::empty(),
            }
        });
    }
    {
        let clear = clear.clone();
        drop.connect_leave(move |_| clear());
    }
    {
        let st = st.clone();
        drop.connect_drop(move |_, _, _, y| {
            clear();
            let Some((_, t)) = resolve(y) else { return false };
            let Some(item) = st.dragged.borrow().clone() else { return false };
            let st = st.clone();
            // after the drag has finished, since the rows are rebuilt
            glib::idle_add_local_once(move || {
                folders::move_item(st.ctx.core(), &item, &t);
                (st.hooks.rebuild)();
            });
            true
        });
    }
    row.add_controller(drop);
}

/// Runs `f` on a right-click on `widget`.
fn context_click(widget: &impl IsA<gtk::Widget>, f: impl Fn() + 'static) {
    let g = gtk::GestureClick::new();
    g.set_button(gdk::BUTTON_SECONDARY);
    g.connect_pressed(move |g, _, _, _| {
        g.set_state(gtk::EventSequenceState::Claimed);
        f();
    });
    widget.add_controller(g);
}

type MenuItems = Vec<(String, bool, Box<dyn Fn()>)>;

/// A small menu popover on `anchor` (items: label, dangerous, action); it removes itself once closed.
fn popover_menu(anchor: &impl IsA<gtk::Widget>, items: MenuItems) {
    let pop = gtk::Popover::new();
    pop.set_has_arrow(false);
    let bx = w::vbox(0);
    for (label, danger, f) in items {
        let b = gtk::Button::with_label(&label);
        b.add_css_class("flat");
        b.add_css_class("chartree-menu-item");
        if danger {
            b.add_css_class("danger");
        }
        if let Some(l) = b.child().and_then(|c| c.downcast::<gtk::Label>().ok()) {
            l.set_xalign(0.0);
        }
        let popw = pop.downgrade();
        b.connect_clicked(move |_| {
            if let Some(p) = popw.upgrade() {
                p.popdown();
            }
            f();
        });
        bx.append(&b);
    }
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroll.set_propagate_natural_height(true);
    scroll.set_max_content_height(420);
    scroll.set_child(Some(&bx));
    pop.set_child(Some(&scroll));
    pop.set_parent(anchor);
    pop.connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
    pop.popup();
}

/// Runs a change, then rebuilds the drawer right away.
fn act(st: &Rc<St>, f: impl Fn(&Core) + 'static) -> Box<dyn Fn()> {
    let st = st.clone();
    Box::new(move || {
        f(st.ctx.core());
        (st.hooks.rebuild)();
    })
}

fn list_menu(st: &Rc<St>, anchor: &gtk::Button) {
    let has_folders = st.layout.root.iter().any(|n| matches!(n, Node::Folder(_)));
    let mut items: MenuItems = Vec::new();
    {
        let st2 = st.clone();
        items.push((
            "New folder".into(),
            false,
            Box::new(move || {
                let st = st2.clone();
                name_dialog(&st2.ctx, "New folder", "", "Create", move |name| {
                    folders::create_folder(st.ctx.core(), &name, &Target::root_end());
                    (st.hooks.rebuild)();
                });
            }),
        ));
    }
    for (label, by_account) in [("Group by account", true), ("Group by corporation", false)] {
        let st2 = st.clone();
        items.push((
            label.into(),
            false,
            Box::new(move || {
                let core: &Core = st2.ctx.core();
                let groups = if by_account { folders::account_groups(core) } else { folders::corporation_groups(core) };
                if groups.iter().all(|(_, m)| m.is_empty()) {
                    st2.ctx.toast(if by_account {
                        "No characters are assigned to accounts yet (Queue Health)"
                    } else {
                        "No corporations known yet"
                    });
                    return;
                }
                let apply = act(&st2, move |core| folders::group(core, &groups));
                if has_folders {
                    w::confirm(
                        &st2.ctx,
                        label,
                        "This replaces your folders with one per group. Characters stay in the order they're in now.",
                        "Replace folders",
                        false,
                        apply,
                    );
                } else {
                    apply();
                }
            }),
        ));
    }
    if has_folders {
        let st2 = st.clone();
        items.push((
            "Remove all folders".into(),
            true,
            Box::new(move || {
                let apply = act(&st2, |core| folders::group(core, &[]));
                let body = "Characters stay in the order they're in now.";
                w::confirm(&st2.ctx, "Remove all folders?", body, "Remove", true, apply);
            }),
        ));
    }
    popover_menu(anchor, items);
}

/// Every folder with its depth, in drawer order.
fn all_folders(nodes: &[Node], depth: usize, out: &mut Vec<(String, String, usize)>) {
    for n in nodes {
        if let Node::Folder(f) = n {
            out.push((f.id.clone(), f.name.clone(), depth));
            all_folders(&f.children, depth + 1, out);
        }
    }
}

/// A character's right-click menu: open it, or move it to a folder without dragging.
fn character_menu(st: &Rc<St>, anchor: &gtk::Button, id: i64, parent: Option<&str>) {
    let mut items: MenuItems = Vec::new();
    let mut list = Vec::new();
    all_folders(&st.layout.root, 0, &mut list);
    if parent.is_some() {
        items.push(("Move to top level".into(), false, act(st, move |core| {
            folders::move_item(core, &Item::Character(id), &Target::root_end())
        })));
    }
    for (fid, name, depth) in list {
        if parent == Some(fid.as_str()) {
            continue;
        }
        let label = format!("{}Move to {name}", "    ".repeat(depth));
        items.push((label, false, act(st, move |core| {
            folders::move_item(core, &Item::Character(id), &Target { parent: Some(fid.clone()), index: usize::MAX })
        })));
    }
    {
        let st2 = st.clone();
        items.push((
            "Move to a new folder…".into(),
            false,
            Box::new(move || {
                let st = st2.clone();
                name_dialog(&st2.ctx, "New folder", "", "Create", move |name| {
                    let core: &Core = st.ctx.core();
                    let fid = folders::create_folder(core, &name, &Target::root_end());
                    folders::move_item(core, &Item::Character(id), &Target { parent: Some(fid), index: 0 });
                    (st.hooks.rebuild)();
                });
            }),
        ));
    }
    popover_menu(anchor, items);
}

fn folder_menu(st: &Rc<St>, anchor: &gtk::Button, id: &str) {
    let Some(f) = st.layout.folder(id) else { return };
    let name = f.name.clone();
    let mut items: MenuItems = Vec::new();
    {
        let (st2, id, name) = (st.clone(), id.to_string(), name.clone());
        items.push((
            "Rename".into(),
            false,
            Box::new(move || {
                let (st, id) = (st2.clone(), id.clone());
                name_dialog(&st2.ctx, "Rename folder", &name, "Rename", move |n| {
                    folders::rename_folder(st.ctx.core(), &id, &n);
                    (st.hooks.rebuild)();
                });
            }),
        ));
    }
    {
        let (st2, id) = (st.clone(), id.to_string());
        items.push((
            "New subfolder".into(),
            false,
            Box::new(move || {
                let (st, id) = (st2.clone(), id.clone());
                name_dialog(&st2.ctx, "New subfolder", "", "Create", move |n| {
                    let core: &Core = st.ctx.core();
                    folders::create_folder(core, &n, &Target { parent: Some(id.clone()), index: usize::MAX });
                    folders::set_collapsed(core, &id, false);
                    (st.hooks.rebuild)();
                });
            }),
        ));
    }
    {
        let (st2, id) = (st.clone(), id.to_string());
        let empty = f.children.is_empty();
        items.push((
            "Delete folder".into(),
            true,
            Box::new(move || {
                let id = id.clone();
                let apply = act(&st2, move |core| folders::remove_folder(core, &id));
                if empty {
                    apply();
                } else {
                    w::confirm(
                        &st2.ctx,
                        &format!("Delete “{name}”?"),
                        "What's in it moves up to where the folder was. No character is removed.",
                        "Delete folder",
                        true,
                        apply,
                    );
                }
            }),
        ));
    }
    popover_menu(anchor, items);
}

/// Asks for a folder name; `on_ok` gets it trimmed and not empty.
fn name_dialog(ctx: &Ctx, title: &str, initial: &str, ok_label: &str, on_ok: impl Fn(String) + 'static) {
    let (win, content, actions) = w::dialog(ctx, title, 380);
    content.append(&w::text("Folder name", &["label-large"]));
    let entry = gtk::Entry::new();
    entry.set_text(initial);
    entry.set_placeholder_text(Some("e.g. Industry, PvP, Scouts"));
    content.append(&entry);
    let cancel = w::button("Cancel", Btn::Text);
    let ok = w::button(ok_label, Btn::Filled);
    {
        let win = win.clone();
        cancel.connect_clicked(move |_| win.close());
    }
    let submit = Rc::new({
        let (win, entry) = (win.clone(), entry.clone());
        move || {
            let name = entry.text().trim().to_string();
            if name.is_empty() {
                return;
            }
            on_ok(name);
            win.close();
        }
    });
    {
        let submit = submit.clone();
        ok.connect_clicked(move |_| submit());
    }
    entry.connect_activate(move |_| submit());
    actions.append(&cancel);
    actions.append(&ok);
    win.present();
    entry.grab_focus();
}
