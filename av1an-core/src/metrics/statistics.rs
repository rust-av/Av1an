use std::{cmp::Ordering, collections::HashMap};

pub struct MetricStatistics {
    scores: Vec<f64>,
    cache:  HashMap<String, f64>,
}

impl MetricStatistics {
    pub fn new(scores: Vec<f64>) -> Self {
        MetricStatistics {
            scores,
            cache: HashMap::new(),
        }
    }

    fn get_or_compute(&mut self, key: &str, compute: impl FnOnce(&[f64]) -> f64) -> f64 {
        *self.cache.entry(key.to_string()).or_insert_with(|| compute(&self.scores))
    }

    pub fn mean(&mut self) -> f64 {
        self.get_or_compute("average", |scores| {
            scores.iter().sum::<f64>() / scores.len() as f64
        })
    }

    pub fn harmonic_mean(&mut self) -> f64 {
        self.get_or_compute("harmonic_mean", |scores| {
            let sum_reciprocals: f64 = scores.iter().map(|&x| 1.0 / x).sum();
            scores.len() as f64 / sum_reciprocals
        })
    }

    pub fn median(&mut self) -> f64 {
        let mut sorted_scores = self.scores.clone();
        sorted_scores.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Less));
        self.get_or_compute("median", |scores| {
            let mid = scores.len() / 2;
            if scores.len() % 2 == 0 {
                f64::midpoint(sorted_scores[mid - 1], sorted_scores[mid])
            } else {
                sorted_scores[mid]
            }
        })
    }

    pub fn mode(&mut self) -> f64 {
        let mut counts = HashMap::new();
        for score in &self.scores {
            // Round to nearest integer for fewer unique buckets
            let rounded_score = score.round() as i32;
            *counts.entry(rounded_score).or_insert(0) += 1;
        }
        let max_count = counts.values().copied().max().unwrap_or(0);
        self.get_or_compute("mode", |scores| {
            *scores
                .iter()
                .find(|score| counts[&(score.round() as i32)] == max_count)
                .unwrap_or(&0.0)
        })
    }

    pub fn minimum(&mut self) -> f64 {
        self.get_or_compute("minimum", |scores| {
            *scores
                .iter()
                .min_by(|a, b| a.partial_cmp(b).expect("should not have NaN in scores set"))
                .unwrap_or(&0.0)
        })
    }

    pub fn maximum(&mut self) -> f64 {
        self.get_or_compute("maximum", |scores| {
            *scores
                .iter()
                .max_by(|a, b| a.partial_cmp(b).expect("should not have NaN in scores set"))
                .unwrap_or(&0.0)
        })
    }

    pub fn variance(&mut self) -> f64 {
        let average = self.mean();
        self.get_or_compute("variance", |scores| {
            scores
                .iter()
                .map(|x| {
                    let diff = x - average;
                    diff * diff
                })
                .sum::<f64>()
                / scores.len() as f64
        })
    }

    pub fn standard_deviation(&mut self) -> f64 {
        let variance = self.variance();
        self.get_or_compute("standard_deviation", |_| variance.sqrt())
    }

    pub fn percentile(&mut self, index: usize) -> f64 {
        let mut sorted_scores = self.scores.clone();
        sorted_scores.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Less));
        self.get_or_compute(&format!("percentile_{index}"), |scores| {
            let index = (index as f64 / 100.0 * scores.len() as f64) as usize;
            *sorted_scores.get(index).unwrap_or(&sorted_scores[0])
        })
    }

    pub fn root_mean_square(&mut self) -> f64 {
        self.get_or_compute("root_mean_square", |scores| {
            let sum_of_squares: f64 = scores.iter().map(|&x| x * x).sum();
            (sum_of_squares / scores.len() as f64).sqrt()
        })
    }
}
