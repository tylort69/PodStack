//! Assets with assembled ships as containers: a ship is one row, and what ESI says is inside it (fitted modules,
//! rigs, subsystems, loaded charges, drones, fighters, cargo and every other hold) hangs under it, as does whatever
//! sits in a container in its cargo. A ship's value is the hull plus everything inside it.
//!
//! Only ships nest: a station container standing in a hangar stays a row of its own with its contents beside it, as
//! in the flat list. Each item is counted once, so the rolled-up values of the top rows add up to the plain total.

use super::industry::{asset_matches, normalize_query};
use crate::domains::assets::{Asset, AssetsData};
use crate::sde::SHIPS;
use std::collections::HashMap;

/// How many levels an item may sit below its ship (guards against a loop in bad data).
const MAX_DEPTH: usize = 10;

/// Every asset with its children; node `i` is `data.items[i]`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AssetTree {
    /// Items nested under each item, in display order (slot, then value).
    pub children: Vec<Vec<usize>>,
    /// Each item's own value plus everything nested under it.
    pub value: Vec<f64>,
    /// Items that don't nest under a ship, most valuable first.
    pub roots: Vec<usize>,
}

impl AssetTree {
    /// How many items are nested under `i`, at any depth.
    pub fn contents(&self, i: usize) -> usize {
        self.children[i].iter().map(|&c| 1 + self.contents(c)).sum()
    }
}

/// An assembled ship: only those hold fittings and cargo (a packaged one is a stack of hulls).
fn assembled_ship(a: &Asset, is_ship: &impl Fn(i64) -> bool) -> bool {
    a.is_singleton && !a.is_blueprint_copy && is_ship(a.type_id)
}

/// Where a slot shows among a ship's contents: fittings in slot order, then drones and fighters, then holds.
fn slot_rank(flag: &str) -> u8 {
    let base = flag.trim_end_matches(|c: char| c.is_ascii_digit());
    match base {
        "SubSystemSlot" => 0,
        "HiSlot" => 1,
        "MedSlot" => 2,
        "LoSlot" => 3,
        "RigSlot" => 4,
        "DroneBay" => 5,
        "FighterTube" => 6,
        "FighterBay" => 7,
        "Cargo" => 8,
        _ => 9,
    }
}

/// The tree with ships recognised from the static data.
pub fn ship_tree(data: &AssetsData) -> AssetTree {
    build(data, |t| SHIPS.by_id.contains_key(&t))
}

/// Nests what is inside each assembled ship under it; `is_ship` says which types are ships.
pub fn build(data: &AssetsData, is_ship: impl Fn(i64) -> bool) -> AssetTree {
    let n = data.items.len();
    let index: HashMap<i64, usize> = data.items.iter().enumerate().map(|(i, a)| (a.item_id, i)).collect();
    let parent: Vec<Option<usize>> = data.items.iter().map(|a| a.parent_id.and_then(|p| index.get(&p).copied())).collect();

    // An item nests when some item above it is an assembled ship. One caught in a loop of bad data never nests,
    // so it still shows (as a top row).
    let nests = |i: usize| {
        let (mut cur, mut in_ship) = (i, false);
        for _ in 0..MAX_DEPTH {
            match parent[cur] {
                Some(p) if p == i => return false,
                Some(p) => {
                    in_ship |= assembled_ship(&data.items[p], &is_ship);
                    cur = p;
                }
                None => break,
            }
        }
        in_ship
    };
    let mut children = vec![Vec::new(); n];
    let mut roots = Vec::new();
    for (i, p) in parent.iter().enumerate() {
        match p.filter(|_| nests(i)) {
            Some(p) => children[p].push(i),
            None => roots.push(i),
        }
    }

    // Values roll up from the leaves; the depth guard covers chains longer than `nests` looks.
    let mut value = vec![f64::NAN; n];
    fn roll(i: usize, data: &AssetsData, children: &[Vec<usize>], value: &mut [f64], depth: usize) -> f64 {
        if !value[i].is_nan() {
            return value[i];
        }
        let mut v = data.items[i].value();
        if depth < MAX_DEPTH * 2 {
            for &c in &children[i] {
                v += roll(c, data, children, value, depth + 1);
            }
        }
        value[i] = v;
        v
    }
    for &r in &roots {
        roll(r, data, &children, &mut value, 0);
    }
    for v in &mut value {
        // only unreachable items (bad data) are left; count them as nothing rather than NaN
        if v.is_nan() {
            *v = 0.0;
        }
    }

    let label = |i: usize| data.items[i].label().to_lowercase();
    for list in &mut children {
        list.sort_by(|&a, &b| {
            slot_rank(&data.items[a].flag)
                .cmp(&slot_rank(&data.items[b].flag))
                .then(value[b].total_cmp(&value[a]))
                .then_with(|| label(a).cmp(&label(b)))
        });
    }
    roots.sort_by(|&a, &b| value[b].total_cmp(&value[a]).then_with(|| label(a).cmp(&label(b))));
    AssetTree { children, value, roots }
}

/// One location's part of the tree that a search keeps, ready for a tree table: `items` are the rows (indices into
/// [`AssetsData::items`]); `roots` and `children` point into `items`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreeGroup {
    pub location_id: i64,
    pub name: String,
    pub items: Vec<usize>,
    pub roots: Vec<usize>,
    pub children: Vec<Vec<usize>>,
    /// How many of the rows match the search (themselves or by being inside a match); the rest are the ships and
    /// containers around a match, shown so it has somewhere to sit.
    pub matches: usize,
    /// The matching rows' own values, each counted once (all of the location's assets when nothing is searched).
    pub value: f64,
    /// Top rows with a match that the limit left out (they still count in `matches` and `value`).
    pub more: usize,
}

/// The tree split by root location and cut down to `query`: a match keeps everything inside it and the ships it's
/// in. Locations come most valuable first; each keeps at most `root_limit` top rows.
pub fn group_tree(data: &AssetsData, tree: &AssetTree, query: &str, root_limit: usize) -> Vec<TreeGroup> {
    let query = normalize_query(query);
    let by_id = data.by_id();
    let names: HashMap<i64, String> = data.items.iter().map(|a| (a.root_location_id, data.location_name(a.root_location_id))).collect();
    let matched: Vec<bool> = data
        .items
        .iter()
        .map(|a| asset_matches(&query, a, a.parent_id.and_then(|p| by_id.get(&p).copied()), &names[&a.root_location_id]))
        .collect();

    struct Walk<'a> {
        data: &'a AssetsData,
        tree: &'a AssetTree,
        matched: &'a [bool],
        g: TreeGroup,
    }
    impl Walk<'_> {
        /// Adds `i` and what it keeps under it; returns its row, or None when nothing in it is kept.
        fn visit(&mut self, i: usize, inside_match: bool, depth: usize) -> Option<usize> {
            let m = inside_match || self.matched[i];
            let row = self.g.items.len();
            self.g.items.push(i);
            self.g.children.push(Vec::new());
            let mut kept = Vec::new();
            if depth < MAX_DEPTH * 2 {
                for &c in &self.tree.children[i] {
                    if let Some(r) = self.visit(c, m, depth + 1) {
                        kept.push(r);
                    }
                }
            }
            if !m && kept.is_empty() {
                self.g.items.pop();
                self.g.children.pop();
                return None;
            }
            if m {
                self.g.matches += 1;
                self.g.value += self.data.items[i].value();
            }
            self.g.children[row] = kept;
            Some(row)
        }
    }

    let mut groups: HashMap<i64, Walk> = HashMap::new();
    for &r in &tree.roots {
        let loc = data.items[r].root_location_id;
        let w = groups.entry(loc).or_insert_with(|| Walk {
            data,
            tree,
            matched: &matched,
            g: TreeGroup { location_id: loc, name: names[&loc].clone(), ..Default::default() },
        });
        if w.g.roots.len() >= root_limit {
            // past the limit a root still counts toward the location's matches and value, just isn't shown
            let mut extra = Walk { data, tree, matched: &matched, g: TreeGroup::default() };
            if extra.visit(r, false, 0).is_some() {
                w.g.matches += extra.g.matches;
                w.g.value += extra.g.value;
                w.g.more += 1;
            }
            continue;
        }
        if let Some(row) = w.visit(r, false, 0) {
            w.g.roots.push(row);
        }
    }
    let mut out: Vec<TreeGroup> = groups.into_values().map(|w| w.g).filter(|g| g.matches > 0).collect();
    out.sort_by(|a, b| b.value.total_cmp(&a.value).then_with(|| a.name.cmp(&b.name)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RIFTER: i64 = 587;
    const ORCA: i64 = 28606;

    fn item(item_id: i64, type_id: i64, name: &str, quantity: i64, flag: &str, parent: Option<i64>, price: f64) -> Asset {
        Asset {
            item_id,
            type_id,
            name: name.into(),
            quantity,
            flag: flag.into(),
            parent_id: parent,
            root_location_id: 60003760,
            is_singleton: quantity == 1,
            unit_price: Some(price),
            ..Default::default()
        }
    }

    /// A fitted Rifter with ammo, drones and a container of minerals in its cargo; an Orca carrying the Rifter's
    /// twin in its ship hangar; a station container with ore; and loose minerals.
    fn sample() -> AssetsData {
        let items = vec![
            item(1, RIFTER, "Rifter", 1, "Hangar", None, 500.0),
            item(2, 100, "200mm Autocannon", 1, "HiSlot0", Some(1), 50.0),
            item(3, 101, "EMP S", 200, "HiSlot0", Some(1), 0.1),
            item(4, 102, "Small Armor Repairer", 1, "LoSlot0", Some(1), 30.0),
            item(5, 103, "Small Trimark", 1, "RigSlot0", Some(1), 70.0),
            item(6, 104, "Hobgoblin", 2, "DroneBay", Some(1), 10.0),
            item(7, 105, "Small Container", 1, "Cargo", Some(1), 5.0),
            item(8, 34, "Tritanium", 1000, "Unlocked", Some(7), 0.5),
            item(9, 34, "Tritanium", 400, "Hangar", None, 0.5),
            item(10, 106, "Station Container", 1, "Hangar", None, 20.0),
            item(11, 107, "Veldspar", 100, "Unlocked", Some(10), 2.0),
            item(12, ORCA, "Orca", 1, "Hangar", None, 1000.0),
            item(13, RIFTER, "Rifter", 1, "ShipHangar", Some(12), 500.0),
            item(14, 100, "200mm Autocannon", 1, "HiSlot0", Some(13), 50.0),
        ];
        AssetsData { items, ..Default::default() }
    }

    fn tree(d: &AssetsData) -> AssetTree {
        build(d, |t| t == RIFTER || t == ORCA)
    }

    fn ids(d: &AssetsData, list: &[usize]) -> Vec<i64> {
        list.iter().map(|&i| d.items[i].item_id).collect()
    }

    #[test]
    fn fittings_and_holds_nest_under_the_ship() {
        let d = sample();
        let t = tree(&d);
        // slot order: high slots (module before the cheaper charge), low, rig, drones, cargo
        assert_eq!(ids(&d, &t.children[0]), vec![2, 3, 4, 5, 6, 7]);
        // the Rifter: hull 500 + 50 + 20 + 30 + 70 + 20 + container 5 + its 500 of minerals
        assert_eq!(t.value[0], 1195.0);
        assert_eq!(t.contents(0), 7);
    }

    #[test]
    fn containers_nest_inside_ships_but_not_in_hangars() {
        let d = sample();
        let t = tree(&d);
        assert_eq!(ids(&d, &t.children[6]), vec![8]);
        assert_eq!(t.value[6], 505.0);
        // the station container in the hangar stays flat, its ore beside it
        assert!(t.children[9].is_empty());
        assert!(t.roots.contains(&9) && t.roots.contains(&10));
        // a ship in a ship hangar nests, with its own fittings
        assert_eq!(ids(&d, &t.children[11]), vec![13]);
        assert_eq!(ids(&d, &t.children[12]), vec![14]);
        assert_eq!(t.value[11], 1550.0);
    }

    #[test]
    fn packaged_ships_hold_nothing() {
        let mut d = sample();
        d.items[0].is_singleton = false;
        let t = tree(&d);
        assert!(t.children[0].is_empty());
        assert!(t.roots.contains(&1));
    }

    #[test]
    fn totals_count_each_item_once() {
        let d = sample();
        let t = tree(&d);
        let rolled: f64 = t.roots.iter().map(|&r| t.value[r]).sum();
        assert_eq!(rolled, d.total_value());
        let groups = group_tree(&d, &t, "", usize::MAX);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].value, d.total_value());
        assert_eq!(groups[0].matches, d.items.len());
        assert_eq!(groups[0].items.len(), d.items.len());
        // roots come most valuable first: Orca (with its Rifter) before the Rifter
        assert_eq!(ids(&d, &groups[0].roots.iter().map(|&r| groups[0].items[r]).collect::<Vec<_>>())[..2], [12, 1]);
    }

    #[test]
    fn search_keeps_the_ships_around_a_match() {
        let d = sample();
        let t = tree(&d);
        let g = &group_tree(&d, &t, "trimark", usize::MAX)[0];
        // the rig, under the Rifter it's fitted to; only the rig counts
        assert_eq!(ids(&d, &g.items), vec![1, 5]);
        assert_eq!(g.roots, vec![0]);
        assert_eq!(g.children[0], vec![1]);
        assert_eq!(g.matches, 1);
        assert_eq!(g.value, 70.0);

        // matching a ship keeps everything inside it, nested containers too
        let g = &group_tree(&d, &t, "orca", usize::MAX)[0];
        assert_eq!(ids(&d, &g.items), vec![12, 13, 14]);
        assert_eq!(g.value, 1550.0);

        // the minerals in the Rifter's container, and the loose ones
        let g = &group_tree(&d, &t, "tritanium", usize::MAX)[0];
        assert_eq!(g.matches, 2);
        assert_eq!(g.value, 700.0);
        assert!(group_tree(&d, &t, "nothing like this", usize::MAX).is_empty());
    }

    #[test]
    fn root_limit_keeps_the_totals() {
        let d = sample();
        let t = tree(&d);
        let g = &group_tree(&d, &t, "", 1)[0];
        assert_eq!(g.roots.len(), 1);
        assert_eq!(g.more, 4);
        assert_eq!(g.value, d.total_value());
        assert_eq!(g.matches, d.items.len());
    }

    #[test]
    fn loops_terminate() {
        let items = vec![item(1, RIFTER, "Rifter", 1, "Cargo", Some(2), 1.0), item(2, RIFTER, "Rifter", 1, "Cargo", Some(1), 1.0)];
        let d = AssetsData { items, ..Default::default() };
        let t = tree(&d);
        // both still show, each once
        assert_eq!(t.roots.len(), 2);
        assert_eq!(t.value, vec![1.0, 1.0]);
    }
}
