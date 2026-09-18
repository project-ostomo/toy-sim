//! Clip exact homogeneous conics, fit screen-space cubics, let egui flatten them.
use super::{
    geometry::RationalArc,
    projection::{View, occluded},
};
use bevy::math::{DVec2, DVec3, DVec4};
use toy_sim_ui::egui;

const MAX_CUBICS: usize = 1024;
const MAX_VISITS: usize = 8192;

pub(super) struct ProjectedArc {
    pub points: [egui::Pos2; 4],
    pub hidden: bool,
}

fn roots(values: [f64; 3], out: &mut Vec<f64>) {
    let scale = values.iter().copied().map(f64::abs).fold(0., f64::max);
    if scale == 0. || !scale.is_finite() {
        return;
    }
    let [a, b, c] = values.map(|v| v / scale);
    let (qa, qb, qc) = (a - 2. * b + c, 2. * (b - a), a);
    let mut add = |t: f64| {
        if t > 0. && t < 1. {
            out.push(t);
        }
    };
    if qa.abs() < 1e-15 {
        if qb != 0. {
            add(-qc / qb);
        }
    } else {
        let d = qb * qb - 4. * qa * qc;
        if d >= 0. {
            let q = -0.5 * (qb + d.sqrt().copysign(qb));
            if q == 0. {
                add(-qb / (2. * qa));
            } else {
                add(q / qa);
                add(qc / q);
            }
        }
    }
}

fn camera(view: &View, arc: RationalArc) -> RationalArc {
    RationalArc(
        arc.0
            .map(|h| (view.rotation * (h.truncate() - view.eye * h.w)).extend(h.w)),
    )
}

fn planes(view: &View, h: DVec4) -> [f64; 5] {
    // A small guard band retains antialiased strokes and controls near viewport edges.
    let x = 1. + 8. / view.rect.width() as f64;
    let y = 1. + 8. / view.rect.height() as f64;
    [
        -h.z - 0.1 * h.w,
        -h.z * x + view.sx * h.x,
        -h.z * x - view.sx * h.x,
        -h.z * y + view.sy * h.y,
        -h.z * y - view.sy * h.y,
    ]
}

/// Split at exact intersections with all five clip planes. Unlike endpoint tests,
/// this retains arcs which enter the viewport with both endpoints outside it.
fn visible_intervals(view: &View, arc: RationalArc) -> Vec<(f64, f64)> {
    let values = arc.0.map(|p| planes(view, p));
    let mut times = vec![0., 1.];
    for i in 0..5 {
        roots(values.map(|v| v[i]), &mut times);
    }
    times.sort_by(f64::total_cmp);
    times.dedup_by(|a, b| (*a - *b).abs() < 1e-14);
    times
        .windows(2)
        .filter_map(|w| {
            let p = arc.point((w[0] + w[1]) * 0.5);
            planes(view, p.extend(1.))
                .iter()
                .all(|v| *v >= 0.)
                .then_some((w[0], w[1]))
        })
        .collect()
}

fn projected_controls(view: &View, arc: RationalArc) -> [DVec3; 3] {
    let center = view.rect.center();
    let controls = arc.0.map(|h| {
        DVec3::new(
            -h.z * center.x as f64 + h.x * view.sx * view.rect.width() as f64 * 0.5,
            -h.z * center.y as f64 - h.y * view.sy * view.rect.height() as f64 * 0.5,
            -h.z,
        )
    });
    let scale = controls.iter().map(|h| h.z.abs()).fold(0., f64::max);
    controls.map(|h| h / scale)
}

/// Endpoint/tangent-preserving cubic fit with a conservative Bernstein error bound.
/// E(t) = N(t) - W(t) C(t) is degree five. Its control-vector hull bounds E,
/// and positive minimum weight bounds E/W over the entire interval, not just samples.
fn cubic(h: [DVec3; 3], tolerance: f64) -> Option<[DVec2; 4]> {
    let min_weight = h.iter().map(|h| h.z).fold(f64::INFINITY, f64::min);
    if min_weight <= 0. || !min_weight.is_finite() {
        return None;
    }
    let p = h.map(|h| h.truncate() / h.z);
    let c = [
        p[0],
        p[0] + (p[1] - p[0]) * (2. * h[1].z / (3. * h[0].z)),
        p[2] + (p[1] - p[2]) * (2. * h[1].z / (3. * h[2].z)),
        p[2],
    ];
    if !c.iter().all(|p| p.is_finite()) {
        return None;
    }
    let mut error = [DVec2::ZERO; 6];
    for i in 0..3 {
        for j in 0..4 {
            let weight = [1., 2., 1.][i] * [1., 3., 3., 1.][j] / [1., 5., 10., 10., 5., 1.][i + j];
            error[i + j] += (p[i] - c[j]) * (h[i].z * weight);
        }
    }
    (error.iter().map(|e| e.length()).fold(0., f64::max) <= tolerance * min_weight).then_some(c)
}

pub(super) fn project(
    view: &View,
    arcs: &[RationalArc],
    pixels_per_point: f32,
    bodies: &[(DVec3, f64)],
) -> Vec<ProjectedArc> {
    struct Fit<'a> {
        view: &'a View,
        tolerance: f64,
        bodies: &'a [(DVec3, f64)],
        visits: usize,
        out: Vec<ProjectedArc>,
    }
    impl Fit<'_> {
        fn visit(&mut self, world: RationalArc, cam: RationalArc, depth: u32) {
            if self.visits >= MAX_VISITS || self.out.len() >= MAX_CUBICS {
                return;
            }
            self.visits += 1;
            let controls = projected_controls(self.view, cam);
            let fit = cubic(controls, self.tolerance);
            let hidden = [0., 0.25, 0.5, 0.75, 1.]
                .map(|t| occluded(self.view.eye, world.point(t), self.bodies));
            let same_visibility = hidden.iter().all(|v| *v == hidden[2]);
            // Permit subpixel occlusion transitions, otherwise locate the silhouette.
            let short = controls.iter().all(|h| h.z > 0.) && {
                let p = controls.map(|h| h.truncate() / h.z);
                p[0].distance(p[1]) + p[1].distance(p[2]) < self.tolerance
            };
            if let Some(c) = fit.filter(|_| same_visibility || short) {
                // Never hand egui huge offscreen f32 control coordinates.
                let safe = self.view.rect.expand(self.view.rect.size().max_elem());
                if c.iter()
                    .all(|p| safe.contains(egui::pos2(p.x as f32, p.y as f32)))
                {
                    self.out.push(ProjectedArc {
                        points: c.map(|p| egui::pos2(p.x as f32, p.y as f32)),
                        hidden: hidden[2],
                    });
                    return;
                }
            }
            if depth == 32 {
                return;
            }
            let (wa, wb) = world.split(0.5);
            let (ca, cb) = cam.split(0.5);
            self.visit(wa, ca, depth + 1);
            self.visit(wb, cb, depth + 1);
        }
    }
    let mut fit = Fit {
        view,
        tolerance: 0.25 / pixels_per_point.max(0.1) as f64,
        bodies,
        visits: 0,
        out: vec![],
    };
    for &world in arcs {
        let cam = camera(view, world);
        for (a, b) in visible_intervals(view, cam) {
            fit.visit(world.interval(a, b), cam.interval(a, b), 0);
        }
    }
    fit.out
}

#[cfg(test)]
mod tests {
    use super::super::{conic::Conic, geometry::AnalyticCurve};
    use super::*;
    use bevy::math::DQuat;

    fn view(eye: DVec3, rotation: DQuat) -> View {
        View {
            rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600., 1000.)),
            eye,
            rotation,
            sx: 1.,
            sy: 1.6,
        }
    }
    fn flattened(arcs: &[ProjectedArc], ppp: f32) -> Vec<[egui::Pos2; 2]> {
        arcs.iter()
            .flat_map(|a| {
                egui::epaint::CubicBezierShape::from_points_stroke(
                    a.points,
                    false,
                    egui::Color32::TRANSPARENT,
                    egui::Stroke::new(1., egui::Color32::WHITE),
                )
                .flatten(Some(0.25 / ppp))
                .windows(2)
                .map(|s| [s[0], s[1]])
                .collect::<Vec<_>>()
            })
            .collect()
    }
    fn line_distance(p: egui::Pos2, line: [egui::Pos2; 2]) -> f32 {
        let d = line[1] - line[0];
        let t = ((p - line[0]).dot(d) / d.length_sq().max(1e-20)).clamp(0., 1.);
        p.distance(line[0] + t * d)
    }
    fn check_error(view: &View, arcs: &[RationalArc], ppp: f32) -> usize {
        let cubics = project(view, arcs, ppp, &[]);
        assert!(!cubics.is_empty());
        assert!(cubics.len() < MAX_CUBICS, "exhausted visible-curve budget");
        let lines = flattened(&cubics, ppp);
        assert!(!lines.is_empty());
        for &arc in arcs {
            let cam = camera(view, arc);
            for (lo, hi) in visible_intervals(view, cam) {
                // Dense independent samples from the exact curve, including clipping endpoints.
                for i in 0..=128 {
                    let point = view.screen(cam.point(lo + (hi - lo) * i as f64 / 128.));
                    let error = lines
                        .iter()
                        .map(|l| line_distance(point, *l))
                        .fold(f32::INFINITY, f32::min)
                        * ppp;
                    assert!(
                        error <= 0.55,
                        "screen error {error} px, point {point:?}, cubics {}",
                        cubics.len()
                    );
                }
            }
        }
        cubics.len()
    }

    #[test]
    fn planet_scale_orbits_stay_subpixel_from_ship_distance_to_map_view() {
        let curve = AnalyticCurve {
            conic: Conic {
                r: DVec3::X * 4e7,
                v: DVec3::Y * (4e14_f64 / 4e7).sqrt(),
                mu: 4e14,
                radius: 1.,
            },
            offset: DVec3::ZERO,
        };
        let arcs = curve.arcs(0., curve.conic.period().unwrap()).unwrap();
        for distance in [1., 10., 100., 1e4, 1e6, 1e8] {
            for ppp in [1., 2., 4.] {
                for tilt in [0., 0.7, std::f64::consts::FRAC_PI_2] {
                    let rotation = DQuat::from_rotation_y(tilt);
                    let eye = curve.conic.r + rotation.inverse() * DVec3::Z * distance;
                    let v = view(eye, rotation);
                    check_error(&v, &arcs, ppp);
                    let ship = v.point(curve.conic.r).unwrap();
                    let lines = flattened(&project(&v, &arcs, ppp, &[]), ppp);
                    assert!(
                        lines
                            .iter()
                            .map(|l| line_distance(ship, *l))
                            .fold(f32::INFINITY, f32::min)
                            * ppp
                            < 0.1
                    );
                }
            }
        }
    }

    #[test]
    fn clips_curved_entries_and_near_plane_crossings_before_float_conversion() {
        let v = view(DVec3::ZERO, DQuat::IDENTITY);
        // Both endpoints lie beyond the same viewport edge, but the middle enters it.
        let entering = RationalArc([
            DVec3::new(30., -10., -10.).extend(1.),
            DVec3::new(-40., 0., -10.).extend(1.),
            DVec3::new(30., 10., -10.).extend(1.),
        ]);
        assert!(v.segment(entering.point(0.), entering.point(1.)).is_none());
        check_error(&v, &[entering], 2.);
        let crossing = RationalArc([
            DVec3::new(-20., 0., 5.).extend(1.),
            DVec3::new(0., 20., -40.).extend(1.),
            DVec3::new(20., 0., 5.).extend(1.),
        ]);
        check_error(&v, &[crossing], 2.);
        let behind = RationalArc(
            crossing
                .0
                .map(|h| DVec4::new(h.x, h.y, h.z.abs() + 1., h.w)),
        );
        assert!(project(&v, &[behind], 1., &[]).is_empty());
    }

    #[test]
    fn eccentric_and_escape_arcs_keep_their_shape_under_perspective() {
        for e in [0.95, 0.9999, 1., 1.5] {
            let curve = AnalyticCurve {
                conic: Conic {
                    r: DVec3::X * 4e7,
                    v: DVec3::Y * (4e14 * (1. + e) / 4e7_f64).sqrt(),
                    mu: 4e14,
                    radius: 1.,
                },
                offset: DVec3::ZERO,
            };
            let end = curve.conic.period().unwrap_or(1e5);
            let arcs = curve.arcs(0., end).unwrap();
            for distance in [10., 1e8] {
                let v = view(
                    curve.conic.r + DVec3::Z * distance,
                    DQuat::from_rotation_y(0.2),
                );
                check_error(&v, &arcs, 2.);
            }
        }
    }

    #[test]
    fn occlusion_splits_keep_visible_and_hidden_arcs() {
        let curve = AnalyticCurve {
            conic: Conic {
                r: DVec3::X * 10.,
                v: DVec3::Z * 10.,
                mu: 1000.,
                radius: 1.,
            },
            offset: DVec3::ZERO,
        };
        let arcs = curve.arcs(0., curve.conic.period().unwrap()).unwrap();
        let v = view(DVec3::new(0., 3., 20.), DQuat::IDENTITY);
        let cubics = project(&v, &arcs, 1., &[(DVec3::ZERO, 5.)]);
        assert!(cubics.iter().any(|c| c.hidden));
        assert!(cubics.iter().any(|c| !c.hidden));
        assert!(cubics.len() < MAX_CUBICS);
    }
}
