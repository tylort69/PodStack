//! What pages get: the [`Ctx`] (core data, navigation, snackbars) and the [`Page`] they return.

use gtk::prelude::*;
use podstack_core::Core;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CharTab {
    Summary,
    Skills,
    Plans,
    Clones,
    Mail,
    Wallet,
    Market,
    Contracts,
    Assets,
    Industry,
    Research,
    Planets,
    Notifications,
    Calendar,
    Standings,
    Contacts,
    Kills,
    Medals,
    Esi,
}

impl CharTab {
    pub const ALL: [CharTab; 19] = [
        CharTab::Summary,
        CharTab::Skills,
        CharTab::Plans,
        CharTab::Clones,
        CharTab::Mail,
        CharTab::Wallet,
        CharTab::Market,
        CharTab::Contracts,
        CharTab::Assets,
        CharTab::Industry,
        CharTab::Research,
        CharTab::Planets,
        CharTab::Notifications,
        CharTab::Calendar,
        CharTab::Standings,
        CharTab::Contacts,
        CharTab::Kills,
        CharTab::Medals,
        CharTab::Esi,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CharTab::Summary => "Summary",
            CharTab::Skills => "Skills",
            CharTab::Plans => "Plans",
            CharTab::Clones => "Clones",
            CharTab::Mail => "Mail",
            CharTab::Wallet => "Wallet",
            CharTab::Market => "Market",
            CharTab::Contracts => "Contracts",
            CharTab::Assets => "Assets",
            CharTab::Industry => "Industry",
            CharTab::Research => "Research",
            CharTab::Planets => "PI",
            CharTab::Notifications => "Notifications",
            CharTab::Calendar => "Calendar",
            CharTab::Standings => "Standings",
            CharTab::Contacts => "Contacts",
            CharTab::Kills => "Kill Log",
            CharTab::Medals => "Medals",
            CharTab::Esi => "ESI",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            CharTab::Summary => "assessment",
            CharTab::Skills => "library_books",
            CharTab::Plans => "format_list_numbered",
            CharTab::Clones => "people_outline",
            CharTab::Mail => "mail",
            CharTab::Wallet => "account_balance_wallet",
            CharTab::Market => "storefront",
            CharTab::Contracts => "assignment",
            CharTab::Assets => "inventory_2",
            CharTab::Industry => "precision_manufacturing",
            CharTab::Research => "science",
            CharTab::Planets => "public",
            CharTab::Notifications => "notifications",
            CharTab::Calendar => "event",
            CharTab::Standings => "handshake",
            CharTab::Contacts => "contacts",
            CharTab::Kills => "gps_fixed",
            CharTab::Medals => "military_tech",
            CharTab::Esi => "vpn_key",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Overview,
    SkillBrowser,
    ShipBrowser,
    QueueHealth,
    SpFarming,
    Contracts,
    AssetSearch,
    Market,
    Industry,
    Planets,
    Settings,
    About,
    Character(i64, CharTab),
}

impl Route {
    /// Top-level destinations in the navigation drawer: (route, label, icon).
    pub const DESTINATIONS: [(Route, &'static str, &'static str); 10] = [
        (Route::Overview, "Overview", "dashboard"),
        (Route::SkillBrowser, "Skill Browser", "account_tree"),
        (Route::ShipBrowser, "Ship Browser", "rocket_launch"),
        (Route::QueueHealth, "Queue Health", "monitor_heart"),
        (Route::SpFarming, "SP Farming", "opacity"),
        (Route::Contracts, "Contracts", "assignment"),
        (Route::AssetSearch, "Asset Search", "manage_search"),
        (Route::Market, "Market", "storefront"),
        (Route::Industry, "Industry", "precision_manufacturing"),
        (Route::Planets, "Planetary Industry", "public"),
    ];

    /// Parses a route for `PODSTACK_START_ROUTE` (for UI testing): a destination label ("SP Farming", "Settings",
    /// "About") or "<character id>/<tab label>" ("90000001/Plans"). Case and spaces don't matter.
    pub fn parse(s: &str) -> Option<Route> {
        let norm = |x: &str| x.to_lowercase().replace(' ', "");
        let s = norm(s);
        if let Some((id, tab)) = s.split_once('/') {
            let id = id.parse().ok()?;
            let tab = CharTab::ALL.into_iter().find(|t| norm(t.label()) == tab)?;
            return Some(Route::Character(id, tab));
        }
        Route::DESTINATIONS
            .iter()
            .map(|(r, label, _)| (*r, *label))
            .chain([(Route::Settings, "Settings"), (Route::About, "About")])
            .find(|(_, label)| norm(label) == s)
            .map(|(r, _)| r)
    }

    /// Whether data about character `id` changing affects this page.
    pub fn shows_character(&self, id: i64) -> bool {
        match self {
            Route::Character(c, _) => *c == id,
            Route::Settings | Route::About => false,
            _ => true,
        }
    }

    /// Same destination ignoring the character tab (for highlighting the drawer).
    pub fn same_destination(&self, other: &Route) -> bool {
        match (self, other) {
            (Route::Character(a, _), Route::Character(b, _)) => a == b,
            _ => self == other,
        }
    }
}

/// What a page builder returns.
pub struct Page {
    pub title: String,
    pub content: gtk::Widget,
    /// Wrap in a vertical ScrolledWindow (default). Pages with their own scrolling (big tables) set false and
    /// make `content` vexpand.
    pub scroll: bool,
    /// Rebuild automatically when the data this page shows changes (default). The scroll position is kept.
    /// Pages that update themselves, or hold unsaved input, set false.
    pub live: bool,
    /// Extra buttons for the top app bar.
    pub actions: Vec<gtk::Widget>,
}

impl Page {
    pub fn new(title: impl Into<String>, content: impl IsA<gtk::Widget>) -> Self {
        Page { title: title.into(), content: content.upcast(), scroll: true, live: true, actions: vec![] }
    }
    pub fn no_scroll(mut self) -> Self {
        self.scroll = false;
        self
    }
    pub fn static_page(mut self) -> Self {
        self.live = false;
        self
    }
    pub fn action(mut self, w: impl IsA<gtk::Widget>) -> Self {
        self.actions.push(w.upcast());
        self
    }
}

/// A callback the shell installs once the window exists.
type Hook<F> = RefCell<Option<Box<F>>>;

pub(crate) struct CtxInner {
    pub core: Arc<Core>,
    pub window: gtk::ApplicationWindow,
    pub navigate: Hook<dyn Fn(Route)>,
    pub rebuild: Hook<dyn Fn()>,
    pub toast: Hook<dyn Fn(&str)>,
    pub route: RefCell<Route>,
    pub memory: RefCell<std::collections::HashMap<String, String>>,
}

/// Handed to every page builder. Cheap to clone.
#[derive(Clone)]
pub struct Ctx(pub(crate) Rc<CtxInner>);

impl Ctx {
    pub fn core(&self) -> &Arc<Core> {
        &self.0.core
    }

    pub fn window(&self) -> gtk::Window {
        self.0.window.clone().upcast()
    }

    pub fn route(&self) -> Route {
        *self.0.route.borrow()
    }

    pub fn navigate(&self, r: Route) {
        if let Some(f) = self.0.navigate.borrow().as_ref() {
            f(r);
        }
    }

    /// Rebuilds the current page now.
    pub fn rebuild(&self) {
        if let Some(f) = self.0.rebuild.borrow().as_ref() {
            f();
        }
    }

    /// Shows a snackbar.
    pub fn toast(&self, msg: &str) {
        if let Some(f) = self.0.toast.borrow().as_ref() {
            f(msg);
        }
    }

    /// Remembers a small piece of UI state (search text, selected filter) across page rebuilds and visits.
    /// Use keys prefixed by your page, e.g. "assets.search".
    pub fn remember(&self, key: &str, value: impl Into<String>) {
        self.0.memory.borrow_mut().insert(key.to_string(), value.into());
    }

    pub fn recall(&self, key: &str) -> Option<String> {
        self.0.memory.borrow().get(key).cloned()
    }

    /// Calls `f` on the GTK thread for every core event while `widget` exists.
    pub fn on_event(&self, widget: &impl IsA<gtk::Widget>, f: impl Fn(&podstack_core::Event) + 'static) {
        let mut rx = self.core().subscribe();
        let weak = widget.as_ref().downgrade();
        gtk::glib::spawn_future_local(async move {
            loop {
                let ev = match rx.recv().await {
                    Ok(e) => e,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                };
                let Some(w) = weak.upgrade() else { break };
                if w.root().is_none() {
                    break;
                }
                f(&ev);
            }
        });
    }
}
