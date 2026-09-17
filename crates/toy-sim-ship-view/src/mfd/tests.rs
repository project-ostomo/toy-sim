use super::*;
use toy_sim_model::drawing::{GREEN, WHITE};

fn blank(id: u8) -> ScreenImage {
    ScreenImage {
        screen_id: id,
        width: 512,
        height: 512,
        background: [0; 3],
        draws: Vec::new(),
        buttons: Default::default(),
    }
}

fn context(dpi: f32) -> egui::Context {
    let ctx = egui::Context::default();
    install_font(&ctx);
    ctx.set_pixels_per_point(dpi);
    ctx
}
struct Rendered {
    shapes: Vec<egui::epaint::ClippedShape>,
    pixels_per_point: f32,
    texture_updates: usize,
}
fn render(ctx: &egui::Context, size: f32, frame: &ScreenImage) -> Rendered {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::splat(size + 100.),
            )),
            ..Default::default()
        },
        |ui| {
            let painter = ui.painter().with_clip_rect(egui::Rect::EVERYTHING);
            assert!(paint(
                &painter,
                egui::Rect::from_min_size(egui::pos2(10., 20.), egui::Vec2::splat(size)),
                frame
            ));
        },
    );
    let texture_updates = output.textures_delta.set.len();
    output.textures_delta.clear();
    Rendered {
        shapes: output.shapes,
        pixels_per_point: output.pixels_per_point,
        texture_updates,
    }
}
fn shapes(output: &Rendered) -> impl Iterator<Item = &egui::Shape> {
    output
        .shapes
        .iter()
        .map(|s| &s.shape)
        .filter(|s| !matches!(s, egui::Shape::Noop))
}
#[test]
fn pixel_rectangle_endpoints_and_draw_order_preserve_logical_coordinates() {
    let ctx = context(1.);
    let mut c = blank(0);
    c.draws.push(Draw::Pixel {
        at: [511, 511],
        color: GREEN,
    });
    c.draws.push(Draw::Pixel {
        at: [-1, 0],
        color: GREEN,
    });
    c.draws.push(Draw::Rect {
        at: [-2, 10],
        size: [4, 2],
        filled: true,
        color: WHITE,
    });
    c.draws.push(Draw::Rect {
        at: [20, 20],
        size: [3, 3],
        filled: false,
        color: WHITE,
    });
    c.draws.push(Draw::Rect {
        at: [30, 30],
        size: [0, 3],
        filled: false,
        color: WHITE,
    });
    c.draws.push(Draw::Line {
        from: [5, 6],
        to: [9, 6],
        color: WHITE,
    });
    let output = render(&ctx, 512., &c);
    let draws: Vec<_> = shapes(&output).collect();
    let rectangles: Vec<_> = draws
        .iter()
        .filter_map(|s| {
            if let egui::Shape::Rect(r) = s {
                Some(r)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(rectangles[0].fill, egui::Color32::BLACK);
    assert!(
        rectangles
            .iter()
            .any(|r| r.rect.min == egui::pos2(521., 531.)
                && r.rect.size() == egui::Vec2::ONE
                && r.fill == rgb(GREEN))
    );
    assert!(rectangles.iter().any(|r| r.rect.min == egui::pos2(8., 30.)
        && r.rect.size() == egui::vec2(4., 2.)
        && r.fill == rgb(WHITE)));
    assert!(rectangles.iter().any(|r| r.rect.min == egui::pos2(30., 40.)
        && r.stroke_kind == egui::StrokeKind::Inside
        && r.stroke.width == 1.));
    assert!(draws.iter().any(|s| matches!(s, egui::Shape::LineSegment { points, .. } if *points == [egui::pos2(15.5,26.5),egui::pos2(19.5,26.5)])));
    for s in &output.shapes {
        if !matches!(s.shape, egui::Shape::Noop) {
            assert_eq!(
                s.clip_rect,
                egui::Rect::from_min_size(egui::pos2(10., 20.), egui::Vec2::splat(512.))
            );
        }
    }
}
#[test]
fn grid_spacing_newlines_glyphs_and_dpi_are_independent_of_font_advances() {
    for dpi in [1., 2.] {
        for size in [256., 512., 1024.] {
            let ctx = context(dpi);
            let mut c = blank(0);
            c.draws.push(Draw::Text {
                at: [2 * FONT_WIDTH, 3 * FONT_HEIGHT],
                text: "AA\nA\tA\u{10ffff}".into(),
                color: GREEN,
            });
            c.draws.push(Draw::Text {
                at: [0 * FONT_WIDTH, 10 * FONT_HEIGHT],
                text: "Ωμ─│┌█0123456789".into(),
                color: GREEN,
            });
            let output = render(&ctx, size, &c);
            let text: Vec<_> = shapes(&output)
                .filter_map(|s| {
                    if let egui::Shape::Text(t) = s {
                        Some(t)
                    } else {
                        None
                    }
                })
                .collect();
            let scale = size / 512.;
            assert!((text[1].pos.x - text[0].pos.x - 8. * scale).abs() < 1e-4);
            assert!((text[2].pos.y - text[0].pos.y - 16. * scale).abs() < 1e-4);
            assert!((text[3].pos.x - text[2].pos.x - 16. * scale).abs() < 1e-4);
            assert_eq!(text[0].galley.text(), "A");
            assert_eq!(text[4].galley.text(), "?");
            assert_eq!(text[5].galley.text(), "Ω");
            assert!(text.iter().all(|t| t.galley.pixels_per_point == dpi));
            assert!(text[0].galley.size().y <= 16. * scale + 0.02);
            assert!(text[0].galley.size().x <= 8. * scale + 0.02);
            let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
            assert!(!primitives.is_empty());
            for p in primitives {
                if let egui::epaint::Primitive::Mesh(m) = p.primitive {
                    assert!(m.is_valid());
                    assert!(
                        m.vertices
                            .iter()
                            .all(|v| v.pos.is_finite() && v.uv.is_finite())
                    );
                }
            }
        }
    }
}
#[test]
fn curves_are_bounded_and_offscreen_fill_still_covers_the_canvas() {
    let clip = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(512., 512.));
    for radii in [
        egui::vec2(20., 20.),
        egui::vec2(65535., 65535.),
        egui::vec2(65535., 1.),
    ] {
        let points = geometry::ellipse(egui::pos2(256., 256.), radii, clip, 2.);
        assert!((4..=2048).contains(&points.len()));
        assert!(points.iter().all(|p| p.is_finite()));
    }
    let mut c = blank(0);
    c.draws.push(Draw::Ellipse {
        centre: [256, 256],
        radii: [u16::MAX, u16::MAX],
        filled: true,
        color: GREEN,
    });
    c.draws.push(Draw::Ellipse {
        centre: [i16::MAX, i16::MIN],
        radii: [u16::MAX, u16::MAX],
        filled: false,
        color: GREEN,
    });
    c.draws.push(Draw::Ellipse {
        centre: [5, 6],
        radii: [0, 0],
        filled: false,
        color: GREEN,
    });
    c.draws.push(Draw::Ellipse {
        centre: [10, 10],
        radii: [0, 5],
        filled: false,
        color: GREEN,
    });
    c.draws.push(Draw::Line {
        from: [i16::MIN, i16::MIN],
        to: [i16::MAX, i16::MAX],
        color: WHITE,
    });
    c.draws.push(Draw::Polyline {
        points: vec![[10, 20], [30, 20], [30, 40]],
        color: GREEN,
    });
    let ctx = context(2.);
    let output = render(&ctx, 1024., &c);
    let paths: Vec<_> = shapes(&output)
        .filter_map(|s| {
            if let egui::Shape::Path(p) = s {
                Some(p)
            } else {
                None
            }
        })
        .collect();
    assert!(paths[0].fill == rgb(GREEN));
    // Every viewport corner lies inside the large filled convex contour.
    for p in [
        egui::pos2(10., 20.),
        egui::pos2(1034., 20.),
        egui::pos2(10., 1044.),
        egui::pos2(1034., 1044.),
    ] {
        let edges = paths[0]
            .points
            .iter()
            .zip(paths[0].points.iter().cycle().skip(1))
            .take(paths[0].points.len());
        assert!(
            edges
                .map(|(&a, &b)| (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x))
                .all(|cross| cross >= 0.)
        );
    }
    let meshes = ctx.tessellate(output.shapes, output.pixels_per_point);
    let vertices: usize = meshes
        .iter()
        .map(|m| {
            if let egui::epaint::Primitive::Mesh(m) = &m.primitive {
                m.vertices.len()
            } else {
                0
            }
        })
        .sum();
    assert!(vertices < 40000);
}
#[test]
fn maximum_valid_work_is_bounded_and_invalid_frames_draw_nothing() {
    let ctx = context(2.);
    let mut c = blank(0);
    for _ in 0..256 {
        c.draws.push(Draw::Ellipse {
            centre: [i16::MAX, i16::MIN],
            radii: [u16::MAX, u16::MAX],
            filled: false,
            color: GREEN,
        });
    }
    let frame = c;
    assert!(frame.valid());
    let output = render(&ctx, 1024., &frame);
    assert!(shapes(&output).count() <= 257);
    let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
    assert!(
        primitives
            .iter()
            .all(|p| matches!(&p.primitive,egui::epaint::Primitive::Mesh(m) if m.is_valid()))
    );
    let mut invalid = blank(0);
    for _ in 0..5 {
        invalid.draws.push(Draw::Rect {
            at: [0, 0],
            size: [512, 512],
            filled: true,
            color: WHITE,
        });
    }
    let invalid = invalid;
    let mut output = ctx.run_ui(Default::default(), |ui| {
        assert!(!paint(ui.painter(), ui.max_rect(), &invalid));
    });
    output.textures_delta.clear();
}
#[test]
fn replacing_clearing_and_resizing_need_no_framebuffer_texture() {
    let ctx = context(1.);
    let mut c = blank(0);
    c.draws.push(Draw::Text {
        at: [0 * FONT_WIDTH, 0 * FONT_HEIGHT],
        text: "Hello".into(),
        color: GREEN,
    });
    c.draws.push(Draw::Ellipse {
        centre: [256, 256],
        radii: [100, 100],
        filled: false,
        color: GREEN,
    });
    let c = c;
    render(&ctx, 512., &c);
    let again = render(&ctx, 512., &c);
    assert!(
        again.texture_updates == 0,
        "unchanged frame re-uploaded an image"
    );
    let resized = render(&ctx, 1024., &c);
    assert!(shapes(&resized).any(|s| matches!(s, egui::Shape::Path(_))));
    let blank = render(&ctx, 512., &blank(0));
    assert_eq!(shapes(&blank).count(), 1);
    assert!(blank.texture_updates == 0);
}
fn click(pos: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        },
    ]
}
#[test]
fn bezel_clicks_and_canvas_focus_keep_the_same_input_contract() {
    let ctx = context(1.);
    let mut renderer = MfdRenderer::default();
    let mut c = blank(1);
    c.buttons[BezelKey::L1.index()] = Some("HLD".into());
    let c = c;
    let mut frame = |events| {
        let mut result = MfdResponse::default();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640., 600.),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                result = renderer.show(ui, 1, Some(&c));
            },
        );
        output.textures_delta.clear();
        result
    };
    frame(vec![]);
    frame(click(egui::pos2(21., 45.), true));
    let bezel = frame(click(egui::pos2(21., 45.), false));
    assert_eq!(bezel.keys, vec![BezelKey::L1]);
    assert!(bezel.focused);
    frame(click(egui::pos2(320., 320.), true));
    let screen = frame(click(egui::pos2(320., 320.), false));
    assert!(screen.keys.is_empty());
    assert!(screen.focused);
    assert!(!ctx.egui_wants_keyboard_input());
}
