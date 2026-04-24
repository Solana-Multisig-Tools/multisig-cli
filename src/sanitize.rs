/// Sanitize a string for safe display by stripping:
/// - Bidi override characters (U+202A-U+202E, U+2066-U+2069)
/// - Zero-width characters (U+200B-U+200F, U+FEFF)
/// - Non-printable control characters (except newline, tab, carriage return)
pub fn sanitize_display(input: &str) -> String {
    input.chars().filter(|c| !is_dangerous_char(*c)).collect()
}

fn is_dangerous_char(c: char) -> bool {
    if ('\u{202A}'..='\u{202E}').contains(&c) {
        return true;
    }
    if ('\u{2066}'..='\u{2069}').contains(&c) {
        return true;
    }
    if ('\u{200B}'..='\u{200F}').contains(&c) {
        return true;
    }
    if c == '\u{FEFF}' {
        return true;
    }
    if c.is_control() && c != '\n' && c != '\t' && c != '\r' {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_normal_ascii() {
        let input = "Hello, World! 123 @#$%";
        assert_eq!(sanitize_display(input), input);
    }

    #[test]
    fn strips_bidi_overrides() {
        let input = "abc\u{202A}def\u{202E}ghi";
        assert_eq!(sanitize_display(input), "abcdefghi");
    }

    #[test]
    fn strips_bidi_isolates() {
        let input = "abc\u{2066}def\u{2069}ghi";
        assert_eq!(sanitize_display(input), "abcdefghi");
    }

    #[test]
    fn strips_zero_width() {
        let input = "abc\u{200B}def\u{200D}ghi\u{FEFF}jkl";
        assert_eq!(sanitize_display(input), "abcdefghijkl");
    }

    #[test]
    fn preserves_newlines_and_tabs() {
        let input = "line1\nline2\ttab\r\n";
        assert_eq!(sanitize_display(input), input);
    }

    #[test]
    fn strips_null_and_control() {
        let input = "abc\x00def\x01ghi";
        assert_eq!(sanitize_display(input), "abcdefghi");
    }
}
