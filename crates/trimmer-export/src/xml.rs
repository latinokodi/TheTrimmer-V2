//! Text encoding for documents: XML escaping, forbidden characters, and URL paths.
//!
//! The three live together because they are the same job seen from three angles — turning a
//! string a person typed into a string a document can carry — and because getting one of
//! them subtly wrong is how a document that looks fine fails to import.

use std::borrow::Cow;

/// Characters XML 1.0 cannot represent at all.
///
/// This is not a matter of escaping. `&#1;` is not a legal character reference, so a C0
/// control character has no spelling in XML; it can only be dropped. Tab, newline and
/// carriage return are the three exceptions the specification carves out.
fn is_forbidden(character: char) -> bool {
    character.is_control() && !matches!(character, '\t' | '\n' | '\r')
}

/// True when a string holds a character XML cannot carry.
pub(crate) fn has_forbidden(text: &str) -> bool {
    text.chars().any(is_forbidden)
}

/// Remove the characters XML cannot carry.
///
/// The second half of the pair says whether anything was removed, so a caller can warn about
/// it rather than silently altering what the user typed.
pub(crate) fn strip_forbidden(text: &str) -> (Cow<'_, str>, bool) {
    if !has_forbidden(text) {
        return (Cow::Borrowed(text), false);
    }
    let cleaned: String = text
        .chars()
        .filter(|character| !is_forbidden(*character))
        .collect();
    (Cow::Owned(cleaned), true)
}

/// Escape `&`, `<`, `>`, `"` and `'` for XML.
///
/// All five, everywhere, including in character data. The alternative — escaping an
/// apostrophe only inside an attribute — makes correctness depend on where the text landed,
/// which is precisely the review question nobody catches.
pub(crate) fn escape(text: &str) -> Cow<'_, str> {
    quick_xml::escape::escape(text)
}

/// Text ready to be written between XML tags: cleaned, then escaped.
///
/// `what` names the source of the text for the warning, so a report can say *which* name had
/// a stray control character in it rather than only that one did.
pub(crate) fn clean(raw: &str, what: &str, warnings: &mut Vec<String>) -> String {
    if has_forbidden(raw) {
        warnings.push(format!(
            "{what} contained control characters, which XML cannot represent; they were \
             removed, because XML has no way to spell them"
        ));
    }
    escape(&strip_forbidden(raw).0).into_owned()
}

/// The characters a URL path may carry literally.
///
/// RFC 3986's unreserved set, plus `/` because a path is a sequence of segments, and `:`
/// because a Windows drive letter is the single most common thing in one of these paths and
/// every tool that reads them accepts it unescaped.
fn is_url_safe(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/' | b':')
}

/// Percent-encode a filesystem path for a URL, with `/` separators and uppercase hex.
///
/// A backslash is a separator here, not something to escape: `H:\work\a b.mp4` and
/// `H:/work/a b.mp4` name the same file, and only one of them is a URL path.
pub(crate) fn url_path(path: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    let mut out = String::with_capacity(path.len() + 8);
    for &byte in path.as_bytes() {
        if byte == b'\\' {
            out.push('/');
        } else if is_url_safe(byte) {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 0x0F)]));
        }
    }
    out
}

/// The `<pathurl>` form FCP7 writes: an explicit `localhost` authority.
pub(crate) fn premiere_url(path: &str) -> String {
    let encoded = url_path(path);
    format!("file://localhost/{}", encoded.trim_start_matches('/'))
}

/// The `src` form FCPXML writes: an empty authority, so three slashes.
pub(crate) fn fcpxml_url(path: &str) -> String {
    let encoded = url_path(path);
    format!("file:///{}", encoded.trim_start_matches('/'))
}

/// Append one indented line to a document.
///
/// Writing into a `String` cannot fail, so there is nothing to report; using `writeln!` at
/// every one of the several hundred line sites in this crate would mean an `expect` at every
/// one of them, which is noise that hides the lines that genuinely can fail.
pub(crate) fn line(out: &mut String, indent: usize, text: &str) {
    for _ in 0..indent {
        out.push_str("  ");
    }
    out.push_str(text);
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_xml_special_character_is_escaped() {
        assert_eq!(
            escape("a & b < c > d \" e ' f"),
            "a &amp; b &lt; c &gt; d &quot; e &apos; f"
        );
        assert_eq!(escape("nothing to do"), "nothing to do");
    }

    #[test]
    fn a_control_character_is_stripped_and_reported_but_whitespace_is_kept() {
        let (cleaned, changed) = strip_forbidden("cold\u{1} open\u{7}");
        assert!(changed);
        assert_eq!(cleaned, "cold open");

        let (kept, changed) = strip_forbidden("two\nlines\tand a tab\r");
        assert!(!changed);
        assert_eq!(kept, "two\nlines\tand a tab\r");
    }

    #[test]
    fn a_windows_path_becomes_a_url_path() {
        assert_eq!(
            url_path(r"H:\THEROLLUPFILES\Andy Ross.mp4"),
            "H:/THEROLLUPFILES/Andy%20Ross.mp4"
        );
        assert_eq!(url_path("/srv/media/a+b.mp4"), "/srv/media/a%2Bb.mp4");
    }

    #[test]
    fn the_two_url_dialects_differ_only_in_their_authority() {
        let path = r"H:\work\Andy Ross.mp4";
        assert_eq!(
            premiere_url(path),
            "file://localhost/H:/work/Andy%20Ross.mp4"
        );
        assert_eq!(fcpxml_url(path), "file:///H:/work/Andy%20Ross.mp4");
    }

    #[test]
    fn a_unix_absolute_path_does_not_gain_a_fourth_slash() {
        assert_eq!(
            fcpxml_url("/srv/masters/a.mp4"),
            "file:///srv/masters/a.mp4"
        );
        assert_eq!(
            premiere_url("/srv/masters/a.mp4"),
            "file://localhost/srv/masters/a.mp4"
        );
    }

    #[test]
    fn non_ascii_text_is_encoded_as_its_utf8_bytes() {
        // "é" is C3 A9 in UTF-8, and a URL path carries bytes, not characters.
        assert_eq!(url_path("/média/café.mp4"), "/m%C3%A9dia/caf%C3%A9.mp4");
    }
}
