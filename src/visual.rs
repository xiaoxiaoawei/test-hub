//! Platform-independent color families. Values use RGB24 (0xRRGGBB), never
//! Win32 COLORREF, so contrast and configuration can be checked on every target.
use crate::config::{ThemeMode, ThemePreset};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Palette {
    pub background: u32,
    pub surface: u32,
    pub card: u32,
    pub raised: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    pub border: u32,
    pub accent_active: u32,
    pub on_accent: u32,
    pub selection: u32,
    pub selection_text: u32,
}

impl Palette {
    pub const fn for_theme(mode: ThemeMode, preset: ThemePreset) -> Self {
        match (mode, preset) {
            (ThemeMode::Dark, ThemePreset::Ocean) => Self {
                background: 0x0A1222,
                surface: 0x0C172A,
                card: 0x101E34,
                raised: 0x1B2E49,
                text: 0xEEF6FF,
                muted: 0xA5B8D2,
                accent: 0x79B9FF,
                border: 0x2C4260,
                accent_active: 0x5EA5F3,
                on_accent: 0x07162B,
                selection: 0x264D7E,
                selection_text: 0xF2F7FF,
            },
            (ThemeMode::Light, ThemePreset::Ocean) => Self {
                background: 0xEDF4FF,
                surface: 0xF5F8FF,
                card: 0xFFFFFF,
                raised: 0xE2EDFF,
                text: 0x142746,
                muted: 0x52657D,
                accent: 0x245FD6,
                border: 0xB9CCE5,
                accent_active: 0x194BAE,
                on_accent: 0xFFFFFF,
                selection: 0xD8E7FF,
                selection_text: 0x163D7A,
            },
            (ThemeMode::Dark, ThemePreset::Aurora) => Self {
                background: 0x081919,
                surface: 0x0A2021,
                card: 0x0E2828,
                raised: 0x173939,
                text: 0xE9FFFA,
                muted: 0x9DBEB7,
                accent: 0x5CD9C0,
                border: 0x2B5250,
                accent_active: 0x3BBDA8,
                on_accent: 0x06251F,
                selection: 0x1B5E52,
                selection_text: 0xEDFFF8,
            },
            (ThemeMode::Light, ThemePreset::Aurora) => Self {
                background: 0xEDF9F5,
                surface: 0xF4FCF9,
                card: 0xFFFFFF,
                raised: 0xD8EFE6,
                text: 0x153A33,
                muted: 0x4E6E65,
                accent: 0x0B7769,
                border: 0xAFD4C7,
                accent_active: 0x075B50,
                on_accent: 0xFFFFFF,
                selection: 0xC8EADF,
                selection_text: 0x144E40,
            },
            (ThemeMode::Dark, ThemePreset::Nebula) => Self {
                background: 0x151126,
                surface: 0x19142D,
                card: 0x211B36,
                raised: 0x302647,
                text: 0xF5EFFF,
                muted: 0xB9ABC9,
                accent: 0xBDA5FF,
                border: 0x504065,
                accent_active: 0xA58BEA,
                on_accent: 0x201335,
                selection: 0x523B78,
                selection_text: 0xFCF7FF,
            },
            (ThemeMode::Light, ThemePreset::Nebula) => Self {
                background: 0xF5F0FC,
                surface: 0xFBF8FF,
                card: 0xFFFFFF,
                raised: 0xEBE1FA,
                text: 0x322149,
                muted: 0x6F5B82,
                accent: 0x744AC5,
                border: 0xCFBDDF,
                accent_active: 0x5C369F,
                on_accent: 0xFFFFFF,
                selection: 0xE8DBFA,
                selection_text: 0x44226C,
            },
            (ThemeMode::Dark, ThemePreset::Amber) => Self {
                background: 0x211D1B,
                surface: 0x302925,
                card: 0x2A2421,
                raised: 0x40342C,
                text: 0xF1E5D8,
                muted: 0xBBAA99,
                accent: 0xD6A575,
                border: 0x59493E,
                accent_active: 0xBD8E62,
                on_accent: 0x211D1B,
                selection: 0x674B36,
                selection_text: 0xFFF2E3,
            },
            (ThemeMode::Light, ThemePreset::Amber) => Self {
                background: 0xF4EAD8,
                surface: 0xFFF8EA,
                card: 0xFBF3E6,
                raised: 0xE6D6BC,
                text: 0x33271E,
                muted: 0x75624D,
                accent: 0x8D5A39,
                border: 0xA18A6D,
                accent_active: 0x734329,
                on_accent: 0xFFF8EA,
                selection: 0xE5D1B8,
                selection_text: 0x4E321F,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::THEME_PRESETS;
    use std::collections::HashSet;

    fn luminance(rgb: u32) -> f64 {
        let component = |shift: u32| {
            let value = ((rgb >> shift) & 255u32) as f64 / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * component(16) + 0.7152 * component(8) + 0.0722 * component(0)
    }

    fn contrast(first: u32, second: u32) -> f64 {
        let first = luminance(first);
        let second = luminance(second);
        (first.max(second) + 0.05) / (first.min(second) + 0.05)
    }

    #[test]
    fn every_template_keeps_normal_secondary_button_and_selection_text_readable() {
        for &preset in THEME_PRESETS {
            for mode in [ThemeMode::Dark, ThemeMode::Light] {
                let palette = Palette::for_theme(mode, preset);
                for surface in [palette.background, palette.surface, palette.card] {
                    for text in [palette.text, palette.muted] {
                        assert!(
                            contrast(text, surface) >= 4.5,
                            "{preset:?} / {mode:?}: text #{text:06X} on #{surface:06X}"
                        );
                    }
                    assert!(
                        contrast(palette.accent, surface) >= 3.0,
                        "{preset:?} / {mode:?}: focus indicator is hard to distinguish"
                    );
                }
                assert!(contrast(palette.text, palette.raised) >= 4.5);
                assert!(contrast(palette.on_accent, palette.accent) >= 4.5);
                assert!(contrast(palette.on_accent, palette.accent_active) >= 4.5);
                assert!(contrast(palette.selection_text, palette.selection) >= 4.5);
            }
        }
    }

    #[test]
    fn families_remain_distinct_and_modes_adapt_surfaces_without_alpha_values() {
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let mut accents = HashSet::new();
            for &preset in THEME_PRESETS {
                let colors = Palette::for_theme(mode, preset);
                assert!(accents.insert(colors.accent), "duplicate family accent");
                for color in [
                    colors.background,
                    colors.surface,
                    colors.card,
                    colors.raised,
                    colors.text,
                    colors.muted,
                    colors.accent,
                    colors.border,
                    colors.accent_active,
                    colors.on_accent,
                    colors.selection,
                    colors.selection_text,
                ] {
                    assert!(color <= 0xFFFFFF, "not an opaque RGB24 color");
                }
                let background = luminance(colors.background);
                match mode {
                    ThemeMode::Dark => assert!(background < 0.05),
                    ThemeMode::Light => assert!(background > 0.75),
                }
            }
            assert_eq!(accents.len(), THEME_PRESETS.len());
        }
        for &preset in THEME_PRESETS {
            let dark = Palette::for_theme(ThemeMode::Dark, preset);
            let light = Palette::for_theme(ThemeMode::Light, preset);
            assert_ne!(dark.background, light.background);
            assert_ne!(dark.accent, light.accent);
        }
    }
}
