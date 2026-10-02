// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The JSON escaping the report writes its strings through.
//!
//! The suite writes its own JSON rather than taking a serialization
//! dependency, exactly as the conformance summary does; a guest
//! description or an error message is the only place a report carries
//! text a person wrote, so escaping it is all that is needed.

/// `text` with the characters JSON forbids in a string replaced by
/// their escapes.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if (control as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => out.push(other),
        }
    }
    out
}

/// A finite `value` as a JSON number, and anything else as `null`:
/// JSON has no NaN, and a sample the clock could not read must not
/// come back as a plausible number.
pub fn number(value: f64) -> String {
    if value.is_finite() {
        format!("{value:.1}")
    } else {
        "null".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_escapes_what_json_forbids_in_a_string() {
        assert_eq!(escape("a \"b\" \\ c"), "a \\\"b\\\" \\\\ c");
        assert_eq!(escape("one\ntwo"), "one\\ntwo");
        assert_eq!(escape("bell\u{7}"), "bell\\u0007");
    }

    #[wcmp_macros::test]
    fn it_writes_a_number_json_cannot_carry_as_null() {
        assert_eq!(number(2.0), "2.0");
        assert_eq!(number(1234.56), "1234.6");
        assert_eq!(number(f64::NAN), "null");
        assert_eq!(number(f64::INFINITY), "null");
    }
}
