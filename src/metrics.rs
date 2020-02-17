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
use slog::{info, Logger};
use snafu::ResultExt;
use std::time::Duration;

pub struct MetricsRetriever {
    logger: Logger,
    pod_ips: Vec<String>,
}

impl MetricsRetriever {
    pub fn new(logger: Logger, pod_ips: Vec<String>) -> Self {
        Self { logger, pod_ips }
    }

    pub async fn retrieve_aggregate_metric(&self, metric_name: &str) -> Result<Option<f64>, Error> {
        let mut metrics_receiver = {
            let (metrics_sender, metrics_receiver) = unbounded::<Result<Option<f64>, Error>>();
            for pod_ip in &self.pod_ips {
                let pod_ip = String::from(pod_ip);
                let metric = String::from(metric_name);
                let mut metrics_sender = metrics_sender.clone();
                let metrics_logger = self.logger.clone();
                tokio::spawn(async move {
                    info!(metrics_logger, "Querying pod metrics"; "pod_ip" => pod_ip.clone());
                    metrics_sender
                        .send(Self::pull_metric_from_pod(&pod_ip, &metric).await)
                        .await
                        .unwrap();
                });
            }
            metrics_receiver
        };

        let mut metric_values: Vec<f64> = Vec::new();
        while let Some(Ok(metric_result)) = metrics_receiver.next().await {
            if let Some(metric_value) = metric_result {
                metric_values.push(metric_value);
            }
        }

        let n_values = metric_values.len() as f64;
        Ok(if !metric_values.is_empty() {
            let mean_metric_value =
                metric_values
                    .into_iter()
                    .fold(0.0, |mut accum, metric_value| {
                        accum += metric_value / n_values;
                        accum
                    });
            Some(mean_metric_value)
        } else {
            None
        })
    }

    async fn pull_metric_from_pod(pod_ip: &str, metric_name: &str) -> Result<Option<f64>, Error> {
        let metric_uri = format!("http://{}:9090/metrics", pod_ip);
        let metrics_client = reqwest::Client::builder()
            .timeout(Duration::from_millis(250))
            .build()
            .context(HttpClient {})?;

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
}
