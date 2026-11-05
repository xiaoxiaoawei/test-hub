//! Limits and ordering for a hit-tested UIA subtree. This module has no provider
//! calls so the selection boundaries can be tested without a browser or COM.

pub(super) const MAX_DEPTH: usize = 8;
pub(super) const MAX_SIBLINGS: usize = 24;
const MAX_UNKNOWN_BOUNDS: usize = 2;
const HIT_SLOP: i64 = 12;

#[derive(Clone, Copy)]
pub(super) struct Bounds {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Clone, Copy)]
pub(super) struct Gesture {
    pub origin: Option<(i32, i32)>,
    pub end: (i32, i32),
}

impl Gesture {
    /// UIA returns screen-coordinate [left, top, width, height] quadruples.
    /// Empty/invalid geometry means unavailable, never invented text bounds.
    pub fn intersects_rectangles(self, rectangles: &[f64]) -> Option<bool> {
        if rectangles.is_empty() || rectangles.len() % 4 != 0 {
            return None;
        }
        let origin = self.origin.unwrap_or(self.end);
        let left = f64::from(origin.0.min(self.end.0)) - 16.0;
        let right = f64::from(origin.0.max(self.end.0)) + 16.0;
        let top = f64::from(origin.1.min(self.end.1)) - 16.0;
        let bottom = f64::from(origin.1.max(self.end.1)) + 16.0;
        let mut valid = false;
        for rect in rectangles.chunks_exact(4) {
            if !rect.iter().all(|number| number.is_finite()) || rect[2] <= 0.0 || rect[3] <= 0.0 {
                continue;
            }
            let rect_right = rect[0] + rect[2];
            let rect_bottom = rect[1] + rect[3];
            if !rect_right.is_finite() || !rect_bottom.is_finite() {
                continue;
            }
            valid = true;
            if rect[0] <= right && rect_right >= left && rect[1] <= bottom && rect_bottom >= top {
                return Some(true);
            }
        }
        valid.then_some(false)
    }
}

impl Bounds {
    fn valid(self) -> bool {
        self.right > self.left && self.bottom > self.top
    }

    fn hits(self, point: (i32, i32)) -> bool {
        let (x, y) = (i64::from(point.0), i64::from(point.1));
        x >= i64::from(self.left) - HIT_SLOP
            && x < i64::from(self.right) + HIT_SLOP
            && y >= i64::from(self.top) - HIT_SLOP
            && y < i64::from(self.bottom) + HIT_SLOP
    }
}

#[derive(Clone, Copy)]
pub(super) struct Metadata {
    pub bounds: Option<Bounds>,
    pub offscreen: bool,
    pub document: bool,
    pub edit: bool,
}

/// Smaller values are visited first. Embedded Documents precede generic pane
/// and glyph nodes, including when the provider returned them later as siblings.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct Priority(pub usize, pub usize, pub usize);

pub(super) fn priority(
    metadata: Metadata,
    point: Option<(i32, i32)>,
    depth: usize,
    unknown_bounds: usize,
) -> Option<(Priority, usize)> {
    if metadata.offscreen || depth > MAX_DEPTH {
        return None;
    }
    let bounds = metadata.bounds.filter(|bounds| bounds.valid());
    if let (Some(bounds), Some(point)) = (bounds, point) {
        if !bounds.hits(point) {
            return None;
        }
    }
    let unknown_bounds = if bounds.is_none() {
        unknown_bounds + 1
    } else {
        0
    };
    if unknown_bounds > MAX_UNKNOWN_BOUNDS {
        return None;
    }
    let kind = if metadata.document {
        0
    } else if metadata.edit {
        1
    } else {
        2
    };
    Some((
        Priority(usize::from(bounds.is_none()), kind, depth),
        unknown_bounds,
    ))
}

/// A separate node and pattern budget prevents a complex page or a provider that
/// repeats sibling nodes from using the entire capture on an unbounded walk.
pub(super) struct Budget {
    nodes: usize,
    patterns: usize,
}

impl Budget {
    pub fn new(nodes: usize, patterns: usize) -> Self {
        Self { nodes, patterns }
    }

    pub fn node(&mut self) -> bool {
        take(&mut self.nodes)
    }

    pub fn pattern(&mut self) -> bool {
        take(&mut self.patterns)
    }

    pub fn has_nodes(&self) -> bool {
        self.nodes > 0
    }
}

fn take(remaining: &mut usize) -> bool {
    if *remaining == 0 {
        false
    } else {
        *remaining -= 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visible() -> Metadata {
        Metadata {
            bounds: Some(Bounds {
                left: 0,
                top: 0,
                right: 400,
                bottom: 500,
            }),
            offscreen: false,
            document: false,
            edit: false,
        }
    }

    #[test]
    fn embedded_document_precedes_generic_children_regardless_of_sibling_order() {
        let pane = visible();
        let document = Metadata {
            document: true,
            ..pane
        };
        let edit = Metadata { edit: true, ..pane };
        let mut candidates = [
            priority(pane, Some((80, 80)), 1, 0).unwrap().0,
            priority(edit, Some((80, 80)), 1, 0).unwrap().0,
            priority(document, Some((80, 80)), 2, 0).unwrap().0,
        ];
        candidates.sort();
        assert_eq!(candidates[0], Priority(0, 0, 2));
        assert_eq!(candidates[1], Priority(0, 1, 1));
    }

    #[test]
    fn unrelated_toolbar_and_offscreen_pdf_pages_are_pruned() {
        let toolbar = Metadata {
            bounds: Some(Bounds {
                left: 0,
                top: 0,
                right: 400,
                bottom: 40,
            }),
            edit: true,
            ..visible()
        };
        assert!(priority(toolbar, Some((100, 200)), 1, 0).is_none());
        assert!(priority(
            Metadata {
                offscreen: true,
                ..visible()
            },
            Some((100, 200)),
            1,
            0
        )
        .is_none());
    }

    #[test]
    fn release_just_outside_text_region_still_reaches_the_provider() {
        assert!(priority(visible(), Some((406, 200)), 1, 0).is_some());
        assert!(priority(visible(), Some((430, 200)), 1, 0).is_none());
    }

    #[test]
    fn invalid_geometry_bridges_are_bounded_and_reset_by_a_visible_frame() {
        let unknown = Metadata {
            bounds: None,
            ..visible()
        };
        let (_, count) = priority(unknown, Some((80, 80)), 1, 0).unwrap();
        let (_, count) = priority(unknown, Some((80, 80)), 2, count).unwrap();
        assert!(priority(unknown, Some((80, 80)), 3, count).is_none());
        let (_, reset) = priority(visible(), Some((80, 80)), 3, count).unwrap();
        assert_eq!(reset, 0);
    }

    #[test]
    fn extreme_coordinates_do_not_overflow_hit_testing() {
        let metadata = Metadata {
            bounds: Some(Bounds {
                left: i32::MIN,
                top: i32::MIN,
                right: i32::MAX,
                bottom: i32::MAX,
            }),
            ..visible()
        };
        assert!(priority(metadata, Some((i32::MIN, i32::MAX)), 1, 0).is_some());
    }

    #[test]
    fn provider_cycles_have_finite_node_pattern_and_depth_limits() {
        let mut budget = Budget::new(3, 2);
        for _ in 0..3 {
            assert!(budget.node());
        }
        assert!(!budget.node());
        assert!(!budget.has_nodes());
        assert!(budget.pattern());
        assert!(budget.pattern());
        assert!(!budget.pattern());
        assert!(priority(visible(), None, MAX_DEPTH + 1, 0).is_none());
    }

    #[test]
    fn gesture_geometry_rejects_old_address_bar_selection_but_accepts_multiline_pdf() {
        let gesture = Gesture {
            origin: Some((100, 240)),
            end: (270, 290),
        };
        assert_eq!(
            gesture.intersects_rectangles(&[100.0, 25.0, 400.0, 20.0]),
            Some(false)
        );
        assert_eq!(
            gesture.intersects_rectangles(&[
                95.0, 230.0, 300.0, 18.0, 95.0, 254.0, 300.0, 18.0, 95.0, 278.0, 180.0, 18.0,
            ]),
            Some(true)
        );
    }

    #[test]
    fn gesture_geometry_handles_reverse_drags_and_single_point_clicks() {
        let reverse = Gesture {
            origin: Some((270, 290)),
            end: (100, 240),
        };
        assert_eq!(
            reverse.intersects_rectangles(&[130.0, 250.0, 100.0, 18.0]),
            Some(true)
        );
        let click = Gesture {
            origin: None,
            end: (155, 260),
        };
        assert_eq!(
            click.intersects_rectangles(&[130.0, 250.0, 100.0, 18.0]),
            Some(true)
        );
        assert_eq!(
            click.intersects_rectangles(&[300.0, 450.0, 100.0, 18.0]),
            Some(false)
        );
    }

    #[test]
    fn missing_or_broken_provider_rectangles_preserve_the_scoped_fallback() {
        let gesture = Gesture {
            origin: None,
            end: (100, 200),
        };
        assert_eq!(gesture.intersects_rectangles(&[]), None);
        assert_eq!(gesture.intersects_rectangles(&[0.0, 0.0, 0.0, 0.0]), None);
        assert_eq!(
            gesture.intersects_rectangles(&[f64::NAN, 0.0, 100.0, 20.0]),
            None
        );
        assert_eq!(gesture.intersects_rectangles(&[1.0]), None);
    }
}
