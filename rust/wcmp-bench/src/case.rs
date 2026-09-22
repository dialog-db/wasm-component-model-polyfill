//! One benchmark definition's case.

use core::fmt;

/// The case a benchmark definition is measured at.
///
/// One definition can be repeated over several payload sizes
/// ([`Case::Number`]) or over several named guests ([`Case::Name`]); a
/// definition measured once carries [`Case::None`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Case {
    /// The definition is measured once, at no particular size.
    None,
    /// A payload size: the number of bytes, elements, or entries the
    /// benchmark's own body reads it as.
    Number(u64),
    /// A named case, such as one guest out of several.
    Name(&'static str),
}

impl Case {
    /// The case as a number. A named case and no case are `0`, so a
    /// benchmark that asks for a size gets one whatever it declared.
    pub fn number(self) -> u64 {
        match self {
            Case::Number(number) => number,
            Case::None | Case::Name(_) => 0,
        }
    }

    /// The case's name. A numeric case and no case are `""`.
    pub fn name(self) -> &'static str {
        match self {
            Case::Name(name) => name,
            Case::None | Case::Number(_) => "",
        }
    }

    /// The suffix this case adds to its definition's name, or `None`
    /// when the definition is measured once.
    pub fn suffix(self) -> Option<String> {
        match self {
            Case::None => None,
            Case::Number(number) => Some(number.to_string()),
            Case::Name(name) => Some(name.to_owned()),
        }
    }

    /// The case as a JSON value: a number, a string, or `null`.
    pub fn json(self) -> String {
        match self {
            Case::None => "null".to_owned(),
            Case::Number(number) => number.to_string(),
            Case::Name(name) => format!("\"{}\"", crate::json::escape(name)),
        }
    }
}

impl fmt::Display for Case {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.suffix() {
            Some(suffix) => formatter.write_str(&suffix),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_suffixes_a_numeric_case_with_its_number() {
        assert_eq!(Case::Number(4096).suffix().as_deref(), Some("4096"));
        assert_eq!(Case::Number(4096).number(), 4096);
        assert_eq!(Case::Number(4096).json(), "4096");
    }

    #[wcmp_macros::test]
    fn it_suffixes_a_named_case_with_its_name() {
        assert_eq!(Case::Name("maps").suffix().as_deref(), Some("maps"));
        assert_eq!(Case::Name("maps").name(), "maps");
        assert_eq!(Case::Name("maps").json(), "\"maps\"");
    }

    #[wcmp_macros::test]
    fn it_leaves_a_definition_measured_once_unsuffixed() {
        assert_eq!(Case::None.suffix(), None);
        assert_eq!(Case::None.number(), 0);
        assert_eq!(Case::None.json(), "null");
    }
}
