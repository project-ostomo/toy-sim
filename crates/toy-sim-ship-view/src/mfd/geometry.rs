use bevy_egui::egui::{Pos2, Rect, Vec2};

/// Liang–Barsky clipping bounds coordinates before egui tessellates a stroke.
pub(super) fn clip_line(from: Pos2, to: Pos2, clip: Rect) -> Option<[Pos2; 2]> {
    let delta = to - from;
    let mut start = 0f32;
    let mut end = 1f32;
    for (p, q) in [
        (-delta.x, from.x - clip.left()),
        (delta.x, clip.right() - from.x),
        (-delta.y, from.y - clip.top()),
        (delta.y, clip.bottom() - from.y),
    ] {
        if p == 0. {
            if q < 0. {
                return None;
            }
        } else {
            let ratio = q / p;
            if p < 0. {
                start = start.max(ratio);
            } else {
                end = end.min(ratio);
            }
            if start > end {
                return None;
            }
        }
    }
    Some([from + delta * start, from + delta * end])
}

/// Quarter-arc bounds are monotone. Offscreen arcs become a single chord whose
/// bounding box is also offscreen, preserving filled-polygon coverage in the clip.
/// Four budgets of 512 edges put a hard bound on work, even at extreme DPI/radii.
pub(super) fn ellipse(centre: Pos2, radii: Vec2, clip: Rect, pixels_per_point: f32) -> Vec<Pos2> {
    struct ArcBuilder {
        centre: Pos2,
        radii: Vec2,
        clip: Rect,
        tolerance: f32,
        points: Vec<Pos2>,
    }
    impl ArcBuilder {
        fn point(&self, t: f64) -> Pos2 {
            self.centre + Vec2::new(t.cos() as f32 * self.radii.x, t.sin() as f32 * self.radii.y)
        }
        fn arc(&mut self, from: f64, to: f64, a: Pos2, b: Pos2, depth: u8, budget: usize) {
            let middle = (from + to) * 0.5;
            let m = self.point(middle);
            if depth >= 16
                || budget <= 1
                || !Rect::from_two_pos(a, b).intersects(self.clip)
                || m.distance(a.lerp(b, 0.5)) <= self.tolerance
            {
                self.points.push(b);
            } else {
                self.arc(from, middle, a, m, depth + 1, budget / 2);
                self.arc(middle, to, m, b, depth + 1, budget / 2);
            }
        }
    }
    let mut builder = ArcBuilder {
        centre,
        radii,
        clip,
        tolerance: 0.25 / pixels_per_point.max(0.01),
        points: Vec::new(),
    };
    builder.points.push(builder.point(0.));
    for quarter in 0..4 {
        let a = quarter as f64 * std::f64::consts::FRAC_PI_2;
        let b = a + std::f64::consts::FRAC_PI_2;
        builder.arc(a, b, builder.point(a), builder.point(b), 0, 512);
    }
    builder.points.pop(); // closing vertex is implicit in PathShape
    builder.points
}
