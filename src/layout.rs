use ratatui::layout::Rect;

use crate::pty::SessionId;

/// Split axis. Horizontal = panes side by side (vertical divider).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SplitDir {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NavDir {
    Left,
    Right,
    Up,
    Down,
}

pub enum PaneNode {
    Leaf(SessionId),
    Split {
        dir: SplitDir,
        ratio: f32,
        first: Box<PaneNode>,
        second: Box<PaneNode>,
    },
}

pub struct Tab {
    pub name: String,
    pub root: PaneNode,
    pub focus: SessionId,
    pub zoomed: bool,
}

impl Tab {
    pub fn new(name: String, id: SessionId) -> Self {
        Self {
            name,
            root: PaneNode::Leaf(id),
            focus: id,
            zoomed: false,
        }
    }

    pub fn leaves(&self) -> Vec<SessionId> {
        let mut out = Vec::new();
        self.root.leaves(&mut out);
        out
    }
}

impl PaneNode {
    pub fn leaves(&self, out: &mut Vec<SessionId>) {
        match self {
            PaneNode::Leaf(id) => out.push(*id),
            PaneNode::Split { first, second, .. } => {
                first.leaves(out);
                second.leaves(out);
            }
        }
    }

    pub fn contains(&self, target: SessionId) -> bool {
        match self {
            PaneNode::Leaf(id) => *id == target,
            PaneNode::Split { first, second, .. } => {
                first.contains(target) || second.contains(target)
            }
        }
    }

    pub fn first_leaf(&self) -> SessionId {
        match self {
            PaneNode::Leaf(id) => *id,
            PaneNode::Split { first, .. } => first.first_leaf(),
        }
    }

    /// Replace the `target` leaf with a split holding target + new_id.
    pub fn split(&mut self, target: SessionId, dir: SplitDir, new_id: SessionId) -> bool {
        match self {
            PaneNode::Leaf(id) if *id == target => {
                *self = PaneNode::Split {
                    dir,
                    ratio: 0.5,
                    first: Box::new(PaneNode::Leaf(target)),
                    second: Box::new(PaneNode::Leaf(new_id)),
                };
                true
            }
            PaneNode::Leaf(_) => false,
            PaneNode::Split { first, second, .. } => {
                first.split(target, dir, new_id) || second.split(target, dir, new_id)
            }
        }
    }

    /// Remove the `target` leaf, promoting its sibling. None = tree is now empty.
    pub fn remove(self, target: SessionId) -> Option<PaneNode> {
        match self {
            PaneNode::Leaf(id) => {
                if id == target {
                    None
                } else {
                    Some(PaneNode::Leaf(id))
                }
            }
            PaneNode::Split {
                dir,
                ratio,
                first,
                second,
            } => {
                if first.contains(target) {
                    match first.remove(target) {
                        None => Some(*second),
                        Some(f) => Some(PaneNode::Split {
                            dir,
                            ratio,
                            first: Box::new(f),
                            second,
                        }),
                    }
                } else {
                    match second.remove(target) {
                        None => Some(*first),
                        Some(s) => Some(PaneNode::Split {
                            dir,
                            ratio,
                            first,
                            second: Box::new(s),
                        }),
                    }
                }
            }
        }
    }

    pub fn rects(&self, area: Rect, out: &mut Vec<(SessionId, Rect)>) {
        match self {
            PaneNode::Leaf(id) => out.push((*id, area)),
            PaneNode::Split {
                dir,
                ratio,
                first,
                second,
            } => match dir {
                SplitDir::Horizontal => {
                    let total = area.width;
                    let w1 = if total <= 2 {
                        total / 2
                    } else {
                        ((f32::from(total) * ratio).round() as u16).clamp(1, total - 1)
                    };
                    first.rects(Rect { width: w1, ..area }, out);
                    second.rects(
                        Rect {
                            x: area.x + w1,
                            width: total - w1,
                            ..area
                        },
                        out,
                    );
                }
                SplitDir::Vertical => {
                    let total = area.height;
                    let h1 = if total <= 2 {
                        total / 2
                    } else {
                        ((f32::from(total) * ratio).round() as u16).clamp(1, total - 1)
                    };
                    first.rects(Rect { height: h1, ..area }, out);
                    second.rects(
                        Rect {
                            y: area.y + h1,
                            height: total - h1,
                            ..area
                        },
                        out,
                    );
                }
            },
        }
    }

    /// Grow (positive delta) or shrink the `target` pane along `axis` by
    /// adjusting the ratio of its nearest ancestor split on that axis.
    /// Returns (target found, ratio adjusted).
    pub fn adjust_ratio(&mut self, target: SessionId, axis: SplitDir, delta: f32) -> (bool, bool) {
        match self {
            PaneNode::Leaf(id) => (*id == target, false),
            PaneNode::Split {
                dir,
                ratio,
                first,
                second,
            } => {
                let (found, adjusted) = first.adjust_ratio(target, axis, delta);
                if found {
                    if !adjusted && *dir == axis {
                        *ratio = (*ratio + delta).clamp(0.1, 0.9);
                        return (true, true);
                    }
                    return (true, adjusted);
                }
                let (found, adjusted) = second.adjust_ratio(target, axis, delta);
                if found {
                    if !adjusted && *dir == axis {
                        *ratio = (*ratio - delta).clamp(0.1, 0.9);
                        return (true, true);
                    }
                    return (true, adjusted);
                }
                (false, false)
            }
        }
    }
}

/// Pick the nearest pane in `dir` from `from`, by centroid distance.
pub fn directional_focus(
    rects: &[(SessionId, Rect)],
    from: SessionId,
    dir: NavDir,
) -> Option<SessionId> {
    let cur = rects.iter().find(|(id, _)| *id == from)?.1;
    let (cx, cy) = center(cur);
    rects
        .iter()
        .filter(|(id, _)| *id != from)
        .filter(|(_, r)| match dir {
            NavDir::Left => r.x + r.width <= cur.x,
            NavDir::Right => r.x >= cur.x + cur.width,
            NavDir::Up => r.y + r.height <= cur.y,
            NavDir::Down => r.y >= cur.y + cur.height,
        })
        .min_by_key(|(_, r)| {
            let (x, y) = center(*r);
            let dx = i32::from(x) - i32::from(cx);
            let dy = i32::from(y) - i32::from(cy);
            dx * dx + dy * dy
        })
        .map(|(id, _)| *id)
}

fn center(r: Rect) -> (u16, u16) {
    (r.x + r.width / 2, r.y + r.height / 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(w: u16, h: u16) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        }
    }

    #[test]
    fn split_remove_promotes_sibling() {
        let mut root = PaneNode::Leaf(1);
        assert!(root.split(1, SplitDir::Horizontal, 2));
        assert!(root.split(2, SplitDir::Vertical, 3));
        assert_eq!(
            {
                let mut v = Vec::new();
                root.leaves(&mut v);
                v
            },
            vec![1, 2, 3]
        );
        let root = root.remove(2).expect("tree not empty");
        let mut v = Vec::new();
        root.leaves(&mut v);
        assert_eq!(v, vec![1, 3]);
        let root = root.remove(1).expect("tree not empty");
        assert!(matches!(root, PaneNode::Leaf(3)));
        assert!(root.remove(3).is_none());
    }

    #[test]
    fn rects_cover_area_without_overlap() {
        let mut root = PaneNode::Leaf(1);
        root.split(1, SplitDir::Horizontal, 2);
        root.split(2, SplitDir::Vertical, 3);
        let mut rects = Vec::new();
        root.rects(area(100, 40), &mut rects);
        assert_eq!(rects.len(), 3);
        let total: u32 = rects
            .iter()
            .map(|(_, r)| u32::from(r.width) * u32::from(r.height))
            .sum();
        assert_eq!(total, 100 * 40);
        // pane 1 fills the left half full-height
        assert_eq!(rects[0], (1, area(50, 40)));
    }

    #[test]
    fn rects_survive_tiny_areas() {
        let mut root = PaneNode::Leaf(1);
        root.split(1, SplitDir::Horizontal, 2);
        for w in 0..4u16 {
            for h in 0..4u16 {
                let mut rects = Vec::new();
                root.rects(area(w, h), &mut rects);
                assert_eq!(rects.len(), 2);
            }
        }
    }

    #[test]
    fn directional_focus_picks_neighbor() {
        let rects = vec![
            (1, area(50, 40)),
            (
                2,
                Rect {
                    x: 50,
                    y: 0,
                    width: 50,
                    height: 20,
                },
            ),
            (
                3,
                Rect {
                    x: 50,
                    y: 20,
                    width: 50,
                    height: 20,
                },
            ),
        ];
        assert_eq!(directional_focus(&rects, 1, NavDir::Right), Some(2));
        assert_eq!(directional_focus(&rects, 2, NavDir::Down), Some(3));
        assert_eq!(directional_focus(&rects, 3, NavDir::Left), Some(1));
        assert_eq!(directional_focus(&rects, 1, NavDir::Left), None);
    }

    #[test]
    fn adjust_ratio_targets_nearest_axis_ancestor() {
        let mut root = PaneNode::Leaf(1);
        root.split(1, SplitDir::Horizontal, 2);
        root.split(2, SplitDir::Vertical, 3);
        // pane 3 grows horizontally via the outer horizontal split
        let (found, adjusted) = root.adjust_ratio(3, SplitDir::Horizontal, 0.05);
        assert!(found && adjusted);
        if let PaneNode::Split { ratio, .. } = &root {
            // pane 3 is in the second child, so growing it shrinks the ratio
            assert!((*ratio - 0.45).abs() < f32::EPSILON);
        } else {
            panic!("root must be a split");
        }
    }
}
