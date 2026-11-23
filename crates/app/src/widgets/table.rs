//! Sortable Material data tables on top of `gtk::ColumnView`.
//!
//! ```ignore
//! Table::new(orders)
//!     .text("Item", |o| o.name.clone())
//!     .num("Price", |o| fmt::isk(o.price), |o| o.price)
//!     .widget("State", |o| widgets::status(&o.state, Status::Info).upcast())
//!     .sort_by(1, true)
//!     .on_activate(|o| println!("{}", o.name))
//!     .build()
//! ```

use gtk::{glib, prelude::*};
use std::cmp::Ordering;
use std::rc::Rc;

type CellFn<T> = Rc<dyn Fn(&T) -> gtk::Widget>;
type SortFn<T> = Rc<dyn Fn(&T, &T) -> Ordering>;

struct Col<T> {
    title: String,
    expand: bool,
    width: Option<i32>,
    cell: CellFn<T>,
    sort: Option<SortFn<T>>,
}

type RowAction<T> = Rc<dyn Fn(&T)>;
type RowTest<T> = Rc<dyn Fn(&T) -> bool>;

pub struct Table<T: 'static> {
    rows: Rc<Vec<T>>,
    cols: Vec<Col<T>>,
    on_activate: Option<RowAction<T>>,
    own_scroll: bool,
    min_height: Option<i32>,
    max_height: Option<i32>,
    initial_sort: Option<(usize, bool)>,
    empty: Option<String>,
    pinned: Option<RowTest<T>>,
}

fn cell_label(s: &str, numeric: bool) -> gtk::Widget {
    let l = gtk::Label::new(Some(s));
    l.set_xalign(if numeric { 1.0 } else { 0.0 });
    l.set_ellipsize(pango::EllipsizeMode::End);
    // Short values keep their full width, so a narrow window scrolls the table sideways instead of cutting names
    // and numbers to "…"; only long text (past 22 characters) is shortened.
    l.set_width_chars(s.chars().count().min(22) as i32);
    if numeric {
        l.add_css_class("numeric");
    }
    l.set_tooltip_text(if s.len() > 40 { Some(s) } else { None });
    l.upcast()
}

impl<T: 'static> Table<T> {
    pub fn new(rows: Vec<T>) -> Self {
        Table { rows: Rc::new(rows), cols: vec![], on_activate: None, own_scroll: true, min_height: None, max_height: None, initial_sort: None, empty: None, pinned: None }
    }

    /// Text column, sorted alphabetically.
    pub fn text(mut self, title: &str, f: impl Fn(&T) -> String + 'static) -> Self {
        let f = Rc::new(f);
        let f2 = f.clone();
        self.cols.push(Col {
            title: title.into(),
            expand: false,
            width: None,
            cell: Rc::new(move |r| cell_label(&f(r), false)),
            sort: Some(Rc::new(move |a, b| f2(a).to_lowercase().cmp(&f2(b).to_lowercase()))),
        });
        self
    }

    /// Right-aligned number column: `display` shows it, `key` sorts it.
    pub fn num(mut self, title: &str, display: impl Fn(&T) -> String + 'static, key: impl Fn(&T) -> f64 + 'static) -> Self {
        self.cols.push(Col {
            title: title.into(),
            expand: false,
            width: None,
            cell: Rc::new(move |r| cell_label(&display(r), true)),
            sort: Some(Rc::new(move |a, b| key(a).partial_cmp(&key(b)).unwrap_or(Ordering::Equal))),
        });
        self
    }

    /// Any widget (not sortable).
    pub fn widget(mut self, title: &str, f: impl Fn(&T) -> gtk::Widget + 'static) -> Self {
        self.cols.push(Col { title: title.into(), expand: false, width: None, cell: Rc::new(f), sort: None });
        self
    }

    /// Any widget, sorted by `key`.
    pub fn widget_sorted(mut self, title: &str, f: impl Fn(&T) -> gtk::Widget + 'static, cmp: impl Fn(&T, &T) -> Ordering + 'static) -> Self {
        self.cols.push(Col { title: title.into(), expand: false, width: None, cell: Rc::new(f), sort: Some(Rc::new(cmp)) });
        self
    }

    /// The last added column takes spare width.
    pub fn expand(mut self) -> Self {
        if let Some(c) = self.cols.last_mut() {
            c.expand = true;
        }
        self
    }

    /// Fixed width for the last added column.
    pub fn width(mut self, px: i32) -> Self {
        if let Some(c) = self.cols.last_mut() {
            c.width = Some(px);
        }
        self
    }

    pub fn sort_by(mut self, column: usize, descending: bool) -> Self {
        self.initial_sort = Some((column, descending));
        self
    }

    /// Called with the row on double-click / Enter.
    pub fn on_activate(mut self, f: impl Fn(&T) + 'static) -> Self {
        self.on_activate = Some(Rc::new(f));
        self
    }

    /// Without its own scrolling the table is as tall as its rows (for short tables inside a scrolled page).
    pub fn natural_height(mut self) -> Self {
        self.own_scroll = false;
        self
    }

    #[allow(dead_code)] // builder option kept for tables that shouldn't collapse when short
    pub fn min_height(mut self, px: i32) -> Self {
        self.min_height = Some(px);
        self
    }

    /// Scrolls itself once taller than `px`, and is only as tall as its rows below that (for a table sharing the
    /// page with another one).
    pub fn max_height(mut self, px: i32) -> Self {
        self.max_height = Some(px);
        self
    }

    /// Rows for which `f` is true stay at the bottom whichever way the table is sorted (a totals row).
    pub fn pinned_last(mut self, f: impl Fn(&T) -> bool + 'static) -> Self {
        self.pinned = Some(Rc::new(f));
        self
    }

    /// Shown instead of an empty table.
    pub fn empty_text(mut self, t: &str) -> Self {
        self.empty = Some(t.into());
        self
    }

    pub fn build(self) -> gtk::Widget {
        if self.rows.is_empty() {
            if let Some(e) = &self.empty {
                let l = crate::widgets::text(e, &["body-medium", "dim"]);
                l.set_margin_top(16);
                l.set_margin_bottom(16);
                l.set_margin_start(16);
                return l.upcast();
            }
        }
        let store = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        for i in 0..self.rows.len() {
            store.append(&glib::BoxedAnyObject::new(i));
        }
        let cv = gtk::ColumnView::new(None::<gtk::SelectionModel>);
        cv.add_css_class("m3-table");
        cv.set_reorderable(false);
        cv.set_show_row_separators(false);
        let idx = |o: &glib::Object| *o.downcast_ref::<glib::BoxedAnyObject>().unwrap().borrow::<usize>();

        let mut columns = vec![];
        for col in &self.cols {
            let factory = gtk::SignalListItemFactory::new();
            let rows = self.rows.clone();
            let cell = col.cell.clone();
            factory.connect_bind(move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().unwrap();
                if let Some(obj) = item.item() {
                    let w = cell(&rows[idx(&obj)]);
                    item.set_child(Some(&w));
                }
            });
            factory.connect_unbind(|_, item| {
                item.downcast_ref::<gtk::ListItem>().unwrap().set_child(None::<&gtk::Widget>);
            });
            let c = gtk::ColumnViewColumn::new(Some(&col.title), Some(factory));
            c.set_expand(col.expand);
            c.set_resizable(true);
            if let Some(w) = col.width {
                c.set_fixed_width(w);
            }
            if let Some(sort) = &col.sort {
                let rows = self.rows.clone();
                let sort = sort.clone();
                let pinned = self.pinned.clone();
                // The column view inverts a column's order for descending sorts, so a pinned row has to know the
                // direction to stay last. Weak: the column view owns the sorter that owns this closure.
                let view_sorter = cv.sorter().and_downcast::<gtk::ColumnViewSorter>().map(|s| s.downgrade());
                let sorter = gtk::CustomSorter::new(move |a, b| {
                    let (a, b) = (&rows[idx(a)], &rows[idx(b)]);
                    if let Some(p) = &pinned {
                        let (pa, pb) = (p(a), p(b));
                        if pa != pb {
                            let descending = view_sorter
                                .as_ref()
                                .and_then(|w| w.upgrade())
                                .is_some_and(|s| s.primary_sort_order() == gtk::SortType::Descending);
                            return if pa != descending { Ordering::Greater } else { Ordering::Less }.into();
                        }
                    }
                    sort(a, b).into()
                });
                c.set_sorter(Some(&sorter));
            }
            cv.append_column(&c);
            columns.push(c);
        }

        // Sort before the model is attached: sorting afterwards keeps the old first row in view, so the table
        // would open scrolled partway down.
        if let Some((i, desc)) = self.initial_sort {
            if let Some(c) = columns.get(i) {
                cv.sort_by_column(Some(c), if desc { gtk::SortType::Descending } else { gtk::SortType::Ascending });
            }
        }
        let sort_model = gtk::SortListModel::new(Some(store), cv.sorter());
        let selection = gtk::SingleSelection::new(Some(sort_model));
        selection.set_autoselect(false);
        selection.set_can_unselect(true);
        cv.set_model(Some(&selection));
        if let Some(f) = self.on_activate.clone() {
            let rows = self.rows.clone();
            cv.connect_activate(move |cv, pos| {
                if let Some(obj) = cv.model().and_then(|m| m.item(pos)) {
                    f(&rows[idx(&obj)]);
                }
            });
        }

        let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
        frame.add_css_class("table-frame");
        frame.set_overflow(gtk::Overflow::Hidden);
        if self.own_scroll {
            let sw = gtk::ScrolledWindow::new();
            sw.set_child(Some(&cv));
            sw.set_vexpand(true);
            sw.set_hexpand(true);
            sw.set_propagate_natural_height(self.min_height.is_none());
            if let Some(h) = self.min_height {
                sw.set_min_content_height(h);
            }
            if let Some(h) = self.max_height {
                sw.set_max_content_height(h);
                sw.set_vexpand(false);
                // The column view guesses its natural height from rows it hasn't laid out yet and comes up short
                // (a two-row table showed one), so ask for room for every row up to the maximum.
                if self.min_height.is_none() {
                    sw.set_min_content_height(h.min(48 + self.rows.len() as i32 * 62));
                }
            }
            frame.append(&sw);
            frame.set_vexpand(self.max_height.is_none());
        } else {
            let sw = gtk::ScrolledWindow::new();
            sw.set_vscrollbar_policy(gtk::PolicyType::Never);
            sw.set_child(Some(&cv));
            sw.set_propagate_natural_height(true);
            frame.append(&sw);
        }
        frame.upcast()
    }
}
