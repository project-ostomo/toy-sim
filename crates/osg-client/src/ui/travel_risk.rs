use osg_ui::egui::Color32;

pub(super) fn color(loss_ppm: Option<f64>) -> Color32 {
    // Match the displayed precision at color boundaries.
    match loss_ppm.map(|loss| (loss * 100.0).round() / 100.0) {
        Some(loss) if loss <= 100.0 => Color32::from_rgb(107, 210, 165),
        Some(loss) if loss <= 1_000.0 => Color32::from_rgb(235, 211, 100),
        Some(loss) if loss <= 10_000.0 => Color32::from_rgb(240, 163, 80),
        Some(_) => Color32::from_rgb(242, 91, 91),
        None => Color32::from_rgb(139, 155, 171),
    }
}
