// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One node of a view and its children.

use wcmp::Val;

use super::{Kind, Property, ViewError};

/// One node of a view and its children.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    /// A stable identity among siblings.
    pub key: Option<String>,
    /// What the node is.
    pub kind: Kind,
    /// The children, in document order.
    pub children: Vec<View>,
}

impl View {
    /// Read the `list<node>` a render returned back into the views at its
    /// root, in document order.
    ///
    /// # Errors
    ///
    /// A [`ViewError`] when a value has another shape than the WIT gives
    /// a node, or a node names as its parent a node that does not come
    /// before it or is text.
    pub fn from_nodes(nodes: &[Val]) -> Result<Vec<View>, ViewError> {
        let mut flat: Vec<(Option<usize>, View)> = Vec::with_capacity(nodes.len());
        for (index, node) in nodes.iter().enumerate() {
            let (parent, view) = node_of(node)?;
            if let Some(parent) = parent {
                match flat.get(parent) {
                    Some((
                        _,
                        View {
                            kind: Kind::Element { .. },
                            ..
                        },
                    )) if parent < index => {}
                    _ => {
                        return Err(ViewError(format!(
                            "node {index} names node {parent} as its parent"
                        )));
                    }
                }
            }
            flat.push((parent, view));
        }
        // Each node comes after its parent, so building from the last
        // node back hands each finished child to its parent.
        let mut roots = Vec::new();
        while let Some((parent, view)) = flat.pop() {
            match parent {
                Some(parent) => flat[parent].1.children.insert(0, view),
                None => roots.insert(0, view),
            }
        }
        Ok(roots)
    }
}

/// A field of a record value.
fn field<'v>(fields: &'v [wcmp::ValField], name: &str) -> Option<&'v Val> {
    fields
        .iter()
        .find(|field| field.name == name)
        .map(|field| &field.value)
}

/// The parent index and the view of one `node` record.
fn node_of(node: &Val) -> Result<(Option<usize>, View), ViewError> {
    let bad = || ViewError(format!("{node:?} is not a node"));
    let Val::Record(fields) = node else {
        return Err(bad());
    };
    let parent = match field(fields, "parent") {
        Some(Val::Option(None)) => None,
        Some(Val::Option(Some(parent))) => match parent.as_ref() {
            Val::U32(parent) => Some(*parent as usize),
            _ => return Err(bad()),
        },
        _ => return Err(bad()),
    };
    let key = match field(fields, "key") {
        Some(Val::Option(None)) => None,
        Some(Val::Option(Some(key))) => match key.as_ref() {
            Val::String(key) => Some(key.clone()),
            _ => return Err(bad()),
        },
        _ => return Err(bad()),
    };
    let kind = match field(fields, "kind") {
        Some(Val::Variant {
            discriminant,
            payload: Some(payload),
        }) => match (discriminant.as_str(), payload.as_ref()) {
            ("text", Val::String(text)) => Kind::Text(text.clone()),
            ("element", Val::Record(element)) => element_of(element).ok_or_else(bad)?,
            _ => return Err(bad()),
        },
        _ => return Err(bad()),
    };
    Ok((
        parent,
        View {
            key,
            kind,
            children: Vec::new(),
        },
    ))
}

/// The kind of an `element-node` record.
fn element_of(fields: &[wcmp::ValField]) -> Option<Kind> {
    let Some(Val::String(tag)) = field(fields, "tag") else {
        return None;
    };
    let pairs = |name: &str| -> Option<Vec<(String, String)>> {
        let Some(Val::List(items)) = field(fields, name) else {
            return None;
        };
        items
            .iter()
            .map(|item| match item {
                Val::Tuple(pair) => match &pair[..] {
                    [Val::String(a), Val::String(b)] => Some((a.clone(), b.clone())),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    };
    let Some(Val::List(properties)) = field(fields, "properties") else {
        return None;
    };
    let properties = properties
        .iter()
        .map(|item| {
            let Val::Tuple(pair) = item else {
                return None;
            };
            let [
                Val::String(name),
                Val::Variant {
                    discriminant,
                    payload,
                },
            ] = &pair[..]
            else {
                return None;
            };
            let property = match (discriminant.as_str(), payload.as_deref()) {
                ("text", Some(Val::String(text))) => Property::Text(text.clone()),
                ("flag", Some(Val::Bool(flag))) => Property::Flag(*flag),
                _ => return None,
            };
            Some((name.clone(), property))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Kind::Element {
        tag: tag.clone(),
        attributes: pairs("attributes")?,
        properties,
        events: pairs("events")?,
    })
}
