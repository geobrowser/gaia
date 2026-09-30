//! Text templates: from a search document's fields to the one string that gets embedded.
//!
//! The template name is part of the [`crate::Descriptor`]; changing what is embedded is a new
//! slot. `content_hash` of the produced text is what `emb_<slot>_src_hash` stores, so unchanged
//! text is never re-embedded and changed text always is.

use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

use crate::error::{Error, Result};

/// `name`, then a blank line, then `description` when present. Nameless documents produce
/// nothing: they get no vector.
pub const NAME_DESCRIPTION_V1: &str = "name_description_v1";

pub fn is_known(template: &str) -> bool {
    template == NAME_DESCRIPTION_V1
}

/// Render the text for a named template. `Ok(None)` means "this document gets no vector".
pub fn apply(
    template: &str,
    name: Option<&str>,
    description: Option<&str>,
) -> Result<Option<String>> {
    match template {
        NAME_DESCRIPTION_V1 => Ok(name_description_v1(name, description)),
        other => Err(Error::UnknownTemplate(other.to_string())),
    }
}

/// NFC-normalize and collapse runs of whitespace to single spaces.
fn clean(s: &str) -> String {
    s.nfc()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn name_description_v1(name: Option<&str>, description: Option<&str>) -> Option<String> {
    let name = clean(name?);
    if name.is_empty() {
        return None;
    }
    let description = description.map(clean).filter(|d| !d.is_empty());
    Some(match description {
        Some(d) => format!("{name}\n\n{d}"),
        None => name,
    })
}

/// SHA-256 (hex) of the exact text that was embedded.
pub fn content_hash(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_only_and_name_with_description() {
        assert_eq!(
            name_description_v1(Some("Bitcoin"), None).as_deref(),
            Some("Bitcoin")
        );
        assert_eq!(
            name_description_v1(Some("Bitcoin"), Some("A digital currency.")).as_deref(),
            Some("Bitcoin\n\nA digital currency.")
        );
    }

    #[test]
    fn nameless_or_blank_gets_no_vector() {
        assert_eq!(name_description_v1(None, Some("desc")), None);
        assert_eq!(name_description_v1(Some("   "), Some("desc")), None);
        assert_eq!(
            name_description_v1(Some("x"), Some("  ")).as_deref(),
            Some("x")
        );
    }

    #[test]
    fn whitespace_collapses_and_unicode_is_nfc() {
        // "é" as e + combining acute (NFD) becomes the single code point (NFC).
        let nfd = "caf\u{0065}\u{0301}   au   lait\n\n";
        assert_eq!(
            name_description_v1(Some(nfd), None).as_deref(),
            Some("caf\u{00e9} au lait")
        );
    }

    #[test]
    fn hash_is_over_the_rendered_text() {
        let a = content_hash(&name_description_v1(Some("A"), Some("b")).unwrap());
        let b = content_hash("A\n\nb");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        assert!(matches!(
            apply("nope", Some("x"), None),
            Err(Error::UnknownTemplate(_))
        ));
    }
}
