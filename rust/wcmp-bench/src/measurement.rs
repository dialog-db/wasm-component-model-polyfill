// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One benchmark's numbers.

use crate::benchmark::Benchmark;
use crate::case::Case;
use crate::error::Error;
use crate::json;
use crate::run::Run;

/// The note a capped measurement's row carries. See
/// [`Measurement::capped`].
const CAPPED: &str = "batch capped below the clock floor: raise max-batch";

/// What one benchmark measured: its samples and the facts that give
/// them a meaning.
///
/// The samples are nanoseconds per iteration, one per timed batch. The
/// report quotes the median rather than the mean — a browser
/// interleaves work a benchmark did not ask for, and one long sample
/// should not move the number a reader compares — and quotes the
/// spread beside it so that a noisy measurement is visible as one.
pub struct Measurement {
    name: String,
    guest: &'static str,
    payload: &'static str,
    case: Case,
    iterations: u64,
    batch: u64,
    capped: bool,
    samples_ns: Vec<f64>,
    bytes: Option<u64>,
    elements: Option<u64>,
    error: Option<String>,
}

impl Measurement {
    /// The measurement of `benchmark` from the run that drove it, with
    /// the error that ended the run when one did.
    pub fn new(benchmark: &Benchmark, run: &Run, error: Option<&Error>) -> Self {
        let mut error = error.map(ToString::to_string);
        if error.is_none() && run.samples_ns().is_empty() {
            error =
                Some("the benchmark recorded no sample: its body never called `iterate`".into());
        }
        Self {
            name: benchmark.name().to_owned(),
            guest: benchmark.guest(),
            payload: benchmark.payload(),
            case: benchmark.case(),
            iterations: run.iterations(),
            batch: run.batch(),
            capped: run.capped(),
            samples_ns: run.samples_ns().to_vec(),
            bytes: run.bytes(),
            elements: run.elements(),
            error,
        }
    }

    /// The measurement of a benchmark that could not even start.
    pub fn failed(benchmark: &Benchmark, error: &Error) -> Self {
        Self {
            name: benchmark.name().to_owned(),
            guest: benchmark.guest(),
            payload: benchmark.payload(),
            case: benchmark.case(),
            iterations: 0,
            batch: 0,
            capped: false,
            samples_ns: Vec::new(),
            bytes: None,
            elements: None,
            error: Some(error.to_string()),
        }
    }

    /// The benchmark's name, with its case.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The guest the benchmark drove.
    pub fn guest(&self) -> &'static str {
        self.guest
    }

    /// The value the benchmark moved.
    pub fn payload(&self) -> &'static str {
        self.payload
    }

    /// Why the benchmark has no numbers, when it has none.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Whether the batch reached the plan's ceiling while it still lasted
    /// less than half a sample, so that each sample sits near the clock's
    /// resolution. See [`Run::capped`].
    pub fn capped(&self) -> bool {
        self.capped
    }

    /// The sample at `percentile`, by nearest rank. `None` when the
    /// benchmark recorded no sample.
    pub fn percentile_ns(&self, percentile: f64) -> Option<f64> {
        if self.samples_ns.is_empty() {
            return None;
        }
        let mut sorted = self.samples_ns.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
        let rank = (percentile * sorted.len() as f64).ceil() as usize;
        Some(sorted[rank.clamp(1, sorted.len()) - 1])
    }

    /// The median nanoseconds one iteration took.
    pub fn median_ns(&self) -> Option<f64> {
        self.percentile_ns(0.5)
    }

    /// How far the middle eight tenths of the samples spread, as a
    /// percentage of the median: the run's own noise, in the units a
    /// reader compares two runs in.
    pub fn spread_percent(&self) -> Option<f64> {
        let median = self.median_ns()?;
        let low = self.percentile_ns(0.1)?;
        let high = self.percentile_ns(0.9)?;
        if median > 0.0 {
            Some((high - low) * 100.0 / median)
        } else {
            None
        }
    }

    /// Bytes per second at the median, when the benchmark stated the
    /// bytes it moves.
    pub fn bytes_per_second(&self) -> Option<f64> {
        let median = self.median_ns()?;
        let bytes = self.bytes?;
        (median > 0.0).then(|| bytes as f64 * 1_000_000_000.0 / median)
    }

    /// Elements per second at the median, when the benchmark stated
    /// the elements it moves.
    pub fn elements_per_second(&self) -> Option<f64> {
        let median = self.median_ns()?;
        let elements = self.elements?;
        (median > 0.0).then(|| elements as f64 * 1_000_000_000.0 / median)
    }

    /// The row this measurement contributes to the report's table:
    /// name, iterations, batch, median, spread, and throughput.
    pub fn row(&self) -> Vec<String> {
        let median = match self.median_ns() {
            Some(median) => format!("{:.3}", median / 1000.0),
            None => "-".to_owned(),
        };
        let spread = match self.spread_percent() {
            Some(spread) => format!("{spread:.1}"),
            None => "-".to_owned(),
        };
        let throughput = if let Some(bytes) = self.bytes_per_second() {
            format!("{:.1} MB/s", bytes / 1_000_000.0)
        } else if let Some(elements) = self.elements_per_second() {
            format!("{:.2} Melem/s", elements / 1_000_000.0)
        } else {
            "-".to_owned()
        };
        vec![
            self.name.clone(),
            self.iterations.to_string(),
            self.batch.to_string(),
            median,
            spread,
            throughput,
            match &self.error {
                Some(error) => error.clone(),
                None if self.capped => CAPPED.to_owned(),
                None => String::new(),
            },
        ]
    }

    /// The measurement as JSON, in the shape both targets write.
    pub fn json(&self) -> String {
        let optional = |value: Option<u64>| match value {
            Some(value) => value.to_string(),
            None => "null".to_owned(),
        };
        let quantile = |percentile: f64| match self.percentile_ns(percentile) {
            Some(value) => json::number(value),
            None => "null".to_owned(),
        };
        let derived = |value: Option<f64>| match value {
            Some(value) => json::number(value),
            None => "null".to_owned(),
        };
        format!(
            concat!(
                "{{\"name\":\"{}\",\"guest\":\"{}\",\"payload\":\"{}\",\"case\":{},",
                "\"iterations\":{},\"batch\":{},\"batch_capped\":{},\"samples\":{},",
                "\"median_ns\":{},\"min_ns\":{},\"p10_ns\":{},\"p90_ns\":{},\"spread_percent\":{},",
                "\"bytes_per_iteration\":{},\"elements_per_iteration\":{},",
                "\"bytes_per_second\":{},\"elements_per_second\":{},\"error\":{}}}"
            ),
            json::escape(&self.name),
            json::escape(self.guest),
            json::escape(self.payload),
            self.case.json(),
            self.iterations,
            self.batch,
            self.capped,
            self.samples_ns.len(),
            quantile(0.5),
            quantile(0.0),
            quantile(0.1),
            quantile(0.9),
            derived(self.spread_percent()),
            optional(self.bytes),
            optional(self.elements),
            derived(self.bytes_per_second()),
            derived(self.elements_per_second()),
            match &self.error {
                Some(error) => format!("\"{}\"", json::escape(error)),
                None => "null".to_owned(),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A measurement of `samples_ns`, with no payload and no error.
    fn of(samples_ns: &[f64]) -> Measurement {
        Measurement {
            name: "known".to_owned(),
            guest: "no guest",
            payload: "nothing",
            case: Case::None,
            iterations: samples_ns.len() as u64,
            batch: 1,
            capped: false,
            samples_ns: samples_ns.to_vec(),
            bytes: None,
            elements: None,
            error: None,
        }
    }

    #[wcmp_macros::test]
    fn it_takes_each_percentile_by_nearest_rank() {
        // The samples arrive unsorted, as a run records them.
        let measurement = of(&[4.0, 1.0, 5.0, 3.0, 2.0]);
        assert_eq!(measurement.median_ns(), Some(3.0));
        assert_eq!(measurement.percentile_ns(0.0), Some(1.0));
        assert_eq!(measurement.percentile_ns(0.1), Some(1.0));
        assert_eq!(measurement.percentile_ns(0.9), Some(5.0));
        assert_eq!(measurement.percentile_ns(1.0), Some(5.0));
    }

    #[wcmp_macros::test]
    fn it_gives_the_spread_as_a_percentage_of_the_median() {
        // From the tenth percentile, 1, to the ninetieth, 5, is 4, which
        // is 133.3 percent of the median, 3.
        let spread = of(&[1.0, 2.0, 3.0, 4.0, 5.0])
            .spread_percent()
            .expect("a spread");
        assert!((spread - 400.0 / 3.0).abs() < 1e-9, "{spread}");
        assert_eq!(
            of(&[0.0, 0.0]).spread_percent(),
            None,
            "no median to divide by"
        );
    }

    #[wcmp_macros::test]
    fn it_has_no_numbers_without_a_sample() {
        let measurement = of(&[]);
        assert_eq!(measurement.median_ns(), None);
        assert_eq!(measurement.spread_percent(), None);
        let json = measurement.json();
        assert!(json.contains("\"median_ns\":null"), "{json}");
        assert!(json.contains("\"spread_percent\":null"), "{json}");
    }

    #[wcmp_macros::test]
    fn it_marks_a_capped_batch_in_its_row_and_its_json() {
        let mut measurement = of(&[1.0]);
        assert!(!measurement.row()[6].contains("capped"));
        assert!(measurement.json().contains("\"batch_capped\":false"));
        measurement.capped = true;
        assert_eq!(measurement.row()[6], CAPPED);
        assert!(measurement.json().contains("\"batch_capped\":true"));
        // An error is the note, capped or not.
        measurement.error = Some("the guest trapped".to_owned());
        assert_eq!(measurement.row()[6], "the guest trapped");
    }
}
