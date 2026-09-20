//! Dedicated embedded MFD family; leaves every other egui font unchanged.
use bevy::prelude::*;
use bevy_egui::{EguiContext, EguiPreUpdateSet, egui};
const NAME: &str = "osg-mfd-iosevka-fixed";
const DATA: &[u8] = include_bytes!("../../data/fonts/IosevkaFixed-Regular.ttf");
pub fn family() -> egui::FontFamily {
    egui::FontFamily::Name(NAME.into())
}
/// Call before the first egui pass when using the painter without Bevy.
pub fn install(ctx: &egui::Context) {
    use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
    ctx.add_font(FontInsert::new(
        NAME,
        egui::FontData::from_static(DATA),
        vec![InsertFontFamily {
            family: family(),
            priority: FontPriority::Highest,
        }],
    ));
}
#[derive(Component)]
struct Installed;
pub struct MfdFontPlugin;
impl Plugin for MfdFontPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PreUpdate,
            install_context_fonts
                .after(EguiPreUpdateSet::InitContexts)
                .before(EguiPreUpdateSet::BeginPass),
        );
    }
}
fn install_context_fonts(
    mut commands: Commands,
    mut contexts: Query<(Entity, &mut EguiContext), Without<Installed>>,
) {
    for (entity, mut context) in &mut contexts {
        install(context.get_mut());
        commands.entity(entity).insert(Installed);
    }
}

/// egui 0.36.1's has_glyph compares font-face identity with the replacement
/// face, returning false for every glyph in a single-font family. Read our
/// embedded cmap directly so supported characters aren't replaced with '?'.
pub(super) fn has_glyph(ch: char) -> bool {
    use skrifa::MetadataProvider;
    static CHARMAP: std::sync::OnceLock<skrifa::charmap::Charmap<'static>> =
        std::sync::OnceLock::new();
    CHARMAP
        .get_or_init(|| {
            skrifa::FontRef::new(DATA)
                .expect("embedded Iosevka font")
                .charmap()
        })
        .map(ch)
        .is_some()
}
