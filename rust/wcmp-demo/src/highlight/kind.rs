// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What a token of Zena source is, for its color.

/// What a token of Zena source is: the categories of the playground's
/// highlighter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A line or block comment.
    Comment,
    /// A string, or a template string's text.
    String,
    /// An escape in a string, such as `\n`.
    Escape,
    /// Brackets, separators, and a template's `${` and `}`.
    Punctuation,
    /// A decorator, such as `@intrinsic`.
    Meta,
    /// A number.
    Number,
    /// An operator.
    Operator,
    /// A private member, such as `#count`.
    Property,
    /// A keyword of control flow, such as `if` or `return`.
    Control,
    /// A keyword that declares, such as `let` or `class`.
    Definition,
    /// A keyword of modules: `import`, `export`, `from`, `declare`.
    Module,
    /// A modifier, such as `async` or `static`.
    Modifier,
    /// A keyword that is an operator, such as `new` or `as`.
    OperatorKeyword,
    /// `this` or `super`.
    SelfKeyword,
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool,
    /// A type: a built-in one, or a name with a leading capital.
    Type,
    /// Any other name.
    Variable,
}

impl Kind {
    /// The CSS class of the kind.
    pub fn class(self) -> &'static str {
        match self {
            Kind::Comment => "tok-comment",
            Kind::String => "tok-string",
            Kind::Escape => "tok-escape",
            Kind::Punctuation => "tok-punctuation",
            Kind::Meta => "tok-meta",
            Kind::Number => "tok-number",
            Kind::Operator => "tok-operator",
            Kind::Property => "tok-property",
            Kind::Control => "tok-control",
            Kind::Definition => "tok-definition",
            Kind::Module => "tok-module",
            Kind::Modifier => "tok-modifier",
            Kind::OperatorKeyword => "tok-operator-keyword",
            Kind::SelfKeyword => "tok-self",
            Kind::Null => "tok-null",
            Kind::Bool => "tok-bool",
            Kind::Type => "tok-type",
            Kind::Variable => "tok-variable",
        }
    }
}
