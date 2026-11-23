//! The main window: Material navigation drawer, top app bar, page area and snackbar.
//! Pages are rebuilt when their data changes (see [`Page::live`]), keeping the scroll position.

use crate::ctx::{CharTab, Ctx, CtxInner, Page, Route};
use crate::widgets::{self as w, Btn};
use crate::{login, pages, services};
use gtk::{gdk, glib, prelude::*};
use podstack_core::{config, Core, Event};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

struct Shell {
    ctx: Ctx,
    drawer: gtk::Revealer,
    drawer_box: gtk::Box,
    title: gtk::Label,
    back: gtk::Button,
    actions: gtk::Box,
    content: gtk::Box,
    scroller: RefCell<Option<gtk::ScrolledWindow>>,
    history: RefCell<Vec<Route>>,
    live: Cell<bool>,
    rebuild_pending: Cell<bool>,
    drawer_pending: Cell<bool>,
    snackbar: gtk::Revealer,
    snack_label: gtk::Label,
    snack_timer: RefCell<Option<glib::SourceId>>,
}

pub fn build(app: &gtk::Application, core: Arc<Core>) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(config::APP_NAME)
        .default_width(1440)
        .default_height(920)
        .icon_name(config::APP_ID)
        .build();
    window.set_size_request(760, 520);
    if let Ok(t) = std::env::var("PODSTACK_DEBUG_WIDTHS") {
        debug_widths(&window, t.parse().unwrap_or(600));
    }

    let ctx = Ctx(Rc::new(CtxInner {
        core: core.clone(),
        window: window.clone(),
        navigate: RefCell::new(None),
        rebuild: RefCell::new(None),
        toast: RefCell::new(None),
        route: RefCell::new(Route::Overview),
        memory: RefCell::new(Default::default()),
    }));

    // drawer
    let drawer_box = w::vbox(0);
    drawer_box.add_css_class("nav-drawer");
    let drawer_scroll = w::scrolled(&drawer_box);
    drawer_scroll.set_size_request(280, -1);
    let drawer = gtk::Revealer::new();
    drawer.set_transition_type(gtk::RevealerTransitionType::SlideRight);
    // Tranquility status sits under the scrolling list, outside the part that's rebuilt when data changes, so it
    // polls ESI once a minute instead of on every rebuild.
    drawer_scroll.set_vexpand(true);
    let status = pages::about::server_status_widget(&ctx);
    status.add_css_class("nav-status");
    let drawer_col = w::vbox(0);
    drawer_col.append(&drawer_scroll);
    drawer_col.append(&status);
    drawer.set_child(Some(&drawer_col));
    drawer.set_reveal_child(true);
    // Keep the drawer at its own width: an expanding widget inside it would otherwise make it share the window's
    // extra width with the page.
    drawer.set_hexpand(false);
    drawer_scroll.set_hexpand(false);

    // top app bar
    let bar = w::hbox(8);
    bar.add_css_class("top-app-bar");
    let menu = w::icon_button("menu", "Show or hide navigation");
    let back = w::icon_button("arrow_back", "Back");
    let title = w::line("", &["title"]);
    title.set_hexpand(true);
    let actions = w::hbox(8);
    let clock = w::line("", &["label-large", "dim", "numeric"]);
    clock.set_tooltip_text(Some("EVE time (UTC)"));
    bar.append(&menu);
    bar.append(&back);
    bar.append(&title);
    bar.append(&actions);
    bar.append(&update_button(&ctx, &core));
    bar.append(&clock);
    let tick = move || clock.set_text(&format!("EVE {}", chrono::Utc::now().format("%H:%M")));
    tick();
    glib::timeout_add_seconds_local(20, move || {
        tick();
        glib::ControlFlow::Continue
    });

    let content = w::vbox(0);
    content.set_vexpand(true);
    content.set_hexpand(true);

    let main = w::vbox(0);
    main.set_hexpand(true);
    main.append(&bar);
    main.append(&content);

    let root = w::hbox(0);
    root.append(&drawer);
    root.append(&main);

    // snackbar
    let snack_label = w::text("", &["body-medium"]);
    snack_label.set_hexpand(true);
    let snack_box = w::hbox(8);
    snack_box.add_css_class("snackbar");
    snack_box.append(&snack_label);
    let dismiss = w::button("Dismiss", Btn::Text);
    snack_box.append(&dismiss);
    let snackbar = gtk::Revealer::new();
    snackbar.set_transition_type(gtk::RevealerTransitionType::SlideUp);
    snackbar.set_child(Some(&snack_box));
    snackbar.set_halign(gtk::Align::Center);
    snackbar.set_valign(gtk::Align::End);
    snackbar.set_margin_bottom(24);
    snack_box.set_size_request(344, -1);
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&root));
    overlay.add_overlay(&snackbar);
    window.set_child(Some(&overlay));

    let shell = Rc::new(Shell {
        ctx: ctx.clone(),
        drawer: drawer.clone(),
        drawer_box,
        title,
        back: back.clone(),
        actions,
        content,
        scroller: RefCell::new(None),
        history: RefCell::new(vec![]),
        live: Cell::new(true),
        rebuild_pending: Cell::new(false),
        drawer_pending: Cell::new(false),
        snackbar: snackbar.clone(),
        snack_label,
        snack_timer: RefCell::new(None),
    });

    {
        let weak = Rc::downgrade(&shell);
        *ctx.0.navigate.borrow_mut() = Some(Box::new(move |r| {
            if let Some(s) = weak.upgrade() {
                s.navigate(r, true);
            }
        }));
        let weak = Rc::downgrade(&shell);
        *ctx.0.rebuild.borrow_mut() = Some(Box::new(move || {
            if let Some(s) = weak.upgrade() {
                s.show(true);
            }
        }));
        let weak = Rc::downgrade(&shell);
        *ctx.0.toast.borrow_mut() = Some(Box::new(move |m| {
            if let Some(s) = weak.upgrade() {
                s.toast(m);
            }
        }));
    }

    menu.connect_clicked(glib::clone!(#[weak] drawer, move |_| drawer.set_reveal_child(!drawer.reveals_child())));
    {
        let weak = Rc::downgrade(&shell);
        back.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() {
                s.go_back();
            }
        });
    }
    {
        let s = snackbar.clone();
        dismiss.connect_clicked(move |_| s.set_reveal_child(false));
    }

    // keyboard shortcuts
    let keys = gtk::EventControllerKey::new();
    {
        let weak = Rc::downgrade(&shell);
        keys.connect_key_pressed(move |_, key, _, mods| {
            let Some(s) = weak.upgrade() else { return glib::Propagation::Proceed };
            let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
            let alt = mods.contains(gdk::ModifierType::ALT_MASK);
            match key {
                gdk::Key::comma if ctrl => s.navigate(Route::Settings, true),
                gdk::Key::Left if alt => s.go_back(),
                gdk::Key::BackSpace if alt => s.go_back(),
                gdk::Key::b if ctrl => s.drawer.set_reveal_child(!s.drawer.reveals_child()),
                gdk::Key::r if ctrl => {
                    if let Route::Character(id, _) = s.ctx.route() {
                        match s.ctx.core().refresh_now(id) {
                            Ok(()) => s.toast("Refreshing from ESI…"),
                            Err(wait) => s.toast(&format!("You can refresh again in {}", podstack_core::fmt::duration(wait, 2))),
                        }
                    }
                }
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
    }
    window.add_controller(keys);

    // hide to tray instead of quitting, when enabled
    {
        let core = core.clone();
        window.connect_close_request(move |win| {
            core.flush();
            if services::tray::close_to_tray(&core) {
                win.set_visible(false);
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
    }

    // live updates
    {
        let weak = Rc::downgrade(&shell);
        let mut rx = core.subscribe();
        glib::spawn_future_local(async move {
            loop {
                let ev = match rx.recv().await {
                    Ok(e) => e,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                };
                let Some(s) = weak.upgrade() else { break };
                s.on_event(&ev);
            }
        });
    }

    shell.rebuild_drawer();
    let start = std::env::var("PODSTACK_START_ROUTE").ok().and_then(|r| Route::parse(&r)).unwrap_or(Route::Overview);
    shell.navigate(start, false);
    if core.characters().is_empty() {
        let ctx = ctx.clone();
        glib::idle_add_local_once(move || login::show(&ctx));
    }

    // Notifications and the tray menu open a character through this action.
    {
        let action = gtk::gio::SimpleAction::new("open-character", Some(glib::VariantTy::INT64));
        let weak = Rc::downgrade(&shell);
        let win = window.clone();
        action.connect_activate(move |_, param| {
            win.set_visible(true);
            win.present();
            if let (Some(s), Some(id)) = (weak.upgrade(), param.and_then(|p| p.get::<i64>())) {
                s.navigate(Route::Character(id, CharTab::Summary), true);
            }
        });
        app.add_action(&action);
    }

    services::alerts::start(core.clone());
    services::tray::start(app, &window, core);
    // keep the shell alive as long as the window
    unsafe { window.set_data("podstack-shell", shell) };
    window
}

impl Shell {
    fn navigate(self: &Rc<Self>, r: Route, push: bool) {
        let prev = self.ctx.route();
        if push && prev != r {
            let mut h = self.history.borrow_mut();
            h.push(prev);
            if h.len() > 50 {
                h.remove(0);
            }
        }
        let same_page = prev == r;
        *self.ctx.0.route.borrow_mut() = r;
        self.show(same_page);
        if !prev.same_destination(&r) {
            self.rebuild_drawer();
        }
    }

    fn go_back(self: &Rc<Self>) {
        let prev = self.history.borrow_mut().pop();
        if let Some(r) = prev {
            self.navigate(r, false);
        }
    }

    /// Builds the current route's page and shows it (keeping the scroll position when `keep_scroll`).
    fn show(self: &Rc<Self>, keep_scroll: bool) {
        let route = self.ctx.route();
        let old_scroll = if keep_scroll { self.scroller.borrow().as_ref().map(|s| s.vadjustment().value()) } else { None };
        let page: Page = pages::build(&self.ctx, route);
        self.title.set_text(&page.title);
        self.ctx.window().set_title(Some(&format!("{} — {}", page.title, config::APP_NAME)));
        self.back.set_visible(!self.history.borrow().is_empty());
        self.live.set(page.live);

        while let Some(c) = self.actions.first_child() {
            self.actions.remove(&c);
        }
        for a in &page.actions {
            // Centered, so a button keeps its own height instead of stretching to the bar's.
            a.set_valign(gtk::Align::Center);
            self.actions.append(a);
        }
        while let Some(c) = self.content.first_child() {
            self.content.remove(&c);
        }
        if page.scroll {
            let sw = w::scrolled(&page.content);
            self.content.append(&sw);
            if let Some(v) = old_scroll {
                let adj = sw.vadjustment();
                // the new content has no height until laid out
                glib::idle_add_local_once(move || adj.set_value(v));
            }
            *self.scroller.borrow_mut() = Some(sw);
        } else {
            page.content.set_vexpand(true);
            self.content.append(&page.content);
            *self.scroller.borrow_mut() = None;
        }
    }

    fn on_event(self: &Rc<Self>, ev: &Event) {
        let route = self.ctx.route();
        let relevant = match ev {
            Event::CharacterUpdated(id) => route.shows_character(*id),
            Event::CharactersChanged => true,
            Event::SettingsChanged | Event::Doc(_) => !matches!(route, Route::Settings),
            Event::Toast(m) => {
                self.toast(m);
                false
            }
            Event::RefreshProgress { .. } => false,
        };
        if matches!(ev, Event::CharactersChanged | Event::CharacterUpdated(_) | Event::Doc(_)) {
            self.schedule_drawer();
        }
        if matches!(ev, Event::CharactersChanged) {
            if let Route::Character(id, _) = route {
                if self.ctx.core().character(id).is_none() {
                    self.history.borrow_mut().clear();
                    self.navigate(Route::Overview, false);
                    return;
                }
            }
        }
        if relevant && self.live.get() && !self.rebuild_pending.get() {
            self.rebuild_pending.set(true);
            let weak = Rc::downgrade(self);
            glib::timeout_add_local_once(std::time::Duration::from_millis(800), move || {
                if let Some(s) = weak.upgrade() {
                    s.rebuild_pending.set(false);
                    if s.live.get() {
                        s.show(true);
                    }
                }
            });
        }
    }

    fn schedule_drawer(self: &Rc<Self>) {
        if self.drawer_pending.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || {
            if let Some(s) = weak.upgrade() {
                s.drawer_pending.set(false);
                s.rebuild_drawer();
            }
        });
    }

    fn toast(self: &Rc<Self>, msg: &str) {
        self.snack_label.set_text(msg);
        self.snackbar.set_reveal_child(true);
        if let Some(id) = self.snack_timer.borrow_mut().take() {
            id.remove();
        }
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(std::time::Duration::from_secs(4), move || {
            if let Some(s) = weak.upgrade() {
                s.snack_timer.borrow_mut().take();
                s.snackbar.set_reveal_child(false);
            }
        });
        *self.snack_timer.borrow_mut() = Some(id);
    }

    fn rebuild_drawer(self: &Rc<Self>) {
        let b = &self.drawer_box;
        while let Some(c) = b.first_child() {
            b.remove(&c);
        }
        let core = self.ctx.core().clone();
        let current = self.ctx.route();

        let head = w::hbox(12);
        head.add_css_class("app-title");
        head.append(&w::app_logo(32));
        head.append(&w::line(config::APP_NAME, &["title-large"]));
        b.append(&head);

        for (route, label, icon) in Route::DESTINATIONS {
            let badge = pages::nav_badge(&core, route);
            b.append(&self.nav_item(icon, label, badge, current.same_destination(&route), route));
        }

        let div = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        div.add_css_class("nav-divider");
        b.append(&div);
        b.append(&w::line("Characters", &["nav-section-label"]));
        for c in core.characters() {
            let route = Route::Character(c.id, match current {
                Route::Character(_, tab) => tab,
                _ => crate::ctx::CharTab::Summary,
            });
            let btn = gtk::Button::new();
            btn.add_css_class("nav-item");
            btn.add_css_class("compact");
            let row = w::hbox(12);
            row.append(&w::avatar(&c.portrait_url(64), 28));
            let name = w::line(&c.name(), &[]);
            name.set_hexpand(true);
            row.append(&name);
            let needs_login = core.auth.get(c.id).map(|a| a.needs_reauth()).unwrap_or(false);
            if needs_login {
                let warn = w::icon_sized("error", 18);
                warn.add_css_class("error-text");
                warn.set_tooltip_text(Some("Login expired"));
                row.append(&warn);
            } else if c.skill_queue.is_some() && !c.is_training() {
                let warn = w::icon_sized("pause_circle", 18);
                warn.add_css_class("warning-text");
                warn.set_tooltip_text(Some("Not training"));
                row.append(&warn);
            }
            btn.set_child(Some(&row));
            if current.same_destination(&route) {
                btn.add_css_class("active");
            }
            let weak = Rc::downgrade(self);
            btn.connect_clicked(move |_| {
                if let Some(s) = weak.upgrade() {
                    s.navigate(route, true);
                }
            });
            b.append(&btn);
        }
        let add = w::button_icon("person_add", "Add character", Btn::Text);
        add.set_halign(gtk::Align::Start);
        add.set_margin_start(4);
        add.set_margin_top(4);
        {
            let ctx = self.ctx.clone();
            add.connect_clicked(move |_| login::show(&ctx));
        }
        b.append(&add);

        let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        spacer.set_vexpand(true);
        b.append(&spacer);
        let div = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        div.add_css_class("nav-divider");
        b.append(&div);
        b.append(&self.nav_item("settings", "Settings", None, current == Route::Settings, Route::Settings));
        b.append(&self.nav_item("info", "About", None, current == Route::About, Route::About));
    }

    fn nav_item(self: &Rc<Self>, icon: &str, label: &str, badge: Option<u32>, active: bool, route: Route) -> gtk::Button {
        let btn = gtk::Button::new();
        btn.add_css_class("nav-item");
        let row = w::hbox(12);
        row.append(&w::icon(icon));
        let l = w::line(label, &["label-large"]);
        l.set_hexpand(true);
        row.append(&l);
        if let Some(n) = badge {
            row.append(&w::badge(n));
        }
        btn.set_child(Some(&row));
        if active {
            btn.add_css_class("active");
        }
        let weak = Rc::downgrade(self);
        btn.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() {
                s.navigate(route, true);
            }
        });
        btn
    }
}

/// For UI testing (`PODSTACK_DEBUG_WIDTHS=<px>`): a few seconds in, logs every widget whose minimum width is at
/// least that many pixels, indented by depth, to find what keeps a page from fitting a narrow window.
fn debug_widths(window: &gtk::ApplicationWindow, threshold: i32) {
    fn walk(w: &gtk::Widget, depth: usize, threshold: i32) {
        let (min, _, _, _) = w.measure(gtk::Orientation::Horizontal, -1);
        if min < threshold {
            return;
        }
        let classes = w.css_classes().iter().map(|c| c.to_string()).collect::<Vec<_>>().join(".");
        let text = w.downcast_ref::<gtk::Label>().map(|l| format!(" {:?}", l.text().chars().take(40).collect::<String>())).unwrap_or_default();
        eprintln!("WIDTH {}{} {} .{}{}", "  ".repeat(depth), min, w.type_().name(), classes, text);
        let mut c = w.first_child();
        while let Some(ch) = c {
            walk(&ch, depth + 1, threshold);
            c = ch.next_sibling();
        }
    }
    let window = window.clone();
    glib::timeout_add_seconds_local_once(6, move || {
        if let Some(c) = window.child() {
            walk(&c, 0, threshold);
        }
    });
}

/// A button in the top bar that appears when GitHub has a newer PodStack release, linking its page (Settings →
/// General can turn the check off).
fn update_button(ctx: &Ctx, core: &Arc<Core>) -> gtk::Box {
    use podstack_core::features::updates;
    let slot = w::hbox(0);
    if !core.setting::<updates::UpdateSettings>(updates::SETTINGS_KEY).check {
        return slot;
    }
    let (s, ctx) = (slot.clone(), ctx.clone());
    crate::rt::run(updates::check(), move |result| match result {
        Ok(Some(release)) => {
            tracing::info!("PodStack {} is available", release.version);
            let button = w::button_icon("system_update", &format!("PodStack {}", release.version), Btn::Tonal);
            button.set_tooltip_text(Some("A new version is out: open its release page"));
            button.connect_clicked(move |_| w::open_url(&ctx, &release.url));
            s.append(&button);
        }
        Ok(None) => {}
        Err(e) => tracing::info!("couldn't check for a new version: {e}"),
    });
    slot
}
