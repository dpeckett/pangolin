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
    pod_ips_and_ports: Vec<String>,
    metric_name: &str,
) -> Result<Option<f64>, Error> {
    let mut metrics_receiver = {
        // Create a channel which will receive the metrics as they are pulled in by the various http client threads.
        let (metrics_sender, metrics_receiver) = unbounded::<Result<Option<f64>, Error>>();
        for pod_ip_and_port in &pod_ips_and_ports {
            let pod_ip_and_port: String = pod_ip_and_port.into();
            let metric: String = metric_name.into();
            let mut metrics_sender = metrics_sender.clone();
            let metrics_logger = logger.clone();
            // Spawn a new coroutine to pull metrics from the supplied pod ip.
            // All pod ips are interrogated concurrently.
            tokio::spawn(async move {
                debug!(metrics_logger, "Querying pod metrics";
                    "pod_ip_and_port" => pod_ip_and_port.clone(),
                    "metric_name" => metric.clone());
                metrics_sender
                    .send(pull_metric_from_pod(&pod_ip_and_port, &metric).await)
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
async fn pull_metric_from_pod(pod_ip_and_port: &str, metric_name: &str) -> Result<Option<f64>, Error> {
    // Construct a new client for pulling metrics.
    let metric_uri = format!("http://{}/metrics", pod_ip_and_port);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{SocketAddr, TcpListener};
    use hyper::service::{make_service_fn, service_fn};
    use hyper::{Server, Request, Body, Response};
    use futures::{future, FutureExt};
    use futures::channel::oneshot;
    use slog::{Drain, o};
    use tokio::time::delay_for;

    #[tokio::test]
    async fn test_pull_metric() {
        let port = get_available_port(8_000).unwrap();
        let shutdown_sender = spawn_server(port, 220.0);

        delay_for(Duration::from_millis(20)).await;

        let response_latency_ms = pull_metric_from_pod(&format!("localhost:{}", port), "response_latency_ms").await.unwrap();
        shutdown_sender.send(()).unwrap();

        assert_eq!(response_latency_ms.unwrap() as i32, 220);
    }

    #[tokio::test]
    async fn test_retrieve_aggregate_metric() {
        let port = get_available_port(8_001).unwrap();
        let second_port = get_available_port(port + 1).unwrap();

        let shutdown_sender = spawn_server(port, 220.0);
        let second_shutdown_sender = spawn_server(second_port, 110.0);

        delay_for(Duration::from_millis(20)).await;

        let pod_ips_and_ports = vec![format!("localhost:{}", port), format!("localhost:{}", second_port)];
        let response_latency_ms = retrieve_aggregate_metric(get_logger(), pod_ips_and_ports, "response_latency_ms").await.unwrap();

        shutdown_sender.send(()).unwrap();
        second_shutdown_sender.send(()).unwrap();

        assert_eq!(response_latency_ms.unwrap() as i32, 165);
    }

    fn spawn_server(port: u16, response_latency_ms: f64) -> oneshot::Sender<()> {
        let (shutdown_sender, shutdown_receiver) = oneshot::channel::<()>();
        tokio::spawn(async move {
            let addr: SocketAddr = format!("127.0.0.1:{}", port)
                .parse()
                .unwrap();
            Server::bind(&addr)
                .serve(make_service_fn(|_| {
                    let service = service_fn(move |request| {
                        serve(request, response_latency_ms).map(Ok::<_, hyper::Error>)
                    });
                    future::ok::<_, hyper::Error>(service)
                }))
                .with_graceful_shutdown(async {
                    shutdown_receiver.await.ok();
                })
                .await
                .unwrap();
        });
        shutdown_sender
    }

    async fn serve(request: Request<Body>, response_latency_ms: f64) -> Response<Body> {
        if request
            .uri()
            .path()
            .eq("/metrics")
        {
            Response::builder()
                .status(200)
                .header("Content-Type", "text/plain")
                .body(Body::from(format!("# HELP response_latency_ms Response latency in milliseconds\n# TYPE response_latency_ms gauge\nresponse_latency_ms {}\n", response_latency_ms)))
                .unwrap()
        } else {
            Response::builder().status(404).body(Body::empty()).unwrap()
        }
    }

    fn get_available_port(base_port: u16) -> Option<u16> {
        (base_port..(base_port + 1000u16)).find(|port| port_is_available(*port))
    }

    fn port_is_available(port: u16) -> bool {
        TcpListener::bind(("127.0.0.1", port)).is_ok()
    }

    fn get_logger() -> Logger {
        let plain = slog_term::PlainSyncDecorator::new(std::io::stdout());
        Logger::root(
            slog_term::FullFormat::new(plain)
                .build().fuse(), o!()
        )
    }
}