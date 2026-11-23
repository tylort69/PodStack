//! Buyback → Appraise: pasted items against a program, what it pays per line, and the contract to make.

use super::{line_status, load_prices, loading, pct, program_dropdown, selected, CSS};
use crate::ctx::Ctx;
use crate::pages::market_orders::{set_child, type_cell};
use crate::widgets::{self as w, Btn, Table};
use gtk::prelude::*;
use podstack_core::features::buyback::{self as bb, Item, Offer, OfferLine, Program};
use podstack_core::features::trade;
use podstack_core::fmt;
use std::cell::RefCell;
use std::rc::Rc;

const TEXT_KEY: &str = "contracts.buyback.appraise.text";

pub(super) fn view(ctx: &Ctx) -> gtk::Box {
    crate::theme::add_css(CSS);
    let col = w::vbox(12);
    let doc = bb::load_doc();
    let Some(first) = selected(ctx, &doc) else {
        col.append(&super::no_programs(ctx));
        return col;
    };
    let program = Rc::new(RefCell::new(first));

    let view = gtk::TextView::new();
    view.add_css_class("bb-paste");
    view.set_wrap_mode(gtk::WrapMode::None);
    view.buffer().set_text(&ctx.recall(TEXT_KEY).unwrap_or_default());
    let scroll = w::scrolled(&view);
    scroll.set_min_content_height(140);
    scroll.set_max_content_height(140);
    scroll.set_hexpand(true);
    scroll.set_vexpand(false);

    let bar = w::hbox(8);
    let go = w::button_icon("calculate", "Appraise", Btn::Filled);
    let clear = w::button("Clear", Btn::Text);
    bar.append(&go);
    bar.append(&clear);
    let with = w::line("Program", &["body-medium", "dim"]);
    with.set_margin_start(8);
    bar.append(&with);
    let terms = w::line(&program.borrow().summary(), &["body-small", "dim"]);
    terms.set_hexpand(true);
    terms.set_xalign(1.0);

    let results = w::vbox(12);
    results.append(&w::empty_state(
        "receipt_long",
        "Nothing appraised yet",
        "Paste items (a hangar or contract window, a cargo scan, an EFT fit or a list like \"5 Hobgoblin II\") and press Appraise to see what the program pays. Selling to a buyback? This is what its terms say you should get.",
    ));

    let run: Rc<dyn Fn()> = {
        let (ctx, view, results, program) = (ctx.clone(), view.clone(), results.clone(), program.clone());
        Rc::new(move || {
            let (s, e) = view.buffer().bounds();
            let text = view.buffer().text(&s, &e, false);
            // nothing pasted yet (e.g. a program picked first): keep the hint
            if text.trim().is_empty() {
                return;
            }
            let parsed = trade::parse_items(text.as_str());
            if parsed.items.is_empty() {
                set_child(&results, &w::empty_state("help", "No items found", "None of the lines named an item sold on the market."));
                return;
            }
            let p = program.borrow().clone();
            let items: Vec<Item> = parsed.items.iter().map(|&(t, q)| Item::new(t, q)).collect();
            set_child(&results, &loading(&format!("Pricing {} items at {}…", items.len(), p.hub().0)));
            let (ctx2, results, unknown) = (ctx.clone(), results.clone(), parsed.unknown.clone());
            load_prices(&ctx, &p.clone(), bb::types_of(&items), move |prices| {
                let missing = items.iter().filter(|i| !prices.contains_key(&i.type_id)).count();
                let offer = bb::appraise(&p, &items, &prices);
                set_child(&results, &offer_view(&ctx2, &p, offer, &unknown, missing));
            });
        })
    };
    {
        let (run, terms, program) = (run.clone(), terms.clone(), program.clone());
        let dd = program_dropdown(ctx, &doc, move |p| {
            terms.set_text(&p.summary());
            *program.borrow_mut() = p;
            run();
        });
        bar.append(&dd);
    }
    bar.append(&terms);
    col.append(&scroll);
    col.append(&bar);
    col.append(&results);

    {
        let ctx = ctx.clone();
        view.buffer().connect_changed(move |b| {
            let (s, e) = b.bounds();
            ctx.remember(TEXT_KEY, b.text(&s, &e, false).as_str());
        });
    }
    {
        let view = view.clone();
        clear.connect_clicked(move |_| view.buffer().set_text(""));
    }
    {
        let run = run.clone();
        go.connect_clicked(move |_| run());
    }
    if !ctx.recall(TEXT_KEY).unwrap_or_default().trim().is_empty() {
        run();
    }
    col
}

/// A copy button for one value.
fn copy_button(ctx: &Ctx, label: &str, value: String) -> gtk::Button {
    let b = w::button_icon("content_copy", label, Btn::Tonal);
    let ctx = ctx.clone();
    b.connect_clicked(move |_| w::copy_to_clipboard(&ctx, &value));
    b
}

fn offer_view(ctx: &Ctx, p: &Program, o: Offer, unknown: &[String], missing: usize) -> gtk::Box {
    let v = w::vbox(12);
    let (hub, _, _) = p.hub();
    let offer_tile = w::stat_tile(
        "Offer",
        &format!("{} ISK", fmt::isk_short(o.total)),
        Some(&match o.effective_rate() {
            Some(r) => format!("{} of {hub} {} value", pct(r), p.basis.short()),
            None => "Nothing in it is bought".into(),
        }),
    );
    offer_tile.add_css_class("accent");
    let accepted = o.lines.iter().filter(|l| l.status.accepted()).count();
    v.append(&w::tile_row(vec![
        offer_tile,
        w::stat_tile("Hub buy value", &format!("{} ISK", fmt::isk_short(o.hub_buy)), Some(&format!("Everything at {hub} buy orders"))),
        w::stat_tile("Hub sell value", &format!("{} ISK", fmt::isk_short(o.hub_sell)), Some(&format!("Everything at {hub} sell orders"))),
        w::stat_tile(
            "Volume",
            &format!("{} m³", fmt::decimal(o.volume, 1)),
            Some(&if o.haul > 0.0 { format!("Hauling −{} ISK", fmt::isk_short(o.haul)) } else { "Packaged".into() }),
        ),
        w::stat_tile("Lines", &format!("{accepted} / {}", o.lines.len()), Some(&format!("{} items accepted", fmt::int(o.lines.iter().filter(|l| l.status.accepted()).map(|l| l.quantity).sum())))),
    ]));

    if !unknown.is_empty() {
        let shown: Vec<&str> = unknown.iter().take(5).map(|s| s.as_str()).collect();
        let more = if unknown.len() > 5 { format!(" and {} more", unknown.len() - 5) } else { String::new() };
        v.append(&w::banner("help", &format!("Not recognised (left out): {}{more}", shown.join(", ")), w::BannerKind::Notice));
    }
    let refused: Vec<&str> = o.lines.iter().filter(|l| !l.status.accepted()).map(|l| l.name.as_str()).collect();
    if !refused.is_empty() {
        let more = if refused.len() > 6 { format!(" and {} more", refused.len() - 6) } else { String::new() };
        v.append(&w::banner(
            "block",
            &format!("Not bought: {}{more}. Leave these out of the contract.", refused.iter().take(6).copied().collect::<Vec<_>>().join(", ")),
            w::BannerKind::Error,
        ));
    }
    if missing > 0 {
        v.append(&w::banner("cloud_off", &format!("{missing} items couldn't be priced: the market didn't answer. Try again in a moment."), w::BannerKind::Notice));
    }

    // the contract to make
    let howto = w::vbox(12);
    let text = bb::instructions(p, &o);
    let instr = w::text(&text, &["bb-instructions", "body-medium"]);
    instr.set_selectable(true);
    howto.append(&instr);
    let buttons = w::hbox(8);
    buttons.append(&copy_button(ctx, "Price", bb::price_field(o.total)));
    let receiver = p.receiver_label();
    if !receiver.is_empty() {
        buttons.append(&copy_button(ctx, "Receiver", receiver));
    }
    buttons.append(&copy_button(ctx, "Description", p.name.trim().to_string()));
    let list: Vec<String> = o.lines.iter().filter(|l| l.status.accepted()).map(|l| format!("{}\t{}", l.name, l.quantity)).collect();
    buttons.append(&copy_button(ctx, "Accepted items", list.join("\n")));
    buttons.append(&copy_button(ctx, "Everything", text.clone()));
    let flow = gtk::FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_column_spacing(8);
    flow.set_row_spacing(8);
    flow.set_max_children_per_line(6);
    while let Some(c) = buttons.first_child() {
        buttons.remove(&c);
        flow.insert(&c, -1);
    }
    howto.append(&flow);
    v.append(&w::card_with(
        "Contract",
        Some(&format!("Make this item exchange to sell to \"{}\". A buyback paying less than this isn't keeping to these terms.", p.name.trim())),
        &[],
        &howto,
    ));

    let price = |x: Option<f64>| x.map(|x| fmt::decimal(x, 2)).unwrap_or_else(|| "—".into());
    let basis = p.basis.short();
    let table = Table::new(o.lines)
        .widget_sorted("Item", |l: &OfferLine| type_cell(l.type_id, &l.name), |a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        .expand()
        .width(240)
        .widget_sorted(
            "Status",
            |l| w::status(l.status.label(), line_status(l.status)).upcast(),
            |a, b| a.status.accepted().cmp(&b.status.accepted()),
        )
        .num("Quantity", |l| fmt::int(l.quantity), |l| l.quantity as f64)
        .num("Volume", |l| format!("{} m³", fmt::decimal(l.volume, 2)), |l| l.volume)
        .num(&format!("Unit ({basis})"), move |l| price(l.unit), |l| l.unit.unwrap_or(0.0))
        .num("Value", |l| fmt::isk_short(l.value), |l| l.value)
        .num("Rate", |l| if l.status.accepted() { format!("{}{}", pct(l.rate), if l.by_rule { " *" } else { "" }) } else { "—".into() }, |l| l.rate)
        .num("Hauling", |l| if l.haul > 0.0 { format!("−{}", fmt::isk_short(l.haul)) } else { String::new() }, |l| l.haul)
        .num("Offer", |l| fmt::isk_short(l.offer), |l| l.offer)
        .sort_by(8, true)
        .natural_height()
        .build();
    v.append(&table);
    let foot = w::line("* set by a rule. Volumes are packaged.", &["body-small", "dim"]);
    v.append(&foot);
    v
}
