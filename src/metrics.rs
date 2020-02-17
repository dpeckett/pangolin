/*
 * Copyright 2020 Damian Peckett <damian@pecke.tt>
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use crate::error::*;
use futures::channel::mpsc::unbounded;
use futures::sink::SinkExt;
use futures::StreamExt;
use slog::{debug, Logger};
use snafu::ResultExt;
use std::time::Duration;

/// Retrieve the mean value of a Prometheus metric from a list of pod ips.
pub async fn retrieve_aggregate_metric(
    logger: Logger,
    pod_ips: Vec<String>,
    metric_name: &str,
) -> Result<Option<f64>, Error> {
    let mut metrics_receiver = {
        // Create a channel which will receive the metrics as they are pulled in by the various http client threads.
        let (metrics_sender, metrics_receiver) = unbounded::<Result<Option<f64>, Error>>();
        for pod_ip in &pod_ips {
            let pod_ip = String::from(pod_ip);
            let metric = String::from(metric_name);
            let mut metrics_sender = metrics_sender.clone();
            let metrics_logger = logger.clone();
            // Spawn a new coroutine to pull metrics from the supplied pod ip.
            // All pod ips are interrogated concurrently.
            tokio::spawn(async move {
                debug!(metrics_logger, "Querying pod metrics";
                    "pod_ip" => pod_ip.clone(),
                    "metric_name" => metric.clone());
                metrics_sender
                    .send(pull_metric_from_pod(&pod_ip, &metric).await)
                    .await
                    .unwrap();
            });
        }
        metrics_receiver
    };

    // As the pulled metrics arrive, collect the valid results.
    let mut metric_values: Vec<f64> = Vec::new();
    while let Some(metric_result) = metrics_receiver.next().await {
        if let Ok(Some(metric_value)) = metric_result {
            metric_values.push(metric_value);
        }
    }

    // Compute the mean metric value across all pods.
    Ok(if !metric_values.is_empty() {
        let n_values = metric_values.len() as f64;
        Some(metric_values.into_iter().sum::<f64>() / n_values)
    } else {
        None
    })
}

/// Retrieve a Prometheus metric value from a pod.
async fn pull_metric_from_pod(pod_ip: &str, metric_name: &str) -> Result<Option<f64>, Error> {
    // Construct a new client for pulling metrics.
    let metric_uri = format!("http://{}:9090/metrics", pod_ip);
    let metrics_client = reqwest::Client::builder()
        .timeout(Duration::from_millis(250))
        .build()
        .context(HttpClient {})?;

    // Pull the Prometheus metrics from the pod.
    let response = metrics_client
        .get(&metric_uri)
        .send()
        .await
        .context(HttpClient {})?
        .text()
        .await
        .context(HttpClient {})?;

    // Very primitive prometheus text mode parser.
    for record in response.lines() {
        let record = String::from(record);
        let mut fields = record.split_whitespace();
        if fields.next().unwrap().eq(metric_name) {
            let metric_value: f64 = fields.next().unwrap().parse().unwrap();
            return Ok(Some(metric_value));
        }
    }

    Ok(None)
}
