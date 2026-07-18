//! Session restore ("continue where you left off").
//!
//! When `restore_session` is enabled, mipoco continuously saves a snapshot of
//! the open tabs/splits/panes to `session.json` (a sibling of config.toml) and
//! replays it on the next launch — like a browser reopening its tabs. We store
//! only what is needed to *recreate* each pane (its kind + working directory)
//! and the split geometry; the live process state / scrollback is inherently
//! not restorable (a restored shell is a fresh process in the saved cwd).
//!
//! JSON is used rather than TOML because the layout is a recursive enum tree,
//! which serde_json handles cleanly (TOML's table model chokes on deep nesting).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::layout::SplitDir;

/// How a pane was created, so it can be recreated on restore. Transient panes
/// (runner/pager/external-viewer, all `auto_close`) carry no meta and are never
/// saved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PaneKind {
    Shell,
    Claude { bypass: bool },
    Viewer { path: PathBuf },
}

/// One serializable leaf/split node mirroring `layout::PaneNode` — reduced to
/// the data needed to rebuild the tree with fresh session ids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SavedNode {
    Leaf {
        pane: PaneKind,
        cwd: Option<PathBuf>,
        focused: bool,
    },
    Split {
        dir: SplitDir,
        ratio: f32,
        first: Box<SavedNode>,
        second: Box<SavedNode>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedTab {
    pub name: String,
    pub zoomed: bool,
    pub root: SavedNode,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedSession {
    pub tabs: Vec<SavedTab>,
    pub active_tab: usize,
    pub explorer_visible: bool,
}

pub fn session_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("mipoco").join("session.json"))
}

/// Load the saved session; never fails (missing/invalid file → empty).
pub fn load() -> SavedSession {
    let Some(path) = session_path() else {
        return SavedSession::default();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return SavedSession::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

/// Persist a pre-serialized snapshot string to `session.json`.
pub fn save_str(json: &str) -> anyhow::Result<PathBuf> {
    let path = session_path().ok_or_else(|| anyhow::anyhow!("no config directory found"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, json)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SavedSession {
        SavedSession {
            tabs: vec![
                SavedTab {
                    name: "work".into(),
                    zoomed: false,
                    root: SavedNode::Split {
                        dir: SplitDir::Horizontal,
                        ratio: 0.5,
                        first: Box::new(SavedNode::Leaf {
                            pane: PaneKind::Shell,
                            cwd: Some(PathBuf::from("/home/nfd/projects/mipoco")),
                            focused: false,
                        }),
                        second: Box::new(SavedNode::Split {
                            dir: SplitDir::Vertical,
                            ratio: 0.6,
                            first: Box::new(SavedNode::Leaf {
                                pane: PaneKind::Claude { bypass: true },
                                cwd: Some(PathBuf::from("/home/nfd")),
                                focused: true,
                            }),
                            second: Box::new(SavedNode::Leaf {
                                pane: PaneKind::Viewer {
                                    path: PathBuf::from("/home/nfd/README.md"),
                                },
                                cwd: None,
                                focused: false,
                            }),
                        }),
                    },
                },
                SavedTab {
                    name: "notes".into(),
                    zoomed: true,
                    root: SavedNode::Leaf {
                        pane: PaneKind::Claude { bypass: false },
                        cwd: None,
                        focused: true,
                    },
                },
            ],
            active_tab: 1,
            explorer_visible: true,
        }
    }

    #[test]
    fn round_trips_through_json() {
        let s = sample();
        let json = serde_json::to_string_pretty(&s).expect("serialize");
        let back: SavedSession = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(s, back);
    }

    #[test]
    fn deeply_nested_split_survives() {
        // Recursion that TOML would mangle serializes and parses fine as JSON.
        let s = sample();
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("Vertical") && json.contains("bypass"));
        let back: SavedSession = serde_json::from_str(&json).unwrap();
        assert_eq!(back.active_tab, 1);
        assert!(back.explorer_visible);
    }

    #[test]
    fn invalid_json_loads_as_empty() {
        let back: SavedSession = serde_json::from_str("{ not json").unwrap_or_default();
        assert_eq!(back, SavedSession::default());
        assert!(back.tabs.is_empty());
    }
}
