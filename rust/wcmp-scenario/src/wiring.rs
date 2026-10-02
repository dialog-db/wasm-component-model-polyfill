// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A scenario's wiring file.

use core::fmt;
use core::str::FromStr;

use crate::error::{Error, Result};
use crate::link::Link;
use crate::linking::Linking;

/// A scenario's wiring: which component's exports satisfy which
/// component's imports, and whether each link is made at run time or
/// by composition.
///
/// A scenario with one component, or with components that do not link,
/// has no wiring file, and its wiring is empty. A contributor writes
/// the file by hand, one link per line:
///
/// ```text
/// # <linking> <importer> <import> <exporter>
/// run-time importer local:demo/greeter exporter
/// ```
///
/// A blank line or a line whose first non-blank character is `#` is
/// ignored. The linking is `run-time` or `composition`. The importer
/// and the exporter are components as the expectations name them, and
/// the exporter exports an item under the import's name. The file reads
/// with [`str::parse`] and prints back with [`Display`](fmt::Display).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Wiring {
    /// The links, in the order the file lists them.
    pub links: Vec<Link>,
}

impl Wiring {
    /// The links the runner makes at run time.
    pub fn run_time(&self) -> impl Iterator<Item = &Link> {
        self.links
            .iter()
            .filter(|link| link.linking == Linking::RunTime)
    }

    /// The links the build makes by composition.
    pub fn composition(&self) -> impl Iterator<Item = &Link> {
        self.links
            .iter()
            .filter(|link| link.linking == Linking::Composition)
    }

    /// The order to instantiate `components` in: every exporter of a
    /// run-time link before its importer, and otherwise the order
    /// `components` come in.
    ///
    /// A composition link orders nothing and is not checked here: the
    /// build composes its exporter into its importer, so once the build
    /// made it, the exporter is no component of its own.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownComponent`] when a run-time link names a component
    /// that is not among `components`, and [`Error::LinkCycle`] when the
    /// run-time links leave no order, such as a component that imports
    /// from itself.
    pub fn order<'a>(&self, components: &[&'a str]) -> Result<Vec<&'a str>> {
        for link in self.run_time() {
            for component in [&link.importer, &link.exporter] {
                if !components.contains(&component.as_str()) {
                    return Err(Error::UnknownComponent {
                        link: link.to_string(),
                        component: component.clone(),
                    });
                }
            }
        }
        let mut ordered: Vec<&'a str> = Vec::with_capacity(components.len());
        let mut waiting: Vec<&'a str> = components.to_vec();
        while !waiting.is_empty() {
            let ready = waiting.iter().position(|component| {
                self.run_time()
                    .filter(|link| link.importer == *component)
                    .all(|link| ordered.contains(&link.exporter.as_str()))
            });
            let Some(ready) = ready else {
                return Err(Error::LinkCycle(
                    waiting
                        .iter()
                        .map(|component| component.to_string())
                        .collect(),
                ));
            };
            ordered.push(waiting.remove(ready));
        }
        Ok(ordered)
    }
}

impl FromStr for Wiring {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        let mut links = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fail = |reason: String| Error::Syntax {
                line: index + 1,
                reason,
            };
            let words: Vec<&str> = line.split_whitespace().collect();
            let [linking, importer, import, exporter] = words[..] else {
                return Err(fail(format!(
                    "expected `<linking> <importer> <import> <exporter>`, found {} words",
                    words.len()
                )));
            };
            links.push(Link {
                linking: linking
                    .parse()
                    .map_err(|error: Error| fail(error.to_string()))?,
                importer: importer.to_string(),
                import: import.to_string(),
                exporter: exporter.to_string(),
            });
        }
        Ok(Wiring { links })
    }
}

impl fmt::Display for Wiring {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for link in &self.links {
            writeln!(formatter, "{link}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = "
        # Both kinds of link.
        run-time importer local:demo/greeter exporter

        composition main local:demo/api partner
    ";

    #[wcmp_macros::test]
    fn it_reads_every_link_and_prints_a_file_that_reads_back_the_same() {
        let wiring: Wiring = EXAMPLE.parse().unwrap();
        assert_eq!(
            wiring.links,
            [
                Link {
                    linking: Linking::RunTime,
                    importer: "importer".to_string(),
                    import: "local:demo/greeter".to_string(),
                    exporter: "exporter".to_string(),
                },
                Link {
                    linking: Linking::Composition,
                    importer: "main".to_string(),
                    import: "local:demo/api".to_string(),
                    exporter: "partner".to_string(),
                },
            ]
        );
        assert_eq!(wiring.to_string().parse::<Wiring>(), Ok(wiring.clone()));
        let run_time: Vec<_> = wiring
            .run_time()
            .map(|link| link.importer.as_str())
            .collect();
        assert_eq!(run_time, ["importer"]);
        let composition: Vec<_> = wiring
            .composition()
            .map(|link| link.importer.as_str())
            .collect();
        assert_eq!(composition, ["main"]);
    }

    #[wcmp_macros::test]
    fn it_names_the_line_a_mistake_is_on() {
        for (text, line, reason) in [
            (
                "run-time importer local:demo/greeter",
                1,
                "expected `<linking> <importer> <import> <exporter>`, found 3 words",
            ),
            (
                "\nruntime importer local:demo/greeter exporter",
                2,
                "`runtime` is not a way to link",
            ),
        ] {
            assert_eq!(
                text.parse::<Wiring>(),
                Err(Error::Syntax {
                    line,
                    reason: reason.to_string()
                })
            );
        }
    }

    #[wcmp_macros::test]
    fn it_orders_each_exporter_before_its_importer_and_keeps_the_order_otherwise() {
        let wiring: Wiring = "
            run-time a local:demo/b b
            run-time b local:demo/c c
        "
        .parse()
        .unwrap();
        assert_eq!(
            wiring.order(&["a", "b", "c", "d"]),
            Ok(vec!["c", "b", "a", "d"])
        );
        assert_eq!(Wiring::default().order(&["b", "a"]), Ok(vec!["b", "a"]));
        // A composition link is the build's, so it orders nothing, and
        // once the build made it, its exporter is inside the importer.
        let composed: Wiring = "composition a local:demo/b b".parse().unwrap();
        assert_eq!(composed.order(&["a", "b"]), Ok(vec!["a", "b"]));
        assert_eq!(composed.order(&["a"]), Ok(vec!["a"]));
    }

    #[wcmp_macros::test]
    fn it_refuses_a_link_to_a_missing_component_or_links_in_a_cycle() {
        let wiring: Wiring = "run-time a local:demo/b b".parse().unwrap();
        assert_eq!(
            wiring.order(&["a"]),
            Err(Error::UnknownComponent {
                link: "run-time a local:demo/b b".to_string(),
                component: "b".to_string(),
            })
        );
        let cycle: Wiring = "
            run-time a local:demo/b b
            run-time b local:demo/a a
        "
        .parse()
        .unwrap();
        assert_eq!(
            cycle.order(&["a", "b", "c"]),
            Err(Error::LinkCycle(vec!["a".to_string(), "b".to_string()]))
        );
        let itself: Wiring = "run-time a local:demo/a a".parse().unwrap();
        assert_eq!(
            itself.order(&["a"]),
            Err(Error::LinkCycle(vec!["a".to_string()]))
        );
    }
}
