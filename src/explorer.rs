use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub struct Entry {
    pub path: PathBuf,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
}

pub struct Explorer {
    /// Allowed folders: the explorer's top level and its upper boundary. It
    /// cannot navigate above these.
    pub roots: Vec<PathBuf>,
    pub entries: Vec<Entry>,
    pub selected: usize,
    pub offset: usize,
    pub show_hidden: bool,
    expanded: BTreeSet<PathBuf>,
}

impl Explorer {
    pub fn new(roots: Vec<PathBuf>) -> Self {
        let roots = if roots.is_empty() {
            vec![std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))]
        } else {
            roots
        };
        let mut ex = Self {
            roots,
            entries: Vec::new(),
            selected: 0,
            offset: 0,
            show_hidden: false,
            expanded: BTreeSet::new(),
        };
        ex.rebuild();
        ex
    }

    /// First allowed root; used as a fallback target when nothing is selected.
    fn primary(&self) -> PathBuf {
        self.roots
            .first()
            .cloned()
            .unwrap_or_else(|| PathBuf::from("."))
    }

    pub fn rebuild(&mut self) {
        let mut entries = Vec::new();
        if self.roots.len() == 1 {
            // a single root reads as its contents (no extra top-level node)
            walk(&self.roots[0], 0, self.show_hidden, &self.expanded, &mut entries);
        } else {
            // multiple roots: each is a top-level, expandable entry
            for root in &self.roots {
                let exp = self.expanded.contains(root);
                entries.push(Entry {
                    path: root.clone(),
                    depth: 0,
                    is_dir: true,
                    expanded: exp,
                });
                if exp {
                    walk(root, 1, self.show_hidden, &self.expanded, &mut entries);
                }
            }
        }
        self.entries = entries;
        self.selected = self.selected.min(self.entries.len().saturating_sub(1));
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        self.entries.get(self.selected)
    }

    pub fn move_sel(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        let max = self.entries.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, max) as usize;
    }

    /// Enter on a directory: toggle expansion.
    pub fn toggle_expand(&mut self) {
        let Some(e) = self.selected_entry() else {
            return;
        };
        if !e.is_dir {
            return;
        }
        let path = e.path.clone();
        if !self.expanded.remove(&path) {
            self.expanded.insert(path);
        }
        self.rebuild();
    }

    pub fn expand(&mut self) {
        if let Some(e) = self.selected_entry()
            && e.is_dir
            && !e.expanded
        {
            self.expanded.insert(e.path.clone());
            self.rebuild();
        }
    }

    /// h: collapse the selected dir, or jump to the parent entry.
    pub fn collapse_or_parent(&mut self) {
        let Some(e) = self.entries.get(self.selected) else {
            return;
        };
        if e.is_dir && e.expanded {
            self.expanded.remove(&e.path);
            self.rebuild();
            return;
        }
        let depth = e.depth;
        if depth == 0 {
            return;
        }
        for i in (0..self.selected).rev() {
            if self.entries[i].depth < depth {
                self.selected = i;
                break;
            }
        }
    }

    /// Backspace: collapse everything and return to the top of the allowed
    /// tree. The explorer is confined to `roots`, so there is no "up" past them.
    pub fn collapse_all(&mut self) {
        self.expanded.clear();
        self.selected = 0;
        self.offset = 0;
        self.rebuild();
    }

    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.rebuild();
    }

    /// Directory the selection refers to: the dir itself, a file's parent, or
    /// the first allowed root.
    pub fn target_dir(&self) -> PathBuf {
        match self.selected_entry() {
            Some(e) if e.is_dir => e.path.clone(),
            Some(e) => e
                .path
                .parent()
                .map_or_else(|| self.primary(), Path::to_path_buf),
            None => self.primary(),
        }
    }
}

fn walk(
    dir: &Path,
    depth: usize,
    show_hidden: bool,
    expanded: &BTreeSet<PathBuf>,
    out: &mut Vec<Entry>,
) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut items: Vec<(bool, String, PathBuf)> = rd
        .flatten()
        .filter_map(|de| {
            let name = de.file_name().to_string_lossy().into_owned();
            if !show_hidden && name.starts_with('.') {
                return None;
            }
            let path = de.path();
            Some((path.is_dir(), name, path))
        })
        .collect();
    items.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.to_lowercase().cmp(&b.1.to_lowercase())));

    for (is_dir, _, path) in items {
        let exp = is_dir && expanded.contains(&path);
        out.push(Entry {
            path: path.clone(),
            depth,
            is_dir,
            expanded: exp,
        });
        if exp {
            walk(&path, depth + 1, show_hidden, expanded, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mktree(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("mipoco_ex_{name}"));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("sub")).unwrap();
        std::fs::write(base.join("a.txt"), "x").unwrap();
        base
    }

    #[test]
    fn single_root_lists_contents_not_the_root() {
        let base = mktree("single");
        let ex = Explorer::new(vec![base.clone()]);
        assert!(ex.entries.iter().all(|e| e.path != base));
        assert!(ex.entries.iter().any(|e| e.path.ends_with("a.txt")));
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn multi_root_lists_each_root_at_top() {
        let base = mktree("multi");
        let r1 = base.join("sub");
        let ex = Explorer::new(vec![base.clone(), r1.clone()]);
        let top: Vec<_> = ex
            .entries
            .iter()
            .filter(|e| e.depth == 0)
            .map(|e| e.path.clone())
            .collect();
        assert_eq!(top, vec![base.clone(), r1.clone()]);
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn collapse_all_clears_expansion_and_resets() {
        let base = mktree("collapse");
        let mut ex = Explorer::new(vec![base.clone()]);
        ex.expanded.insert(base.join("sub"));
        ex.selected = 1;
        ex.rebuild();
        ex.collapse_all();
        assert!(ex.expanded.is_empty());
        assert_eq!(ex.selected, 0);
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn every_entry_stays_within_an_allowed_root() {
        let base = mktree("confine");
        std::fs::create_dir_all(base.join("sub/deep")).unwrap();
        let mut ex = Explorer::new(vec![base.clone()]);
        ex.expanded.insert(base.join("sub"));
        ex.expanded.insert(base.join("sub/deep"));
        ex.rebuild();
        assert!(ex.entries.iter().all(|e| e.path.starts_with(&base)));
        std::fs::remove_dir_all(&base).ok();
    }
}
