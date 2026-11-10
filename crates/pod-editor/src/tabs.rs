//! Editor tab set without GPUI: tab order, the preview tab, most-recently-used
//! order (Ctrl+Tab), positional switching (Ctrl+PageUp/PageDown) and the
//! reopen-closed stack (Ctrl+Shift+T). docs/13 §9.
//!
//! `EditorWorkspace` keeps its buffers in a `Vec` in the same order as
//! [`TabSet::keys`] and mirrors every insert/remove this model reports, so the
//! rules live (and are tested) here.
//!
//! Rules:
//! - **Preview**: a file opened by a single click in the tree or from quick
//!   open is a *preview* tab (italic title). At most one exists; opening
//!   another preview file replaces it in place. Editing, double-clicking the
//!   tab (or the tree row) or opening the file explicitly pins it.
//! - **New tabs** go right after the active tab.
//! - **Closing** the active tab activates the most recently used remaining
//!   tab. Closed tabs go on a stack (newest last, at most [`CLOSED_CAP`]).
//! - **Ctrl+Tab** walks the MRU list while Ctrl is held (a *switch*); the
//!   order is only updated when the switch ends ([`TabSet::commit_switch`]),
//!   so Ctrl+Tab, release, Ctrl+Tab toggles between the two latest files.
//! - **Moving** a tab (drag, Ctrl+Shift+PageUp/PageDown) pins it; the order
//!   is the persisted tab order. Moving does not wrap.
//! - **Bulk close** (tab menu): see [`BulkClose`]. Tabs close right to left,
//!   so Ctrl+Shift+T brings them back left to right at their old places.
//! - **Same names**: [`disambiguate`] gives the shortest parent-path suffix
//!   that tells same-named tabs apart.

/// How many closed tabs Ctrl+Shift+T can bring back.
pub const CLOSED_CAP: usize = 20;

/// A closed tab: where it was, so reopening puts it back there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Closed<K> {
    pub key: K,
    pub index: usize,
}

/// Result of [`TabSet::open`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened<K> {
    /// Tab index of the file, now active.
    pub index: usize,
    /// A new tab was inserted at `index` (the caller creates its buffer).
    pub created: bool,
    /// The previous preview tab, replaced in place at `index` (the caller
    /// drops its buffer first). Not pushed on the closed stack.
    pub replaced: Option<K>,
}

/// Which tabs a bulk close from the tab menu closes, relative to tab `at`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BulkClose {
    /// Every tab but `at` (which becomes active).
    Others,
    /// Tabs right of `at` (which becomes active).
    Right,
    /// Tabs without unsaved edits (the active tab stays if it is dirty).
    Saved,
    /// Every tab.
    All,
}

#[derive(Clone, Debug)]
pub struct TabSet<K> {
    keys: Vec<K>,
    preview: Option<K>,
    active: Option<usize>,
    /// Most recently used first.
    mru: Vec<K>,
    /// Oldest first.
    closed: Vec<Closed<K>>,
    /// Ctrl+Tab in progress: position in `mru`.
    switch: Option<usize>,
}

impl<K> Default for TabSet<K> {
    fn default() -> Self {
        TabSet {
            keys: Vec::new(),
            preview: None,
            active: None,
            mru: Vec::new(),
            closed: Vec::new(),
            switch: None,
        }
    }
}

impl<K: Clone + PartialEq> TabSet<K> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
    pub fn keys(&self) -> &[K] {
        &self.keys
    }
    pub fn index_of(&self, key: &K) -> Option<usize> {
        self.keys.iter().position(|k| k == key)
    }
    pub fn active(&self) -> Option<usize> {
        self.active
    }
    pub fn active_key(&self) -> Option<&K> {
        self.active.and_then(|i| self.keys.get(i))
    }
    pub fn preview(&self) -> Option<&K> {
        self.preview.as_ref()
    }
    pub fn is_preview(&self, key: &K) -> bool {
        self.preview.as_ref() == Some(key)
    }
    /// Most recently used first.
    pub fn mru(&self) -> &[K] {
        &self.mru
    }
    /// Closed tabs, oldest first.
    pub fn closed(&self) -> &[Closed<K>] {
        &self.closed
    }
    /// Whether a Ctrl+Tab switch is in progress.
    pub fn switching(&self) -> bool {
        self.switch.is_some()
    }

    /// Opens `key` (activating it). An existing tab is reused; `preview:
    /// false` pins it. A new preview replaces the current preview tab in
    /// place; any other new tab goes right after the active one.
    pub fn open(&mut self, key: K, preview: bool) -> Opened<K> {
        if let Some(i) = self.index_of(&key) {
            if !preview {
                self.pin(&key);
            }
            self.activate(i);
            return Opened {
                index: i,
                created: false,
                replaced: None,
            };
        }
        let replace = if preview {
            self.preview
                .take()
                .and_then(|p| self.index_of(&p).map(|i| (i, p)))
        } else {
            None
        };
        let (index, replaced) = match replace {
            Some((i, old)) => {
                self.mru.retain(|k| k != &old);
                self.keys[i] = key.clone();
                (i, Some(old))
            }
            None => {
                let at = self
                    .active
                    .map(|a| a + 1)
                    .unwrap_or(self.keys.len())
                    .min(self.keys.len());
                self.insert_raw(at, key.clone());
                (at, None)
            }
        };
        if preview {
            self.preview = Some(key);
        }
        self.activate(index);
        Opened {
            index,
            created: true,
            replaced,
        }
    }

    /// Appends a tab without activating it (session restore).
    pub fn push(&mut self, key: K, preview: bool) -> usize {
        if let Some(i) = self.index_of(&key) {
            return i;
        }
        if preview {
            self.preview = Some(key.clone());
        }
        self.keys.push(key.clone());
        self.mru.push(key);
        self.keys.len() - 1
    }

    /// Puts a tab back at `index` (clamped), pinned and active: Ctrl+Shift+T.
    pub fn insert_at(&mut self, key: K, index: usize) -> usize {
        if let Some(i) = self.index_of(&key) {
            self.activate(i);
            return i;
        }
        let at = index.min(self.keys.len());
        self.insert_raw(at, key);
        self.activate(at);
        at
    }

    fn insert_raw(&mut self, at: usize, key: K) {
        self.keys.insert(at, key.clone());
        if !self.mru.contains(&key) {
            self.mru.push(key);
        }
        if let Some(a) = self.active
            && a >= at
        {
            self.active = Some(a + 1);
        }
    }

    /// Makes a preview tab permanent. Returns whether it was the preview.
    pub fn pin(&mut self, key: &K) -> bool {
        if self.preview.as_ref() == Some(key) {
            self.preview = None;
            true
        } else {
            false
        }
    }

    /// Activates tab `i` and marks it most recently used (ends a switch).
    pub fn activate(&mut self, i: usize) {
        if i >= self.keys.len() {
            return;
        }
        self.switch = None;
        self.active = Some(i);
        let k = self.keys[i].clone();
        self.mru.retain(|x| x != &k);
        self.mru.insert(0, k);
    }

    /// Closes tab `i`. Returns the key and the new active index. With
    /// `remember`, Ctrl+Shift+T can reopen it (not for deleted files).
    pub fn close(&mut self, i: usize, remember: bool) -> Option<(K, Option<usize>)> {
        if i >= self.keys.len() {
            return None;
        }
        self.commit_switch();
        let key = self.keys.remove(i);
        self.mru.retain(|k| k != &key);
        if self.preview.as_ref() == Some(&key) {
            self.preview = None;
        }
        if remember {
            self.closed.retain(|c| c.key != key);
            self.closed.push(Closed {
                key: key.clone(),
                index: i,
            });
            if self.closed.len() > CLOSED_CAP {
                self.closed.remove(0);
            }
        }
        self.active = match self.active {
            _ if self.keys.is_empty() => None,
            Some(a) if a == i => self
                .mru
                .first()
                .and_then(|k| self.index_of(k))
                .or(Some(i.min(self.keys.len() - 1))),
            Some(a) if a > i => Some(a - 1),
            other => other,
        };
        if let Some(a) = self.active {
            // The new active tab is the most recent now.
            let k = self.keys[a].clone();
            self.mru.retain(|x| x != &k);
            self.mru.insert(0, k);
        }
        Some((key, self.active))
    }

    /// Newest closed tab that is not open again (Ctrl+Shift+T). The caller
    /// loads it and calls [`Self::insert_at`]; a file that no longer loads
    /// is simply dropped (call again for the next one).
    pub fn pop_closed(&mut self) -> Option<Closed<K>> {
        while let Some(c) = self.closed.pop() {
            if self.index_of(&c.key).is_none() {
                return Some(c);
            }
        }
        None
    }

    /// Ctrl+PageDown / Ctrl+PageUp: the next/previous tab by position (wraps).
    pub fn step_position(&mut self, forward: bool) -> Option<usize> {
        let n = self.keys.len();
        if n == 0 {
            return None;
        }
        let a = self.active.unwrap_or(0);
        let i = if forward {
            (a + 1) % n
        } else {
            (a + n - 1) % n
        };
        self.activate(i);
        Some(i)
    }

    /// Ctrl+Tab (`forward`) / Ctrl+Shift+Tab while Ctrl is held: the next
    /// entry of the MRU list as it was when the switch began. The MRU order
    /// is updated by [`Self::commit_switch`] (Ctrl released).
    pub fn mru_step(&mut self, forward: bool) -> Option<usize> {
        let n = self.mru.len();
        if n == 0 {
            return None;
        }
        let pos = self.switch.unwrap_or(0);
        let pos = if forward {
            (pos + 1) % n
        } else {
            (pos + n - 1) % n
        };
        self.switch = Some(pos);
        let i = self.index_of(&self.mru[pos])?;
        self.active = Some(i);
        Some(i)
    }

    /// Ends a Ctrl+Tab switch: the tab it landed on becomes most recent.
    pub fn commit_switch(&mut self) {
        if self.switch.take().is_some()
            && let Some(a) = self.active
        {
            let k = self.keys[a].clone();
            self.mru.retain(|x| x != &k);
            self.mru.insert(0, k);
        }
    }

    /// A file was renamed: its tab (and history) follow.
    pub fn rename(&mut self, from: &K, to: K) {
        for k in self
            .keys
            .iter_mut()
            .chain(self.mru.iter_mut())
            .chain(self.preview.iter_mut())
        {
            if k == from {
                *k = to.clone();
            }
        }
        for c in &mut self.closed {
            if &c.key == from {
                c.key = to.clone();
            }
        }
    }

    /// Moves tab `from` so it ends at index `to` (clamped). The active tab
    /// stays active wherever it lands; a moved preview tab is pinned (a
    /// deliberate placement). Returns the final index, `None` if nothing moved.
    pub fn move_tab(&mut self, from: usize, to: usize) -> Option<usize> {
        let n = self.keys.len();
        if from >= n {
            return None;
        }
        let to = to.min(n - 1);
        let key = self.keys[from].clone();
        let pinned = self.pin(&key);
        if from == to {
            return pinned.then_some(to);
        }
        let active = self.active_key().cloned();
        self.keys.remove(from);
        self.keys.insert(to, key);
        self.active = active.and_then(|k| self.index_of(&k));
        Some(to)
    }

    /// Final index of tab `from` dropped at insertion slot `slot` (0..=len:
    /// before tab `slot`, or after the last one), for [`Self::move_tab`].
    pub fn drop_index(&self, from: usize, slot: usize) -> usize {
        let slot = slot.min(self.keys.len());
        if slot > from { slot - 1 } else { slot }
    }

    /// Ctrl+Shift+PageDown / PageUp: moves the active tab one place right /
    /// left (no wrap). Returns `(from, to)` when it moved.
    pub fn move_active(&mut self, forward: bool) -> Option<(usize, usize)> {
        let a = self.active?;
        let to = if forward { a + 1 } else { a.checked_sub(1)? };
        if to >= self.keys.len() {
            return None;
        }
        self.move_tab(a, to).map(|t| (a, t))
    }

    /// Makes `key` the preview tab again (tab menu "고정 해제"). The previous
    /// preview tab, if any, is pinned rather than dropped; it is returned.
    pub fn unpin(&mut self, key: &K) -> Option<K> {
        if self.index_of(key).is_none() || self.is_preview(key) {
            return None;
        }
        self.preview.replace(key.clone())
    }

    /// Tab indices (ascending) a bulk close at tab `at` targets. `dirty`
    /// tells unsaved tabs (only [`BulkClose::Saved`] looks at it; the
    /// caller asks before closing dirty targets of the others).
    pub fn bulk_targets(
        &self,
        kind: BulkClose,
        at: usize,
        dirty: impl Fn(&K) -> bool,
    ) -> Vec<usize> {
        let n = self.keys.len();
        match kind {
            BulkClose::Others => (0..n).filter(|&i| i != at).collect(),
            BulkClose::Right => (at.saturating_add(1)..n).collect(),
            BulkClose::Saved => (0..n).filter(|&i| !dirty(&self.keys[i])).collect(),
            BulkClose::All => (0..n).collect(),
        }
    }

    /// Closes the tabs at `indices` (remembered for Ctrl+Shift+T), right to
    /// left, then activates `then` if it is still open (otherwise the most
    /// recently used tab is active). Returns `(index, key)` in removal order,
    /// so a caller mirroring the tabs in a `Vec` removes the same positions.
    pub fn close_many(&mut self, indices: &[usize], then: Option<&K>) -> Vec<(usize, K)> {
        let mut ix: Vec<usize> = indices
            .iter()
            .copied()
            .filter(|&i| i < self.keys.len())
            .collect();
        ix.sort_unstable();
        ix.dedup();
        let mut out = Vec::with_capacity(ix.len());
        for &i in ix.iter().rev() {
            if let Some((k, _)) = self.close(i, true) {
                out.push((i, k));
            }
        }
        if let Some(i) = then.and_then(|k| self.index_of(k)) {
            self.activate(i);
        }
        out
    }

    /// MRU order as tab indices (layout persistence).
    pub fn mru_indices(&self) -> Vec<usize> {
        self.mru.iter().filter_map(|k| self.index_of(k)).collect()
    }

    /// Restores the MRU order from tab indices (unknown ones ignored; tabs
    /// missing from the list keep their place after the listed ones).
    pub fn set_mru_indices(&mut self, order: &[usize]) {
        let mut mru: Vec<K> = Vec::with_capacity(self.keys.len());
        for &i in order {
            if let Some(k) = self.keys.get(i)
                && !mru.contains(k)
            {
                mru.push(k.clone());
            }
        }
        for k in &self.mru {
            if !mru.contains(k) {
                mru.push(k.clone());
            }
        }
        self.mru = mru;
    }
}

/// Same-name disambiguation for tab titles. `rels` are root-relative paths
/// with `/` separators. For each path whose file name is shared with another
/// tab (compared case-insensitively, as tab keys are on Windows) this gives
/// the shortest suffix of its parent directories that differs from every
/// other same-named path's suffix of the same length, e.g. `podd/src` vs
/// `pod-app/src` for two `src/mod.rs`. A file at the root shows `./`. Unique
/// names get `None`. Paths that cannot be told apart (equal up to case) show
/// their whole parent.
pub fn disambiguate(rels: &[&str]) -> Vec<Option<String>> {
    fn split(rel: &str) -> (String, Vec<&str>) {
        let parts: Vec<&str> = rel.split('/').filter(|p| !p.is_empty()).collect();
        let name = parts.last().copied().unwrap_or("").to_lowercase();
        let dirs = parts[..parts.len().saturating_sub(1)].to_vec();
        (name, dirs)
    }
    // Last `k` directories, lowercased, for comparison.
    fn suffix(dirs: &[&str], k: usize) -> Vec<String> {
        dirs[dirs.len().saturating_sub(k)..]
            .iter()
            .map(|d| d.to_lowercase())
            .collect()
    }
    let split: Vec<(String, Vec<&str>)> = rels.iter().map(|r| split(r)).collect();
    split
        .iter()
        .enumerate()
        .map(|(i, (name, dirs))| {
            let others: Vec<&Vec<&str>> = split
                .iter()
                .enumerate()
                .filter(|(j, (n, _))| *j != i && n == name)
                .map(|(_, (_, d))| d)
                .collect();
            if others.is_empty() {
                return None;
            }
            if dirs.is_empty() {
                return Some("./".to_string());
            }
            let k = (1..=dirs.len())
                .find(|&k| others.iter().all(|o| suffix(dirs, k) != suffix(o, k)))
                .unwrap_or(dirs.len());
            Some(dirs[dirs.len() - k..].join("/"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(keys: &[&'static str]) -> TabSet<&'static str> {
        let mut t = TabSet::new();
        for k in keys {
            t.open(*k, false);
        }
        t
    }

    #[test]
    fn new_tabs_go_after_the_active_one() {
        let mut t = set(&["a", "b", "c"]);
        assert_eq!(t.keys(), ["a", "b", "c"]);
        t.activate(0);
        let o = t.open("d", false);
        assert_eq!(
            o,
            Opened {
                index: 1,
                created: true,
                replaced: None
            }
        );
        assert_eq!(t.keys(), ["a", "d", "b", "c"]);
        assert_eq!(t.active(), Some(1));
        // Opening an open file only activates it.
        assert_eq!(
            t.open("c", false),
            Opened {
                index: 3,
                created: false,
                replaced: None
            }
        );
        assert_eq!(t.len(), 4);
    }

    #[test]
    fn preview_is_replaced_in_place_until_pinned() {
        let mut t = set(&["a", "b"]);
        t.activate(0);
        let o = t.open("p1", true);
        assert_eq!((o.index, o.replaced), (1, None));
        assert!(t.is_preview(&"p1"));
        // Another single click replaces the preview where it stands.
        t.activate(2);
        let o = t.open("p2", true);
        assert_eq!(
            o,
            Opened {
                index: 1,
                created: true,
                replaced: Some("p1")
            }
        );
        assert_eq!(t.keys(), ["a", "p2", "b"]);
        assert!(!t.mru().contains(&"p1"));
        assert!(
            t.closed().is_empty(),
            "a replaced preview is not a closed tab"
        );
        // Re-opening the preview file as preview keeps it preview; pinned
        // explicitly (double click / edit / explicit open) it stays.
        assert!(!t.open("p2", true).created);
        assert!(t.is_preview(&"p2"));
        t.open("p2", false);
        assert_eq!(t.preview(), None);
        let o = t.open("p3", true);
        assert_eq!(o.replaced, None, "pinned tabs are never replaced");
        assert_eq!(t.keys(), ["a", "p2", "p3", "b"]);
        assert!(t.pin(&"p3"));
        assert!(!t.pin(&"p3"));
    }

    #[test]
    fn closing_activates_the_most_recent_and_remembers() {
        let mut t = set(&["a", "b", "c", "d"]);
        t.activate(0); // a, then d was before
        t.activate(2); // c
        // MRU: c a d b
        let (k, active) = t.close(2, true).unwrap();
        assert_eq!(k, "c");
        assert_eq!(t.keys(), ["a", "b", "d"]);
        assert_eq!(active, Some(0), "most recently used (a), not the neighbour");
        assert_eq!(t.mru(), ["a", "d", "b"]);
        // Closing a tab left of the active one shifts the active index.
        t.activate(2); // d
        let (_, active) = t.close(0, true).unwrap();
        assert_eq!(t.keys(), ["b", "d"]);
        assert_eq!(active, Some(1));
        assert_eq!(
            t.closed().iter().map(|c| c.key).collect::<Vec<_>>(),
            ["c", "a"]
        );
        t.close(0, false);
        t.close(0, true);
        assert!(t.is_empty());
        assert_eq!(t.active(), None);
        assert_eq!(
            t.closed().iter().map(|c| c.key).collect::<Vec<_>>(),
            ["c", "a", "d"],
            "not remembered: b"
        );
    }

    #[test]
    fn reopen_closed_restores_position_newest_first_and_skips_open_files() {
        let mut t = set(&["a", "b", "c"]);
        t.close(1, true); // b at 1
        t.close(0, true); // a at 0
        let c = t.pop_closed().unwrap();
        assert_eq!(c, Closed { key: "a", index: 0 });
        assert_eq!(t.insert_at(c.key, c.index), 0);
        assert_eq!(t.keys(), ["a", "c"]);
        // b is opened again by hand: Ctrl+Shift+T skips it.
        t.open("b", false);
        assert_eq!(t.pop_closed(), None);
        // Index past the end is clamped.
        let mut t = set(&["a", "b", "c"]);
        t.close(2, true);
        t.close(1, true);
        t.close(0, true);
        let c = t.pop_closed().unwrap();
        assert_eq!(t.insert_at(c.key, c.index), 0);
        let c = t.pop_closed().unwrap();
        assert_eq!((c.key, t.insert_at(c.key, c.index)), ("b", 1));
        let c = t.pop_closed().unwrap();
        assert_eq!((c.key, t.insert_at(c.key, c.index)), ("c", 2));
        assert_eq!(t.keys(), ["a", "b", "c"]);
        assert_eq!(t.active(), Some(2));
    }

    #[test]
    fn closed_stack_is_capped_and_deduplicated() {
        let mut t: TabSet<usize> = TabSet::new();
        for i in 0..30 {
            t.open(i, false);
        }
        for _ in 0..30 {
            t.close(0, true);
        }
        assert_eq!(t.closed().len(), CLOSED_CAP);
        assert_eq!(t.closed().last().unwrap().key, 29);
        assert_eq!(t.closed()[0].key, 10);
        t.open(29, false);
        t.close(0, true);
        assert_eq!(t.closed().iter().filter(|c| c.key == 29).count(), 1);
    }

    #[test]
    fn ctrl_tab_walks_mru_while_held_and_toggles_after_release() {
        let mut t = set(&["a", "b", "c", "d"]); // MRU: d c b a
        // Ctrl+Tab once, release: previous file (c).
        assert_eq!(t.mru_step(true), Some(2));
        assert!(t.switching());
        t.commit_switch();
        assert_eq!(t.mru(), ["c", "d", "b", "a"]);
        // Again: back to d (toggle).
        assert_eq!(t.mru_step(true), Some(3));
        t.commit_switch();
        assert_eq!(t.active_key(), Some(&"d"));
        // Held: Tab Tab Tab walks c, b, a without reordering.
        assert_eq!(t.mru_step(true), Some(2));
        assert_eq!(t.mru_step(true), Some(1));
        assert_eq!(t.mru_step(true), Some(0));
        assert_eq!(
            t.mru(),
            ["d", "c", "b", "a"],
            "order frozen during the switch"
        );
        // Shift+Tab steps back, wrap-around both ways.
        assert_eq!(t.mru_step(false), Some(1));
        t.commit_switch();
        assert_eq!(t.mru(), ["b", "d", "c", "a"]);
        assert_eq!(
            t.mru_step(false),
            Some(0),
            "Ctrl+Shift+Tab from the start wraps to the oldest (a)"
        );
        t.commit_switch();
        assert_eq!(t.active_key(), Some(&"a"));
        // Any other activation ends a switch, too.
        t.mru_step(true);
        t.activate(3);
        assert!(!t.switching());
        assert_eq!(t.mru()[0], "d");
    }

    #[test]
    fn ctrl_page_keys_move_by_position_and_wrap() {
        let mut t = set(&["a", "b", "c"]);
        t.activate(0);
        assert_eq!(t.step_position(true), Some(1));
        assert_eq!(t.step_position(true), Some(2));
        assert_eq!(t.step_position(true), Some(0));
        assert_eq!(t.step_position(false), Some(2));
        assert_eq!(t.mru()[0], "c");
        let mut empty: TabSet<u8> = TabSet::new();
        assert_eq!(empty.step_position(true), None);
        assert_eq!(empty.mru_step(true), None);
    }

    #[test]
    fn rename_follows_tabs_history_and_preview() {
        let mut t = set(&["a", "b"]);
        t.open("p", true);
        t.close(0, true);
        t.rename(&"p", "q");
        t.rename(&"a", "z");
        assert_eq!(t.keys(), ["b", "q"]);
        assert!(t.is_preview(&"q"));
        assert!(t.mru().contains(&"q"));
        assert_eq!(t.closed()[0].key, "z");
    }

    #[test]
    fn mru_round_trips_through_indices() {
        let mut t = set(&["a", "b", "c"]);
        t.activate(0);
        t.activate(2);
        let saved = t.mru_indices();
        assert_eq!(saved, [2, 0, 1]);
        let mut r: TabSet<&str> = TabSet::new();
        for k in ["a", "b", "c"] {
            r.push(k, k == "b");
        }
        r.set_mru_indices(&saved);
        r.activate(2);
        assert_eq!(r.mru(), ["c", "a", "b"]);
        assert!(r.is_preview(&"b"));
        // Bad indices are ignored; unlisted tabs follow.
        r.set_mru_indices(&[9, 1]);
        assert_eq!(r.mru(), ["b", "c", "a"]);
    }

    #[test]
    fn move_tab_keeps_the_active_key_and_pins() {
        let mut t = set(&["a", "b", "c", "d"]);
        t.activate(1); // b
        assert_eq!(t.move_tab(0, 3), Some(3));
        assert_eq!(t.keys(), ["b", "c", "d", "a"]);
        assert_eq!(t.active_key(), Some(&"b"), "the active tab follows its key");
        assert_eq!(t.move_tab(3, 0), Some(0));
        assert_eq!(t.keys(), ["a", "b", "c", "d"]);
        assert_eq!(t.move_tab(1, 1), None, "same place: nothing moved");
        assert_eq!(t.move_tab(9, 0), None);
        assert_eq!(t.move_tab(0, 99), Some(3), "target clamped");
        assert_eq!(t.keys(), ["b", "c", "d", "a"]);
        // Moving (dragging) a preview tab pins it, even onto its own place.
        let mut t = set(&["a", "b"]);
        t.open("p", true);
        assert_eq!(t.keys(), ["a", "b", "p"]);
        assert_eq!(t.move_tab(2, 2), Some(2));
        assert_eq!(t.preview(), None);
        t.open("q", true);
        t.move_tab(3, 0);
        assert_eq!(t.keys(), ["q", "a", "b", "p"]);
        assert!(!t.is_preview(&"q"));
        // MRU and closed history are not reordered by a move.
        assert_eq!(t.mru()[0], "q");
    }

    #[test]
    fn drop_slots_map_to_final_indices() {
        let t = set(&["a", "b", "c", "d"]);
        // Dragging b (1): before a, before b/c (no move), before d, after d.
        assert_eq!(t.drop_index(1, 0), 0);
        assert_eq!(t.drop_index(1, 1), 1);
        assert_eq!(t.drop_index(1, 2), 1);
        assert_eq!(t.drop_index(1, 3), 2);
        assert_eq!(t.drop_index(1, 4), 3);
        assert_eq!(t.drop_index(1, 99), 3);
        assert_eq!(t.drop_index(3, 0), 0);
    }

    #[test]
    fn move_active_steps_without_wrapping() {
        let mut t = set(&["a", "b", "c"]);
        t.activate(0);
        assert_eq!(t.move_active(false), None, "already first");
        assert_eq!(t.move_active(true), Some((0, 1)));
        assert_eq!(t.move_active(true), Some((1, 2)));
        assert_eq!(t.move_active(true), None, "already last");
        assert_eq!(t.keys(), ["b", "c", "a"]);
        assert_eq!(t.active_key(), Some(&"a"));
        assert_eq!(t.move_active(false), Some((2, 1)));
        assert_eq!(t.keys(), ["b", "a", "c"]);
        let mut empty: TabSet<u8> = TabSet::new();
        assert_eq!(empty.move_active(true), None);
    }

    #[test]
    fn unpin_makes_a_tab_the_preview_and_pins_the_old_one() {
        let mut t = set(&["a", "b"]);
        assert_eq!(t.unpin(&"a"), None);
        assert!(t.is_preview(&"a"));
        assert_eq!(
            t.unpin(&"b"),
            Some("a"),
            "the old preview is pinned, not closed"
        );
        assert!(t.is_preview(&"b") && !t.is_preview(&"a"));
        assert_eq!(t.unpin(&"b"), None, "already the preview");
        assert_eq!(t.unpin(&"zz"), None, "not open");
        // The next preview open replaces the unpinned tab in place.
        let o = t.open("c", true);
        assert_eq!(o.replaced, Some("b"));
        assert_eq!(t.keys(), ["a", "c"]);
    }

    #[test]
    fn bulk_targets_by_kind() {
        let t = set(&["a", "b", "c", "d"]);
        let dirty = |k: &&str| *k == "b" || *k == "d";
        assert_eq!(t.bulk_targets(BulkClose::Others, 1, dirty), [0, 2, 3]);
        assert_eq!(t.bulk_targets(BulkClose::Right, 1, dirty), [2, 3]);
        assert_eq!(t.bulk_targets(BulkClose::Right, 3, dirty), [] as [usize; 0]);
        assert_eq!(
            t.bulk_targets(BulkClose::Saved, 1, dirty),
            [0, 2],
            "dirty tabs stay"
        );
        assert_eq!(t.bulk_targets(BulkClose::All, 1, dirty), [0, 1, 2, 3]);
        let empty: TabSet<&str> = TabSet::new();
        assert!(
            empty
                .bulk_targets(BulkClose::Others, 0, |_| false)
                .is_empty()
        );
        assert!(
            empty
                .bulk_targets(BulkClose::Right, usize::MAX, |_| false)
                .is_empty()
        );
    }

    #[test]
    fn close_many_goes_right_to_left_and_reopens_in_place() {
        let mut t = set(&["a", "b", "c", "d", "e"]);
        t.activate(4); // e
        // "다른 탭 닫기" on b: b becomes active.
        let targets = t.bulk_targets(BulkClose::Others, 1, |_| false);
        let removed = t.close_many(&targets, Some(&"b"));
        assert_eq!(
            removed,
            [(4, "e"), (3, "d"), (2, "c"), (0, "a")],
            "removal order for mirrored Vecs"
        );
        assert_eq!(t.keys(), ["b"]);
        assert_eq!(t.active_key(), Some(&"b"));
        assert_eq!(t.mru(), ["b"]);
        // Ctrl+Shift+T brings them back left to right at their old places.
        while let Some(c) = t.pop_closed() {
            t.insert_at(c.key, c.index);
        }
        assert_eq!(t.keys(), ["a", "b", "c", "d", "e"]);
        // "오른쪽 탭 닫기" while a right tab is active: the menu's tab wins.
        let mut t = set(&["a", "b", "c", "d"]);
        let targets = t.bulk_targets(BulkClose::Right, 1, |_| false);
        assert_eq!(t.close_many(&targets, Some(&"b")).len(), 2);
        assert_eq!((t.keys(), t.active_key()), (&["a", "b"][..], Some(&"b")));
        // "저장된 탭 닫기": the active dirty tab stays active; a closed active
        // tab hands over to the most recent remaining one.
        let mut t = set(&["a", "b", "c", "d"]);
        t.activate(1); // b (dirty)
        let targets = t.bulk_targets(BulkClose::Saved, 1, |k| *k == "b" || *k == "d");
        t.close_many(&targets, None);
        assert_eq!((t.keys(), t.active_key()), (&["b", "d"][..], Some(&"b")));
        let mut t = set(&["a", "b", "c", "d"]); // d active, MRU d c b a
        let targets = t.bulk_targets(BulkClose::Saved, 0, |k| *k == "b");
        t.close_many(&targets, None);
        assert_eq!((t.keys(), t.active_key()), (&["b"][..], Some(&"b")));
        // "모두 닫기": empty, nothing active, all remembered; duplicates and
        // stale indices are ignored.
        let mut t = set(&["a", "b", "c"]);
        let removed = t.close_many(&[2, 0, 1, 1, 7], Some(&"b"));
        assert_eq!(removed.len(), 3);
        assert!(t.is_empty() && t.active().is_none());
        assert_eq!(t.closed().len(), 3);
        // A preview tab closed in bulk is no longer the preview.
        let mut t = set(&["a"]);
        t.open("p", true);
        t.close_many(&[1], None);
        assert_eq!(t.preview(), None);
    }

    #[test]
    fn disambiguation_uses_the_shortest_distinguishing_parent_suffix() {
        let d = |v: &[&str]| disambiguate(v);
        let s = |x: &str| Some(x.to_string());
        // Unique names: nothing extra.
        assert_eq!(d(&["src/main.rs", "README.md"]), [None, None]);
        // Same parent name: one more level tells them apart.
        assert_eq!(
            d(&[
                "crates/podd/src/mod.rs",
                "crates/pod-app/src/mod.rs",
                "crates/podd/src/lib.rs"
            ]),
            [s("podd/src"), s("pod-app/src"), None]
        );
        // Different parents: the parent alone.
        assert_eq!(d(&["a/x/mod.rs", "b/y/mod.rs"]), [s("x"), s("y")]);
        // Three-way, deep, shared middle levels: each against all others.
        assert_eq!(
            d(&["p/q/r/s/t/f.rs", "p/z/r/s/t/f.rs", "u/s/t/f.rs"]),
            [s("q/r/s/t"), s("z/r/s/t"), s("u/s/t")]
        );
        // One parent is a suffix of the other: the shorter shows its whole parent.
        assert_eq!(d(&["src/mod.rs", "a/src/mod.rs"]), [s("src"), s("a/src")]);
        // Root-level file.
        assert_eq!(d(&["mod.rs", "src/mod.rs"]), [s("./"), s("src")]);
        // Case-insensitive like tab keys; equal paths fall back to the whole parent.
        assert_eq!(
            d(&["Src/Mod.rs", "lib/src/mod.rs"]),
            [s("Src"), s("lib/src")]
        );
        assert_eq!(d(&["a/b/x.rs", "A/B/x.rs"]), [s("a/b"), s("A/B")]);
        // Korean names and empty input.
        assert_eq!(d(&["문서/노트.md", "기록/노트.md"]), [s("문서"), s("기록")]);
        assert!(d(&[]).is_empty());
    }
}
