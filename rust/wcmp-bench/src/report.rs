// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The suite's report: the same shape from either target.

use std::fmt::Write as _;

use crate::json;
use crate::measurement::Measurement;
use crate::plan::Plan;

/// Every measurement of one run, with the target, the backend, and the
/// plan that produced them.
///
/// [`Report::json`] writes the shape both targets share, so a native
/// report and a browser report differ only in `target` and in the
/// numbers themselves, and two native reports in `backend`.
/// [`Report::table`] renders the same content for a reader.
pub struct Report {
    target: &'static str,
    backend: &'static str,
    plan: Plan,
    measurements: Vec<Measurement>,
}

impl Report {
    /// The report of `measurements`, taken on `target` over `backend`
    /// under `plan`.
    pub fn new(
        target: &'static str,
        backend: &'static str,
        plan: Plan,
        measurements: Vec<Measurement>,
    ) -> Self {
        Self {
            target,
            backend,
            plan,
            measurements,
        }
    }

    /// The target the run measured.
    pub fn target(&self) -> &'static str {
        self.target
    }

    /// The backend of the runtime layer the run measured.
    pub fn backend(&self) -> &'static str {
        self.backend
    }

    /// The measurements, in the order the suite lists its benchmarks.
    pub fn measurements(&self) -> &[Measurement] {
        &self.measurements
    }

    /// Whether any benchmark failed. A runner exits non-zero on it:
    /// a suite that cannot measure is not a suite that measured zero.
    pub fn failed(&self) -> bool {
        self.measurements
            .iter()
            .any(|measurement| measurement.error().is_some())
    }

    /// The report as an aligned table, followed by what each benchmark
    /// drives and moves.
    pub fn table(&self) -> String {
        let header = [
            "benchmark",
            "iterations",
            "batch",
            "median (us)",
            "spread %",
            "throughput",
            "note",
        ];
        let mut rows: Vec<Vec<String>> =
            vec![header.iter().map(|cell| (*cell).to_owned()).collect()];
        rows.extend(
            self.measurements
                .iter()
                .map(|measurement| measurement.row()),
        );
        let widths: Vec<usize> = (0..header.len())
            .map(|column| {
                rows.iter()
                    .map(|cells| cells[column].len())
                    .max()
                    .unwrap_or_default()
            })
            .collect();

        let mut out = String::new();
        let _ = writeln!(out, "target: {}", self.target);
        let _ = writeln!(out, "backend: {}", self.backend);
        let _ = writeln!(
            out,
            "plan: warm-up {} iterations, {} samples, {} ms per sample, batch at most {}, provider {}",
            self.plan.warmup_iterations,
            self.plan.samples,
            self.plan.target_sample_ms,
            self.plan.max_batch,
            if self.plan.provider { "on" } else { "off" }
        );
        out.push('\n');
        for cells in &rows {
            let line = cells
                .iter()
                .enumerate()
                .map(|(column, cell)| {
                    if column == 0 {
                        format!("{cell:<width$}", width = widths[column])
                    } else {
                        format!("{cell:>width$}", width = widths[column])
                    }
                })
                .collect::<Vec<_>>()
                .join("  ");
            let _ = writeln!(out, "{}", line.trim_end());
        }
        out.push('\n');
        let _ = writeln!(out, "what each benchmark drives and moves:");
        for measurement in &self.measurements {
            let _ = writeln!(out, "  {}", measurement.name());
            let _ = writeln!(out, "    guest:   {}", measurement.guest());
            let _ = writeln!(out, "    payload: {}", measurement.payload());
        }
        out
    }

    /// The report as JSON, for tooling.
    pub fn json(&self) -> String {
        let mut out = format!(
            "{{\"target\":\"{}\",\"backend\":\"{}\",\"plan\":{},\"benchmarks\":[",
            json::escape(self.target),
            json::escape(self.backend),
            self.plan.json()
        );
        for (index, measurement) in self.measurements.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            out.push_str(&measurement.json());
        }
        out.push_str("]}\n");
        out
    }
}
