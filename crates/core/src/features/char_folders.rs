//! The navigation drawer's character list: the player's own order and folders (nested to any depth), stored in the
//! `character_folders` document. Folders are free-form, unlike accounts (`accounts`), which say which characters share
//! an EVE account and drive Queue Health; "Group by account" fills folders from those accounts as a starting point.
//!
//! The layout is kept in step with the characters PodStack has: characters that were removed are pruned and new ones
//! are added at the end of the top level, so with no saved layout the list is in the order characters were added.

use crate::store;
use crate::{Core, Event};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::LazyLock;

pub const DOC: &str = "character_folders";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Node {
    Character { id: i64 },
    Folder(Folder),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Folder {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub collapsed: bool,
    #[serde(default)]
    pub children: Vec<Node>,
}

/// Something in the tree that can be moved.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Item {
    Character(i64),
    Folder(String),
}

/// A place in the tree: position `index` among the children of folder `parent` (`None` = the top level).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub parent: Option<String>,
    pub index: usize,
}

impl Target {
    pub fn root_end() -> Self {
        Target { parent: None, index: usize::MAX }
    }
}

impl Node {
    fn is(&self, item: &Item) -> bool {
        match (self, item) {
            (Node::Character { id }, Item::Character(c)) => id == c,
            (Node::Folder(f), Item::Folder(id)) => &f.id == id,
            _ => false,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Layout {
    #[serde(default)]
    pub root: Vec<Node>,
}

fn find_folder<'a>(nodes: &'a [Node], id: &str) -> Option<&'a Folder> {
    nodes.iter().find_map(|n| match n {
        Node::Folder(f) if f.id == id => Some(f),
        Node::Folder(f) => find_folder(&f.children, id),
        Node::Character { .. } => None,
    })
}

fn find_folder_mut<'a>(nodes: &'a mut [Node], id: &str) -> Option<&'a mut Folder> {
    for n in nodes {
        if let Node::Folder(f) = n {
            if f.id == id {
                return Some(f);
            }
            if let Some(found) = find_folder_mut(&mut f.children, id) {
                return Some(found);
            }
        }
    }
    None
}

/// Removes `item` wherever it is; returns it with the folder it was in and its position there.
fn take(nodes: &mut Vec<Node>, parent: Option<&str>, item: &Item) -> Option<(Node, Option<String>, usize)> {
    if let Some(i) = nodes.iter().position(|n| n.is(item)) {
        return Some((nodes.remove(i), parent.map(str::to_string), i));
    }
    nodes.iter_mut().find_map(|n| match n {
        Node::Folder(f) => take(&mut f.children, Some(&f.id), item),
        Node::Character { .. } => None,
    })
}

fn collect_chars(nodes: &[Node], out: &mut Vec<i64>) {
    for n in nodes {
        match n {
            Node::Character { id } => out.push(*id),
            Node::Folder(f) => collect_chars(&f.children, out),
        }
    }
}

fn collect_folder_ids(nodes: &[Node], out: &mut Vec<String>) {
    for n in nodes {
        if let Node::Folder(f) = n {
            out.push(f.id.clone());
            collect_folder_ids(&f.children, out);
        }
    }
}

impl Layout {
    pub fn folder(&self, id: &str) -> Option<&Folder> {
        find_folder(&self.root, id)
    }

    /// Characters in the order the drawer lists them (folders flattened).
    pub fn order(&self) -> Vec<i64> {
        let mut v = Vec::new();
        collect_chars(&self.root, &mut v);
        v
    }

    fn children_mut(&mut self, parent: Option<&str>) -> Option<&mut Vec<Node>> {
        match parent {
            None => Some(&mut self.root),
            Some(id) => find_folder_mut(&mut self.root, id).map(|f| &mut f.children),
        }
    }

    /// Whether `folder` is `ancestor` or somewhere inside it.
    pub fn is_within(&self, folder: &str, ancestor: &str) -> bool {
        folder == ancestor || self.folder(ancestor).and_then(|a| find_folder(&a.children, folder)).is_some()
    }

    /// Makes the layout hold exactly `ids`: drops characters PodStack no longer has (and duplicates), and adds the
    /// missing ones at the end of the top level in the order given. Returns whether anything changed.
    pub fn sync(&mut self, ids: &[i64]) -> bool {
        fn prune(nodes: &mut Vec<Node>, keep: &HashSet<i64>, seen: &mut HashSet<i64>) -> bool {
            let before = nodes.len();
            let mut changed = false;
            nodes.retain_mut(|n| match n {
                Node::Character { id } => keep.contains(id) && seen.insert(*id),
                Node::Folder(f) => {
                    changed |= prune(&mut f.children, keep, seen);
                    true
                }
            });
            changed || nodes.len() != before
        }
        let keep: HashSet<i64> = ids.iter().copied().collect();
        let mut seen = HashSet::new();
        let mut changed = prune(&mut self.root, &keep, &mut seen);
        for id in ids {
            if !seen.contains(id) {
                self.root.push(Node::Character { id: *id });
                changed = true;
            }
        }
        changed
    }

    /// Moves `item` to `target`. A folder can't go into itself or its own subfolders, and the target folder must
    /// exist; returns false (and changes nothing) otherwise. `target.index` counts positions before the move, so
    /// dropping a row just below itself leaves it where it is.
    pub fn move_item(&mut self, item: &Item, target: &Target) -> bool {
        if let Some(p) = &target.parent {
            if self.folder(p).is_none() {
                return false;
            }
            if let Item::Folder(f) = item {
                if self.is_within(p, f) {
                    return false;
                }
            }
        }
        let Some((node, from, at)) = take(&mut self.root, None, item) else { return false };
        let mut index = target.index;
        if from == target.parent && at < index {
            index -= 1;
        }
        let Some(dest) = self.children_mut(target.parent.as_deref()) else { return false };
        let index = index.min(dest.len());
        dest.insert(index, node);
        true
    }

    /// A folder id not yet used.
    fn new_folder_id(&self) -> String {
        let mut ids = Vec::new();
        collect_folder_ids(&self.root, &mut ids);
        let used: HashSet<String> = ids.into_iter().collect();
        (1..).map(|n| format!("f{n}")).find(|id| !used.contains(id)).expect("unbounded")
    }

    /// Adds an empty folder at `target` (the top level's end when the target folder is gone); returns its id.
    pub fn insert_folder(&mut self, name: &str, target: &Target) -> String {
        let id = self.new_folder_id();
        let folder = Node::Folder(Folder { id: id.clone(), name: name.trim().to_string(), collapsed: false, children: Vec::new() });
        let dest = match self.children_mut(target.parent.as_deref()) {
            Some(d) => d,
            None => &mut self.root,
        };
        let i = target.index.min(dest.len());
        dest.insert(i, folder);
        id
    }

    pub fn rename_folder(&mut self, id: &str, name: &str) -> bool {
        match find_folder_mut(&mut self.root, id) {
            Some(f) => {
                f.name = name.trim().to_string();
                true
            }
            None => false,
        }
    }

    /// Removes a folder; what it held takes its place in the folder around it, so no character is lost.
    pub fn remove_folder(&mut self, id: &str) -> bool {
        let Some((node, parent, at)) = take(&mut self.root, None, &Item::Folder(id.to_string())) else { return false };
        let Node::Folder(f) = node else { return false };
        if let Some(dest) = self.children_mut(parent.as_deref()) {
            dest.splice(at..at, f.children);
        }
        true
    }

    pub fn set_collapsed(&mut self, id: &str, collapsed: bool) -> bool {
        match find_folder_mut(&mut self.root, id) {
            Some(f) if f.collapsed != collapsed => {
                f.collapsed = collapsed;
                true
            }
            _ => false,
        }
    }

    /// Replaces the layout with one folder per group (in the order given, empty groups skipped); characters in no
    /// group stay at the top level after the folders. Characters keep their current relative order.
    pub fn group(&mut self, groups: &[(String, Vec<i64>)]) {
        let order = self.order();
        let rank = |id: &i64| order.iter().position(|o| o == id).unwrap_or(usize::MAX);
        let mut fresh = Layout::default();
        let mut placed = HashSet::new();
        for (name, members) in groups {
            let mut members: Vec<i64> = members.iter().copied().filter(|id| order.contains(id) && placed.insert(*id)).collect();
            if members.is_empty() {
                continue;
            }
            members.sort_by_key(rank);
            let id = fresh.insert_folder(name, &Target::root_end());
            if let Some(f) = find_folder_mut(&mut fresh.root, &id) {
                f.children = members.into_iter().map(|id| Node::Character { id }).collect();
            }
        }
        fresh.root.extend(order.into_iter().filter(|id| !placed.contains(id)).map(|id| Node::Character { id }));
        *self = fresh;
    }
}

static LAYOUT: LazyLock<Mutex<Layout>> = LazyLock::new(|| Mutex::new(store::load_doc(DOC)));

fn change(core: &Core, f: impl FnOnce(&mut Layout)) {
    let snapshot = {
        let mut doc = LAYOUT.lock();
        let before = doc.clone();
        f(&mut doc);
        if *doc == before {
            return;
        }
        doc.clone()
    };
    store::save_doc(DOC, &snapshot);
    core.emit(Event::Doc(DOC.into()));
}

/// The layout, in step with the characters PodStack has now. Pruning or adding characters here isn't saved on its
/// own: the next change saves it, and until then a character that comes back keeps its old place.
pub fn layout(core: &Core) -> Layout {
    let ids: Vec<i64> = core.characters().iter().map(|c| c.id).collect();
    let mut l = LAYOUT.lock().clone();
    l.sync(&ids);
    l
}

/// Changes the layout (synced to the current characters first) and saves it.
fn edit(core: &Core, f: impl FnOnce(&mut Layout)) {
    let ids: Vec<i64> = core.characters().iter().map(|c| c.id).collect();
    change(core, |l| {
        l.sync(&ids);
        f(l);
    });
}

pub fn move_item(core: &Core, item: &Item, target: &Target) {
    edit(core, |l| {
        l.move_item(item, target);
    });
}

/// Creates a folder and returns its id.
pub fn create_folder(core: &Core, name: &str, target: &Target) -> String {
    let mut id = String::new();
    edit(core, |l| id = l.insert_folder(name, target));
    id
}

pub fn rename_folder(core: &Core, id: &str, name: &str) {
    edit(core, |l| {
        l.rename_folder(id, name);
    });
}

pub fn remove_folder(core: &Core, id: &str) {
    edit(core, |l| {
        l.remove_folder(id);
    });
}

pub fn set_collapsed(core: &Core, id: &str, collapsed: bool) {
    edit(core, |l| {
        l.set_collapsed(id, collapsed);
    });
}

/// Replaces the layout with one folder per group (see [`Layout::group`]).
pub fn group(core: &Core, groups: &[(String, Vec<i64>)]) {
    edit(core, |l| l.group(groups));
}

/// Groups for "Group by account": the player's accounts by name, then their characters.
pub fn account_groups(core: &Core) -> Vec<(String, Vec<i64>)> {
    let doc = super::accounts::export_all();
    let chars = core.characters();
    doc.list()
        .into_iter()
        .map(|a| (a.name, chars.iter().filter(|c| doc.account_of(c.id) == Some(a.id.as_str())).map(|c| c.id).collect()))
        .collect()
}

/// Groups for "Group by corporation", in the order the corporations first appear in the list.
pub fn corporation_groups(core: &Core) -> Vec<(String, Vec<i64>)> {
    let order = layout(core).order();
    let mut groups: Vec<(String, Vec<i64>)> = Vec::new();
    for id in order {
        let Some(corp) = core.character(id).and_then(|c| c.corporation_name().map(str::to_string)) else { continue };
        match groups.iter_mut().find(|(n, _)| *n == corp) {
            Some((_, v)) => v.push(id),
            None => groups.push((corp, vec![id])),
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(id: i64) -> Node {
        Node::Character { id }
    }

    fn folder(id: &str, children: Vec<Node>) -> Node {
        Node::Folder(Folder { id: id.into(), name: id.to_uppercase(), collapsed: false, children })
    }

    fn at(parent: Option<&str>, index: usize) -> Target {
        Target { parent: parent.map(str::to_string), index }
    }

    /// A compact picture of the tree: `1 2 a[3 b[4]]`.
    fn shape(nodes: &[Node]) -> String {
        nodes
            .iter()
            .map(|n| match n {
                Node::Character { id } => id.to_string(),
                Node::Folder(f) => format!("{}[{}]", f.id, shape(&f.children)),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn sample() -> Layout {
        Layout { root: vec![ch(1), folder("a", vec![ch(2), folder("b", vec![ch(3)])]), ch(4)] }
    }

    #[test]
    fn sync_without_layout_keeps_added_order() {
        let mut l = Layout::default();
        assert!(l.sync(&[5, 3, 9]));
        assert_eq!(shape(&l.root), "5 3 9");
        assert!(!l.sync(&[5, 3, 9]));
    }

    #[test]
    fn sync_prunes_missing_and_duplicates_and_appends_new() {
        let mut l = sample();
        l.root.push(ch(3));
        assert!(l.sync(&[1, 3, 4, 7]));
        assert_eq!(shape(&l.root), "1 a[b[3]] 4 7", "2 is gone, the second 3 dropped, 7 added at the end");
    }

    #[test]
    fn move_within_same_list_counts_positions_before_the_move() {
        let mut l = Layout { root: vec![ch(1), ch(2), ch(3)] };
        assert!(l.move_item(&Item::Character(1), &at(None, 2)));
        assert_eq!(shape(&l.root), "2 1 3", "dropped above 3");
        assert!(l.move_item(&Item::Character(3), &at(None, 0)));
        assert_eq!(shape(&l.root), "3 2 1");
        assert!(l.move_item(&Item::Character(2), &at(None, 2)));
        assert_eq!(shape(&l.root), "3 2 1", "just below itself: no change");
        assert!(l.move_item(&Item::Character(3), &Target::root_end()));
        assert_eq!(shape(&l.root), "2 1 3");
    }

    #[test]
    fn move_characters_into_out_of_and_between_folders() {
        let mut l = sample();
        assert!(l.move_item(&Item::Character(1), &at(Some("b"), 0)));
        assert_eq!(shape(&l.root), "a[2 b[1 3]] 4");
        assert!(l.move_item(&Item::Character(3), &at(None, 0)));
        assert_eq!(shape(&l.root), "3 a[2 b[1]] 4");
        assert!(l.move_item(&Item::Character(4), &at(Some("a"), usize::MAX)));
        assert_eq!(shape(&l.root), "3 a[2 b[1] 4]");
    }

    #[test]
    fn folders_move_but_never_into_themselves() {
        let mut l = sample();
        assert!(!l.move_item(&Item::Folder("a".into()), &at(Some("a"), 0)));
        assert!(!l.move_item(&Item::Folder("a".into()), &at(Some("b"), 0)), "into its own subfolder");
        assert!(!l.move_item(&Item::Character(1), &at(Some("nope"), 0)));
        assert_eq!(l, sample());
        assert!(l.move_item(&Item::Folder("b".into()), &at(None, 0)));
        assert_eq!(shape(&l.root), "b[3] 1 a[2] 4");
        assert!(l.move_item(&Item::Folder("a".into()), &at(Some("b"), 1)));
        assert_eq!(shape(&l.root), "b[3 a[2]] 1 4");
        assert!(l.is_within("a", "b") && !l.is_within("b", "a"));
    }

    #[test]
    fn insert_rename_and_remove_folders() {
        let mut l = sample();
        let id = l.insert_folder("  PvP ", &at(Some("a"), 1));
        assert_eq!(id, "f1");
        assert_eq!(l.folder(&id).map(|f| f.name.as_str()), Some("PvP"));
        assert_eq!(shape(&l.root), "1 a[2 f1[] b[3]] 4");
        assert_eq!(l.insert_folder("x", &at(Some("gone"), 0)), "f2", "a missing parent puts it at the top level");
        assert!(l.rename_folder("f2", "Scouts"));
        assert!(!l.rename_folder("zzz", "x"));
        assert!(l.remove_folder("a"));
        assert_eq!(shape(&l.root), "f2[] 1 2 f1[] b[3] 4", "its contents take its place");
        assert_eq!(l.order(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn collapsed_state_is_kept() {
        let mut l = sample();
        assert!(l.set_collapsed("b", true));
        assert!(!l.set_collapsed("b", true));
        let back: Layout = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
        assert!(back.folder("b").unwrap().collapsed);
        assert_eq!(back, l);
    }

    #[test]
    fn group_fills_folders_and_keeps_the_rest_at_the_top() {
        let mut l = Layout { root: vec![ch(4), ch(1), ch(3), ch(2)] };
        l.group(&[("Main".into(), vec![1, 4]), ("Empty".into(), vec![99]), ("Alts".into(), vec![2, 1])]);
        assert_eq!(shape(&l.root), "f1[4 1] f2[2] 3", "current order inside folders, a character only once");
        assert_eq!(l.folder("f1").unwrap().name, "Main");
        assert_eq!(l.folder("f2").unwrap().name, "Alts");
    }

    #[test]
    fn stored_format() {
        let l = Layout { root: vec![ch(1), folder("a", vec![ch(2)])] };
        assert_eq!(
            serde_json::to_string(&l).unwrap(),
            r#"{"root":[{"kind":"character","id":1},{"kind":"folder","id":"a","name":"A","collapsed":false,"children":[{"kind":"character","id":2}]}]}"#
        );
    }
}
