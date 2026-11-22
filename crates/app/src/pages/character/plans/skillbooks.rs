//! The strip at the top of a plan with the ISK cost of the skillbooks it needs that the character hasn't injected
//! yet, expandable to a per-book table: Jita (lowest sell in The Forge), the skill window (NPC price plus markup)
//! and EVE's average price.

use super::{plural, Price, P};
use crate::rt;
use crate::widgets::{self as w, Btn};
use gtk::prelude::*;
use podstack_core::features::plan_tools::profile;
use podstack_core::fmt;

fn isk(v: Option<f64>) -> String {
    v.map(fmt::isk).unwrap_or_else(|| "—".into())
}

fn short(v: f64) -> String {
    format!("{} ISK", fmt::isk_short(v))
}

/// Starts loading prices for books not loaded yet.
fn load(p: &P, ids: &[i64]) {
    let missing: Vec<i64> = {
        let st = p.st.borrow();
        ids.iter().copied().filter(|id| !st.prices.contains_key(id) && !st.prices_pending.contains(id)).collect()
    };
    if missing.is_empty() {
        return;
    }
    p.st.borrow_mut().prices_pending.extend(missing.iter().copied());
    let core = p.core().clone();
    let pp = p.clone();
    let ids2 = missing.clone();
    rt::run(
        async move {
            let forge = core.market.forge_sell(&core.esi, &ids2, false).await;
            let avg = core.market.average_prices(&core.esi).await;
            (forge, avg)
        },
        move |(forge, avg)| {
            let mut failed = false;
            {
                let mut st = pp.st.borrow_mut();
                for id in &missing {
                    st.prices_pending.remove(id);
                    if !forge.contains_key(id) {
                        failed = true;
                    }
                    st.prices.insert(*id, Price { forge: forge.get(id).copied().flatten(), average: avg.get(id).copied() });
                }
                st.prices_error = failed;
            }
            if pp.right.root().is_some() {
                pp.render_editor();
            }
        },
    );
}

pub(super) fn strip(p: &P) -> gtk::Widget {
    let out = w::vbox(0);
    let Some(c) = p.character() else { return out.upcast() };
    let books = {
        let st = p.st.borrow();
        if !st.planner.queue.iter().any(|i| i.is_skill()) {
            return out.upcast();
        }
        profile::missing_skillbooks(&c, &st.planner.queue)
    };
    if books.is_empty() {
        let s = w::hbox(12);
        s.add_css_class("skillbook-strip");
        s.add_css_class("done");
        s.append(&w::icon_sized("menu_book", 20));
        s.append(&w::text("Skillbooks: every skill in this plan is already injected.", &["body-medium", "dim"]));
        out.append(&s);
        return out.upcast();
    }
    let ids: Vec<i64> = books.iter().map(|b| b.0).collect();
    load(p, &ids);
    let st = p.st.borrow();
    let loading = ids.iter().any(|id| st.prices_pending.contains(id));
    let price = |id: &i64| st.prices.get(id).copied().unwrap_or_default();
    let jita: f64 = ids.iter().filter_map(|id| price(id).forge).sum();
    let avg: f64 = ids.iter().filter_map(|id| price(id).average).sum();
    let window: f64 = ids.iter().filter_map(|id| profile::skill_window_price(*id)).sum();
    let unpriced = ids.iter().filter(|id| price(id).forge.is_none()).count();

    let s = w::hbox(12);
    s.add_css_class("skillbook-strip");
    s.append(&w::icon_sized("menu_book", 20));
    let mut m = format!("<b>{} skillbook{} to buy</b>", books.len(), plural(books.len()));
    if loading {
        m.push_str(" · loading prices…");
    } else if st.prices_error {
        m.push_str(&format!(" · Skill window <b>{}</b> · <span>couldn't load market prices</span>", short(window)));
    } else {
        m.push_str(&format!(" · Jita <b>{}</b> · Skill window <b>{}</b> · EVE avg {}", short(jita), short(window), short(avg)));
        if unpriced > 0 {
            m.push_str(&format!(" · {unpriced} not on market"));
        }
    }
    let l = w::markup(&m, &["body-medium"]);
    l.set_hexpand(true);
    if unpriced > 0 && !loading {
        l.set_tooltip_text(Some("No sell orders in The Forge right now; left out of the Jita total"));
    }
    s.append(&l);
    let expanded = st.books_expanded;
    let toggle = w::button_icon(if expanded { "expand_less" } else { "expand_more" }, if expanded { "Hide books" } else { "Show books" }, Btn::Text);
    {
        let pp = p.clone();
        toggle.connect_clicked(move |_| {
            let v = !pp.st.borrow().books_expanded;
            pp.st.borrow_mut().books_expanded = v;
            pp.render_editor();
        });
    }
    s.append(&toggle);
    out.append(&s);

    if expanded {
        let grid = gtk::Grid::new();
        grid.set_column_spacing(24);
        grid.set_row_spacing(6);
        grid.set_margin_start(24);
        grid.set_margin_end(24);
        grid.set_margin_bottom(8);
        for (col, h) in ["Skillbook", "Jita lowest sell", "Skill window", "EVE average"].iter().enumerate() {
            let l = w::line(h, &["label-medium", "dim"]);
            if col > 0 {
                l.set_xalign(1.0);
            }
            grid.attach(&l, col as i32, 0, 1, 1);
        }
        for (r, (id, name)) in books.iter().enumerate() {
            let row = r as i32 + 1;
            let pr = price(id);
            let direct = profile::skill_window_price(*id);
            // highlight the cheaper way to get the book
            let jita_cheaper = !loading && pr.forge.map(|j| direct.map(|d| j <= d).unwrap_or(true)).unwrap_or(false);
            let direct_cheaper = !loading && direct.is_some() && !jita_cheaper;
            let n = w::line(name, &["body-medium"]);
            n.set_hexpand(true);
            grid.attach(&n, 0, row, 1, 1);
            let cells = [
                (if loading { "…".into() } else { isk(pr.forge) }, jita_cheaper, false),
                (isk(direct), direct_cheaper, false),
                (if loading { "…".into() } else { isk(pr.average) }, false, true),
            ];
            for (k, (t, best, dim)) in cells.into_iter().enumerate() {
                let mut cls = vec!["numeric", "body-medium"];
                if best {
                    cls.push("skillbook-best");
                }
                if dim {
                    cls.push("dim");
                }
                let l = w::line(&t, &cls);
                l.set_xalign(1.0);
                grid.attach(&l, k as i32 + 1, row, 1, 1);
            }
        }
        out.append(&grid);
        let note = w::text(
            &format!(
                "Skills already injected aren't counted. Jita is the lowest sell order in The Forge (refreshed every 6 hours); \
                 Skill window is buying and injecting straight from the in-game skill window, which costs the NPC price plus {}%. \
                 The cheaper of the two is highlighted.",
                profile::markup_percent()
            ),
            &["body-small", "dim"],
        );
        note.set_margin_start(24);
        note.set_margin_end(24);
        note.set_margin_bottom(12);
        out.append(&note);
    }
    out.upcast()
}
