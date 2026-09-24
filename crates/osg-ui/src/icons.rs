use crate::egui::{self, FontFamily, FontId};

#[derive(Clone, Copy, Debug)]
pub enum Icon {
    Cargo,
    Overview,
    Ship,
    Industry,
    Navigation,
    Planet,
    Clock,
    Settings,
    Look,
    Lock,
    Reset,
    Align,
    Approach,
    KeepRange,
    Stop,
    Shield,
    Power,
    Broadcast,
    Pause,
    Play,
    Target,
    Layout,
    Info,
    Flag,
    User,
    Handshake,
}

impl Icon {
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Cargo => "\u{e1da}",
            Self::Overview => "\u{e2f2}",
            Self::Ship => "\u{e3fc}",
            Self::Industry => "\u{e760}",
            Self::Navigation => "\u{e1c8}",
            Self::Planet => "\u{e652}",
            Self::Clock => "\u{e19a}",
            Self::Settings => "\u{e270}",
            Self::Look => "\u{e220}",
            Self::Lock => "\u{e308}",
            Self::Reset => "\u{e096}",
            Self::Align => "\u{eade}",
            Self::Approach => "\u{e06c}",
            Self::KeepRange => "\u{e0a2}",
            Self::Stop => "\u{e57e}",
            Self::Shield => "\u{e40a}",
            Self::Power => "\u{e2de}",
            Self::Broadcast => "\u{e0f2}",
            Self::Pause => "\u{e39e}",
            Self::Play => "\u{e3d0}",
            Self::Target => "\u{e47c}",
            Self::Layout => "\u{e464}",
            Self::Info => "\u{e2ce}",
            Self::Flag => "\u{e244}",
            Self::User => "\u{e4c2}",
            Self::Handshake => "\u{e582}",
        }
    }

    pub fn font(size: f32) -> FontId {
        FontId::new(size, FontFamily::Name("Phosphor".into()))
    }

    pub fn text(self, size: f32) -> egui::RichText {
        egui::RichText::new(self.glyph()).font(Self::font(size))
    }
}

pub fn install(ctx: &egui::Context) {
    use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};

    ctx.add_font(FontInsert::new(
        "Phosphor",
        egui::FontData::from_static(include_bytes!("../data/fonts/Phosphor.ttf")),
        vec![InsertFontFamily {
            family: FontFamily::Name("Phosphor".into()),
            priority: FontPriority::Highest,
        }],
    ));
}
