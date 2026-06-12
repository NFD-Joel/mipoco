use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub struct Entry {
    pub path: PathBuf,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
}

pub struct Explorer {
    pub root: PathBuf,
    pub entries: Vec<Entry>,
    pub selected: usize,
    pub offset: usize,
    pub show_hidden: bool,
    expanded: BTreeSet<PathBuf>,
}

impl Explorer {
    pub fn new(root: PathBuf) -> Self {
        let mut ex = Self {
            root,
            entries: Vec::new(),
            selected: 0,
            offset: 0,
            show_hidden: false,
            expanded: BTreeSet::new(),
        };
        ex.rebuild();
        ex
    }

    pub fn rebuild(&mut self) {
        let mut entries = Vec::new();
        walk(
            &self.root,
            0,
            self.show_hidden,
            &self.expanded,
            &mut entries,
        );
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

    /// Backspace: make the parent of the current root the new root.
    pub fn go_parent_root(&mut self) {
        if let Some(parent) = self.root.parent().map(Path::to_path_buf) {
            self.expanded.insert(self.root.clone());
            self.root = parent;
            self.selected = 0;
            self.offset = 0;
            self.rebuild();
        }
    }

    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.rebuild();
    }

    /// Directory the selection refers to: the dir itself, a file's parent, or root.
    pub fn target_dir(&self) -> PathBuf {
        match self.selected_entry() {
            Some(e) if e.is_dir => e.path.clone(),
            Some(e) => e
                .path
                .parent()
                .map_or_else(|| self.root.clone(), Path::to_path_buf),
            None => self.root.clone(),
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
