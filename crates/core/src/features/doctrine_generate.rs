//! Doctrines made from in-game fittings, on request: a character's saved fittings (ESI), or an EVE fittings export,
//! grouped into doctrines by their names. Alliances name a doctrine's fits alike ("ABC Muninn - DPS", "ABC Muninn -
//! Logi"), so the shared part of the names is the doctrine. ESI has no corporation or alliance fittings, only a
//! character's own; alliance fits come in once a character saves them to its personal fittings, or through the
//! fitting window's export.
//!
//! What's generated never replaces anything: it goes into a new group inside the top-level "Auto-generated" group
//! (made the first time), for the user to keep, move or delete.

use crate::features::doctrines::{self as dc, Doctrine, DoctrineDoc, DoctrineFit, Group};
use crate::features::fitting::{FitItem, ItemRef, ParsedFit};
use crate::Core;
use serde::Deserialize;

pub const FITTINGS_SCOPE: &str = "esi-fittings.read_fittings.v1";
/// The top-level group generated doctrines go into.
pub const AUTO_GROUP: &str = "Auto-generated";

#[derive(Deserialize)]
struct EsiFitting {
    name: String,
    ship_type_id: i64,
    #[serde(default)]
    items: Vec<EsiItem>,
}

#[derive(Deserialize)]
struct EsiItem {
    type_id: i64,
    #[serde(default = "one")]
    quantity: u32,
}

fn one() -> u32 {
    1
}

/// A character's saved (personal) fittings, as parsed fits.
pub async fn character_fittings(core: &Core, character_id: i64) -> Result<Vec<ParsedFit>, String> {
    let auth = core.auth.auth(&core.esi.http, character_id, 120).await.map_err(|e| e.to_string())?;
    if !auth.has_scope(FITTINGS_SCOPE) {
        return Err("This character's login doesn't allow reading fittings: log it in again and allow Read Fittings.".into());
    }
    let fits: Vec<EsiFitting> = core
        .esi
        .get(format!("characters/{character_id}/fittings/"))
        .auth(&auth)
        .scope(FITTINGS_SCOPE)
        .json()
        .await
        .map_err(|e| e.to_string())?;
    Ok(fits.into_iter().map(parsed).collect())
}

fn parsed(f: EsiFitting) -> ParsedFit {
    let mut items: Vec<FitItem> = Vec::new();
    for i in f.items {
        match items.iter_mut().find(|x| x.item == ItemRef::TypeId(i.type_id)) {
            Some(x) => x.quantity += i.quantity,
            None => items.push(FitItem { item: ItemRef::TypeId(i.type_id), quantity: i.quantity.max(1) }),
        }
    }
    ParsedFit { format: "ESI", fit_name: Some(f.name.trim().to_string()), ship: ItemRef::TypeId(f.ship_type_id), items }
}

/// Where a fit's name splits into doctrine and role ("Muninn - DPS").
const SEPARATORS: [&str; 6] = [" - ", " \u{2013} ", " \u{2014} ", " | ", ": ", " / "];

fn words(s: &str) -> Vec<&str> {
    s.split_whitespace().collect()
}

/// The doctrine each fit name belongs to, as `(doctrine name, indexes into names)` in the order doctrines first
/// appear. A word prefix every name shares (the alliance's tag) is dropped; then a name with a separator belongs to
/// the part before it, and one without to the longest run of leading words it shares with a different such name (or
/// to its whole name when it shares none). Alliances reuse a name across hulls, so names equal to this one don't
/// count. Last, a doctrine whose name only extends another's ("x.Mainline Logi" and "x.Mainline") joins that one.
pub fn group_names(names: &[String]) -> Vec<(String, Vec<usize>)> {
    let all: Vec<Vec<&str>> = names.iter().map(|n| words(n)).collect();
    // the shared tag: leading words (case-insensitive) every name has, with something left after it in each
    let mut tag = 0;
    if all.len() > 1 {
        while let Some(w) = all[0].get(tag) {
            if !all.iter().all(|n| n.len() > tag + 1 && n[tag].eq_ignore_ascii_case(w)) {
                break;
            }
            tag += 1;
        }
    }
    let rest: Vec<String> = all.iter().map(|w| w[tag.min(w.len())..].join(" ")).collect();

    let split: Vec<Option<String>> = rest
        .iter()
        .map(|r| SEPARATORS.iter().filter_map(|s| r.find(s)).min().map(|i| r[..i].trim().to_string()).filter(|d| !d.is_empty()))
        .collect();
    let plain: Vec<usize> = (0..rest.len()).filter(|&i| split[i].is_none()).collect();
    let key = |i: usize| -> String {
        if let Some(d) = &split[i] {
            return d.clone();
        }
        let mine = words(&rest[i]);
        let shared = plain
            .iter()
            .filter(|&&j| !rest[j].eq_ignore_ascii_case(&rest[i]))
            .map(|&j| words(&rest[j]).iter().zip(&mine).take_while(|(a, b)| a.eq_ignore_ascii_case(b)).count())
            .max()
            .unwrap_or(0);
        if shared == 0 {
            rest[i].clone()
        } else {
            mine[..shared].join(" ")
        }
    };

    let keys: Vec<String> = names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let k = key(i);
            if k.is_empty() {
                name.trim().to_string()
            } else {
                k
            }
        })
        .collect();
    // the shortest other doctrine name this one extends by whole words, if any
    let base = |k: &str| -> String {
        let kw = words(k);
        keys.iter()
            .map(|o| words(o))
            .filter(|ow| ow.len() < kw.len() && ow.iter().zip(&kw).all(|(a, b)| a.eq_ignore_ascii_case(b)))
            .min_by_key(|ow| ow.len())
            .map(|ow| kw[..ow.len()].join(" "))
            .unwrap_or_else(|| k.to_string())
    };

    let mut out: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, k) in keys.iter().enumerate() {
        let k = base(k);
        match out.iter_mut().find(|(d, _)| d.eq_ignore_ascii_case(&k)) {
            Some((_, v)) => v.push(i),
            None => out.push((k, vec![i])),
        }
    }
    out
}

/// Doctrines from resolved fits, grouped by their names.
pub fn doctrines(fits: Vec<DoctrineFit>) -> Vec<Doctrine> {
    let names: Vec<String> = fits.iter().map(|f| f.name.clone()).collect();
    let mut fits: Vec<Option<DoctrineFit>> = fits.into_iter().map(Some).collect();
    group_names(&names)
        .into_iter()
        .map(|(name, idx)| Doctrine {
            id: dc::new_id(),
            name,
            notes: String::new(),
            fits: idx.into_iter().filter_map(|i| fits[i].take()).collect(),
        })
        .collect()
}

/// Adds `doctrines` as a new group named `name` inside the "Auto-generated" group (created when missing), leaving
/// everything else as it is. Returns the new group's id.
pub fn add_generated(doc: &mut DoctrineDoc, name: &str, doctrines: Vec<Doctrine>) -> String {
    let id = dc::new_id();
    let group = Group { id: id.clone(), name: name.to_string(), notes: String::new(), groups: Vec::new(), doctrines };
    match doc.groups.iter_mut().find(|g| g.name == AUTO_GROUP) {
        Some(auto) => auto.groups.push(group),
        None => doc.groups.push(Group {
            id: dc::new_id(),
            name: AUTO_GROUP.into(),
            notes: "Doctrines made from in-game fittings. Move what you want to keep into your own groups.".into(),
            groups: vec![group],
            doctrines: Vec::new(),
        }),
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn shape(v: &[&str]) -> Vec<(String, Vec<usize>)> {
        group_names(&names(v))
    }

    #[test]
    fn groups_by_separator_after_the_tag() {
        let got = shape(&["ABC Muninn - DPS", "ABC Muninn - Logi", "ABC Eagle - DPS", "abc muninn - Links"]);
        assert_eq!(got, [("Muninn".to_string(), vec![0, 1, 3]), ("Eagle".to_string(), vec![2])]);
    }

    #[test]
    fn groups_by_shared_words_without_separators() {
        let got = shape(&["[ABC] Hurricane Fleet Issue DPS", "[ABC] Hurricane Fleet Issue Logi", "[ABC] Ferox Navy", "[ABC] Muninn"]);
        assert_eq!(
            got,
            [("Hurricane Fleet Issue".to_string(), vec![0, 1]), ("Ferox Navy".to_string(), vec![2]), ("Muninn".to_string(), vec![3])]
        );
    }

    #[test]
    fn the_same_name_on_several_hulls_and_longer_names() {
        // Some alliances name fits this way: one name on several hulls, roles after the doctrine's name
        let got = shape(&[
            "x.Mainline",
            "x.Mainline Logi",
            "x.Mainline Logi",
            "x.Mainline Boost Shield/Info",
            "x.Mainline Boost Shield/Info",
            "x.Kite T2",
            "x.Kite T2",
            "x.Kite Logi",
            "x.Brawl",
            "x.Brawl",
        ]);
        assert_eq!(
            got,
            [("x.Mainline".to_string(), vec![0, 1, 2, 3, 4]), ("x.Kite".to_string(), vec![5, 6, 7]), ("x.Brawl".to_string(), vec![8, 9])]
        );
    }

    #[test]
    fn a_lone_fit_and_no_shared_tag() {
        assert_eq!(shape(&["Ishtar Sentry"]), [("Ishtar Sentry".to_string(), vec![0])]);
        // nothing is a tag when it would leave a name empty
        assert_eq!(shape(&["Muninn", "Muninn - Logi"]), [("Muninn".to_string(), vec![0, 1])]);
    }

    #[test]
    fn generated_groups_never_replace_anything() {
        let mut doc = DoctrineDoc::default();
        doc.groups.push(Group { id: "mine".into(), name: "ABC".into(), ..Default::default() });
        let d = |n: &str| Doctrine { id: dc::new_id(), name: n.into(), ..Default::default() };
        add_generated(&mut doc, "From Kira", vec![d("Muninn")]);
        add_generated(&mut doc, "From Kira", vec![d("Muninn")]);
        assert_eq!(doc.groups.len(), 2);
        assert_eq!(doc.groups[0].name, "ABC");
        let auto = &doc.groups[1];
        assert_eq!((auto.name.as_str(), auto.groups.len()), (AUTO_GROUP, 2));
        assert_ne!(auto.groups[0].id, auto.groups[1].id);
    }

    #[test]
    fn esi_fittings_become_parsed_fits() {
        let f: EsiFitting = serde_json::from_str(
            r#"{"fitting_id":1,"name":" ABC Muninn - DPS ","description":"","ship_type_id":12015,
               "items":[{"type_id":2048,"flag":"LoSlot0","quantity":1},{"type_id":2048,"flag":"LoSlot1","quantity":1}]}"#,
        )
        .unwrap();
        let p = parsed(f);
        assert_eq!((p.fit_name.as_deref(), &p.ship), (Some("ABC Muninn - DPS"), &ItemRef::TypeId(12015)));
        assert_eq!(p.items, vec![FitItem { item: ItemRef::TypeId(2048), quantity: 2 }]);
    }
}
