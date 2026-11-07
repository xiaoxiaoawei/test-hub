//! A selection must contain a visible character before it can open translation
//! UI or be sent to a service. Classification never rewrites a valid selection:
//! joiners and variation selectors still matter inside words and emoji.

pub fn has_visible_text(text: &str) -> bool {
    text.chars().any(|character| !selection_blank(character))
}

fn selection_blank(character: char) -> bool {
    character.is_whitespace()
        || character.is_control()
        || matches!(
            character,
            '\u{00ad}' // Soft hyphen without surrounding text.
                | '\u{034f}' // Combining grapheme joiner.
                | '\u{061c}' // Arabic letter mark.
                | '\u{115f}'..='\u{1160}' // Hangul fillers.
                | '\u{17b4}'..='\u{17b5}'
                | '\u{180b}'..='\u{180f}' // Mongolian variation selectors/separator.
                | '\u{200b}'..='\u{200f}' // Zero-width joiners and direction marks.
                | '\u{202a}'..='\u{202e}' // Bidirectional formatting controls.
                | '\u{2060}'..='\u{206f}' // Invisible separators/operators.
                | '\u{2800}' // Braille blank.
                | '\u{3164}' // Hangul filler.
                | '\u{fe00}'..='\u{fe0f}' // Variation selectors.
                | '\u{feff}' // BOM/zero-width no-break space.
                | '\u{ffa0}' // Half-width Hangul filler.
                | '\u{fff9}'..='\u{fffb}' // Interlinear annotation controls.
                | '\u{13430}'..='\u{1343f}' // Hieroglyph formatting controls.
                | '\u{1bca0}'..='\u{1bca3}' // Shorthand format controls.
                | '\u{1d173}'..='\u{1d17a}' // Musical formatting controls.
                | '\u{e0000}'..='\u{e0fff}' // Tags and supplemental variation selectors.
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_and_invisible_only_selections_are_not_content() {
        for text in [
            "",
            " \t\r\n\u{00a0}\u{2007}\u{202f}\u{3000}",
            "\u{200b}\u{200c}\u{200d}\u{2060}\u{feff}",
            "\u{061c}\u{200e}\u{200f}\u{202a}\u{202e}\u{2066}\u{2069}",
            "\u{00ad}\u{034f}\u{180e}\u{fe0f}\u{e0100}\u{e0067}",
            "\u{115f}\u{1160}\u{2800}\u{3164}\u{ffa0}",
            "\0\u{0007}\u{001b}",
        ] {
            assert!(!has_visible_text(text), "invisible selection: {text:?}");
        }
    }

    #[test]
    fn short_visible_words_numbers_and_punctuation_remain_valid() {
        for text in ["a", "中", "0", ".", "C++", "—", "é", "\u{0301}"] {
            assert!(has_visible_text(text), "visible selection: {text:?}");
        }
    }

    #[test]
    fn formatting_characters_inside_languages_and_emoji_are_not_removed() {
        for text in [
            "\u{feff}Hello\u{200b}",
            "می\u{200c}روم",
            "👩\u{200d}💻",
            "❤️",
            "A\n\u{00a0}B",
        ] {
            assert!(has_visible_text(text));
        }
    }
}
