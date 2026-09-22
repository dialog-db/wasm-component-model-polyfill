//! One benchmark's numbers.

use crate::benchmark::Benchmark;
use crate::case::Case;
use crate::error::Error;
use crate::json;
use crate::run::Run;

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
            self.error.clone().unwrap_or_default(),
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
                "\"iterations\":{},\"batch\":{},\"samples\":{},",
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
