// Copyright (c) 2023-2026 ClickHouse, Inc.
// Adapted from Langfuse v4.88.0, packages/native/src/topics.rs (MIT).
use anyhow::{Error, Result, anyhow};
use hdbscan_rs::{ClusterSelectionMethod, Hdbscan, HdbscanParams, Metric as ClusterMetric};
use holomap::{Holomap, Init, Metric};
use ndarray::Array2;

#[derive(Debug)]
pub struct TopicClusteringSettings {
    pub minimum_count: u32,
    pub min_cluster_size: u32,
    /// Number of neighboring samples, excluding the sample itself.
    pub min_samples: u32,
}

#[derive(Debug, PartialEq)]
pub struct TopicClusteringResult {
    pub status: String,
    pub labels: Vec<i32>,
    pub coordinates: Vec<Vec<f64>>,
}

/// Fits topic clusters and a separate 2D display projection in memory.
/// This is CPU-bound and synchronous; call it in a killable child process.
pub fn cluster_topic_embeddings(
    embeddings: Vec<Vec<f64>>,
    settings: TopicClusteringSettings,
) -> Result<TopicClusteringResult> {
    if settings.minimum_count < 3 || settings.min_cluster_size < 2 || settings.min_samples < 1 {
        return Err(invalid("Invalid topic clustering settings"));
    }
    let count = embeddings.len();
    let dimensions = embeddings.first().map_or(0, Vec::len);
    let mut vectors = Vec::new();
    let mut first_rounded = Vec::new();
    let mut all_identical = true;
    for (row_index, row) in embeddings.into_iter().enumerate() {
        if dimensions == 0 || row.len() != dimensions || row.iter().any(|value| !value.is_finite())
        {
            return Err(invalid(
                "Expected a finite embedding matrix with equal dimensions",
            ));
        }
        let norm = row.iter().fold(0.0_f64, |norm, value| norm.hypot(*value));
        if !norm.is_finite() || norm < 1e-12 {
            return Err(invalid("Expected nonzero embedding vectors"));
        }
        for (column, value) in row.into_iter().enumerate() {
            let normalized = value / norm;
            let rounded = (normalized * 1e8).round_ties_even();
            if row_index == 0 {
                first_rounded.push(rounded);
            } else if rounded != first_rounded[column] {
                all_identical = false;
            }
            vectors.push(normalized as f32);
        }
    }
    if count < settings.minimum_count as usize {
        return Ok(TopicClusteringResult {
            status: "insufficient_data".to_owned(),
            labels: Vec::new(),
            coordinates: Vec::new(),
        });
    }
    if all_identical {
        return Ok(TopicClusteringResult {
            status: "no_topics".to_owned(),
            labels: vec![-1; count],
            coordinates: vec![vec![0.0, 0.0]; count],
        });
    }

    let neighbors = (count - 1).min(15).min((count / 3).max(3));
    let reduced_dimensions = 10.min(count - 2).min(dimensions);
    let reduced = reduce(&vectors, dimensions, reduced_dimensions, neighbors, 0.0)?;
    let data = Array2::from_shape_vec(
        (count, reduced_dimensions),
        reduced.into_iter().map(f64::from).collect(),
    )
    .map_err(|error| failure(error.to_string()))?;
    let mut clusterer = Hdbscan::new(HdbscanParams {
        min_cluster_size: settings.min_cluster_size as usize,
        // hdbscan-rs includes the sample itself in the neighbor count.
        min_samples: Some((settings.min_samples as usize + 1).min(count)),
        metric: ClusterMetric::Euclidean,
        cluster_selection_method: ClusterSelectionMethod::Eom,
        allow_single_cluster: false,
        ..HdbscanParams::default()
    });
    let labels = clusterer
        .fit_predict(&data.view())
        .map_err(|error| failure(error.to_string()))?;
    let coordinates = reduce(&vectors, dimensions, 2, neighbors, 0.1)?
        .as_chunks::<2>()
        .0
        .iter()
        .map(|point| point.iter().copied().map(f64::from).collect())
        .collect();

    Ok(TopicClusteringResult {
        status: if labels.iter().any(|label| *label >= 0) {
            "complete"
        } else {
            "no_topics"
        }
        .to_owned(),
        labels,
        coordinates,
    })
}

fn reduce(
    vectors: &[f32],
    dimensions: usize,
    output_dimensions: usize,
    neighbors: usize,
    min_dist: f32,
) -> Result<Vec<f32>> {
    let reduced = Holomap::builder(42)
        .n_components(output_dimensions)
        .n_neighbors(neighbors)
        .min_dist(min_dist)
        .metric(Metric::Cosine)
        .init(Init::Random)
        .fit_transform(vectors, dimensions)
        .map_err(|error| failure(error.to_string()))?;
    if reduced.len() != vectors.len() / dimensions * output_dimensions
        || reduced.iter().any(|value| !value.is_finite())
    {
        return Err(failure("Topic reduction returned invalid coordinates"));
    }
    Ok(reduced)
}

fn invalid(message: impl Into<String>) -> Error {
    anyhow!("{}", message.into())
}

fn failure(message: impl Into<String>) -> Error {
    anyhow!("{}", message.into())
}
