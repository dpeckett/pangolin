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
use crate::kubernetes::deployment::KubernetesDeploymentResource;
use crate::kubernetes::replicaset::KubernetesReplicaSetResource;
use crate::kubernetes::statefulset::KubernetesStatefulSetResource;
use crate::kubernetes::KubernetesResource;
use crate::kubernetes::KubernetesResourceTrait;
use crate::kubernetes::{KubernetesObject, KubernetesObjectTrait};
use crate::metrics::MetricsRetriever;
use crate::resource::{AutoScaler, AutoScalerKubernetesResourceKind, AutoScalerStrategyKind};
use crate::strategy::bang_bang::BangBangAutoScalerStrategy;
use crate::strategy::AutoScalerStrategy;
use crate::strategy::AutoScalerStrategyTrait;
use crate::timer::CancellableInterval;
use chrono::{DateTime, Utc};
use clap::{crate_authors, crate_description, crate_name, crate_version, App};
use futures::channel::mpsc::unbounded;
use futures::channel::mpsc::UnboundedSender;
use futures::{SinkExt, StreamExt};
use kube::api::{Api, Informer, ListParams, WatchEvent};
use kube::client::APIClient;
use kube::config;
use slog::{error, info, o, warn, Drain, Logger};
use snafu::OptionExt;
use snafu::ResultExt;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::RwLock;

mod error;
#[allow(clippy::type_complexity)]
mod kubernetes;
mod metrics;
mod resource;
mod strategy;
mod timer;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let logger = slog::Logger::root(
        Mutex::new(slog_json::Json::default(std::io::stdout())).map(slog::Fuse),
        o!("application" => crate_name!(), "version" => crate_version!()),
    );

    let _matches = App::new(crate_name!())
        .version(crate_version!())
        .about(crate_description!())
        .author(crate_authors!())
        .get_matches();

    let kube_config = if let Ok(kube_config) = kube::config::incluster_config() {
        kube_config
    } else {
        config::load_kube_config().await.context(Kube {})?
    };

    let kube_client = APIClient::new(kube_config.clone());
    let autoscaler_api: Api<AutoScaler> = Api::customResource(kube_client.clone(), "autoscalers")
        .version("v1alpha1")
        .group("pangolinscaler.com");

    // Handle for managing the lifecycle of subtasks and sending update information
    let mut task_handle: HashMap<String, UnboundedSender<AutoScaler>> = HashMap::new();

    // Retrieve the current list of autoscalers.
    let autoscalers = autoscaler_api
        .list(&ListParams::default())
        .await
        .context(Kube {})?;
    for autoscaler in autoscalers {
        let task_key = format!(
            "{}/{}",
            autoscaler.metadata.namespace.as_ref().unwrap(),
            autoscaler.metadata.name
        );
        task_handle.insert(
            task_key,
            spawn_task(
                logger.new(o!(
                    "autoscaler_namespace" => autoscaler.metadata.namespace.as_ref().unwrap().clone(),
                    "autoscaler_name" => autoscaler.metadata.name.clone())),
                kube_config.clone(),
                autoscaler.clone(),
            )?,
        );
    }

    // Set up a watcher for autoscaler events.
    let informer = Informer::new(autoscaler_api)
        .timeout(15)
        .init()
        .await
        .context(Kube {})?;

    info!(logger, "Watching for AutoScaler events");

    // Loop enables us to drop and refresh the kubernetes watcher periodically
    // reduces the reliance on long lived connections and provides us a bit more resiliency.
    loop {
        let mut events = match informer
            .poll()
            .await
            .map(|events| events.boxed())
            .context(Kube {})
        {
            Ok(events) => events,
            Err(err) => {
                error!(logger, "Failed to poll for events"; "error" => format!("{}", err));
                // Rely on kubernetes for retry behavior
                return Err(err);
            }
        };

        while let Some(Ok(event)) = events.next().await {
            match event {
                WatchEvent::Added(autoscaler) => {
                    let autoscaler_namespace = autoscaler.metadata.namespace.as_ref().unwrap();
                    info!(logger, "Added AutoScaler";
                        "autoscaler_namespace" => autoscaler_namespace,
                        "autoscaler_name" => &autoscaler.metadata.name);
                    let task_key = format!("{}/{}", autoscaler_namespace, autoscaler.metadata.name);
                    task_handle.insert(
                        task_key,
                        spawn_task(
                            logger.new(o!(
                                "autoscaler_namespace" => autoscaler_namespace.clone(),
                                "autoscaler_name" => autoscaler.metadata.name.clone())),
                            kube_config.clone(),
                            autoscaler.clone(),
                        )?,
                    );
                }
                WatchEvent::Modified(autoscaler) => {
                    let autoscaler_namespace = autoscaler.metadata.namespace.as_ref().unwrap();
                    info!(logger, "Modified AutoScaler";
                        "autoscaler_namespace" => autoscaler_namespace,
                        "autoscaler_name" => &autoscaler.metadata.name);
                    let task_key = format!("{}/{}", autoscaler_namespace, autoscaler.metadata.name);
                    if let Some(task_handle) = task_handle.get_mut(&task_key) {
                        task_handle.send(autoscaler.clone()).await.unwrap();
                    } else {
                        warn!(logger, "Adding Missing AutoScaler";
                        "autoscaler_namespace" => autoscaler_namespace,
                        "autoscaler_name" => &autoscaler.metadata.name);
                        task_handle.insert(
                            task_key,
                            spawn_task(
                                logger.new(o!(
                                    "autoscaler_namespace" => autoscaler_namespace.clone(),
                                    "autoscaler_name" => autoscaler.metadata.name.clone())),
                                kube_config.clone(),
                                autoscaler.clone(),
                            )?,
                        );
                    }
                }
                WatchEvent::Deleted(autoscaler) => {
                    let autoscaler_namespace = autoscaler.metadata.namespace.as_ref().unwrap();
                    info!(logger, "Deleted AutoScaler";
                        "autoscaler_namespace" => autoscaler_namespace,
                        "autoscaler_name" => &autoscaler.metadata.name);
                    let task_key = format!("{}/{}", autoscaler_namespace, autoscaler.metadata.name);
                    // Dropping the handle will terminate the task.
                    task_handle.remove(&task_key);
                }
                WatchEvent::Error(err) => {
                    error!(logger, "AutoScaler error"; "error" => format!("{}", err))
                }
            }
        }
    }
}

fn spawn_task(
    logger: Logger,
    kube_config: kube::config::Configuration,
    autoscaler: AutoScaler,
) -> Result<UnboundedSender<AutoScaler>, Error> {
    let (update_sender, mut update_receiver) = unbounded::<AutoScaler>();

    let timer = Arc::new(RwLock::new(CancellableInterval::new(Duration::from_secs(
        autoscaler.spec.interval as u64,
    ))));
    let spec = Arc::new(RwLock::new(Some(autoscaler)));

    let timer_handle = timer.clone();
    let spec_update_handle = spec.clone();
    let timer_logger = logger.clone();
    tokio::spawn(async move {
        let initial_period = spec_update_handle
            .read()
            .await
            .as_ref()
            .unwrap()
            .spec
            .interval;

        info!(timer_logger, "Starting AutoScaler interval source";
            "period" => initial_period);

        while let Some(updated_spec) = update_receiver.next().await {
            timer_handle
                .read()
                .await
                .set_period(Duration::from_secs(updated_spec.spec.interval as u64));
            info!(timer_logger, "Updated AutoScaler interval source";
                "period" => updated_spec.spec.interval);

            spec_update_handle.write().await.replace(updated_spec);
        }

        timer_handle.read().await.cancel();
        info!(timer_logger, "Stopped AutoScaler interval source");
    });

    tokio::spawn(async move {
        let autoscaler_namespace = String::from(
            spec.read()
                .await
                .as_ref()
                .unwrap()
                .metadata
                .namespace
                .as_ref()
                .unwrap(),
        );
        info!(logger, "Starting AutoScaler task");

        while let Some(_) = timer.write().await.next().await {
            let resource_kind = spec.read().await.as_ref().unwrap().spec.kind.clone();
            let match_labels = spec
                .read()
                .await
                .as_ref()
                .unwrap()
                .spec
                .selector
                .match_labels
                .clone();

            // Create the strategy fresh each time, to simplify handling autoscaler spec changes.
            let strategy = match &spec.read().await.as_ref().unwrap().spec.strategy {
                AutoScalerStrategyKind::BangBang => {
                    AutoScalerStrategy::BangBang(BangBangAutoScalerStrategy::new(
                        spec.read()
                            .await
                            .as_ref()
                            .unwrap()
                            .spec
                            .bang_bang
                            .as_ref()
                            .context(KubeSpec {})
                            .unwrap()
                            .clone(),
                    ))
                }
            };

            autoscaler_loop(
                logger.clone(),
                kube_config.clone(),
                spec.clone(),
                autoscaler_namespace.clone(),
                resource_kind,
                match_labels,
                strategy,
            )
            .await;
        }

        info!(logger, "Stopped AutoScaler task");
    });

    Ok(update_sender)
}

async fn autoscaler_loop(
    logger: Logger,
    kube_config: kube::config::Configuration,
    autoscaler: Arc<RwLock<Option<AutoScaler>>>,
    autoscaler_namespace: String,
    resource_kind: AutoScalerKubernetesResourceKind,
    match_labels: BTreeMap<String, String>,
    strategy: AutoScalerStrategy,
) {
    // Construct a client for the expected kubernetes resource kind.
    let kubernetes_resource = match resource_kind {
        AutoScalerKubernetesResourceKind::Deployment => {
            KubernetesResource::Deployment(KubernetesDeploymentResource::new(
                kube_config.clone(),
                &autoscaler_namespace,
                &match_labels,
            ))
        }
        AutoScalerKubernetesResourceKind::ReplicaSet => {
            KubernetesResource::ReplicaSet(KubernetesReplicaSetResource::new(
                kube_config.clone(),
                &autoscaler_namespace,
                &match_labels,
            ))
        }
        AutoScalerKubernetesResourceKind::StatefulSet => {
            KubernetesResource::StatefulSet(KubernetesStatefulSetResource::new(
                kube_config.clone(),
                &autoscaler_namespace,
                &match_labels,
            ))
        }
    };

    // Get the list of matching kubernetes resources.
    let kubernetes_objects = match kubernetes_resource.list().await {
        Ok(kubernetes_objects) => kubernetes_objects,
        Err(err) => {
            warn!(logger, "AutoScaler failed to list objects";
                "error" => format!("{}", err));
            return;
        }
    };

    // For each matching object run the reconciliation task.
    for kubernetes_object in kubernetes_objects {
        // Resolve the name and namespace of the object.
        let (object_namespace, object_name) = kubernetes_object.namespace_and_name();
        info!(logger, "AutoScaler found matching object";
            "object_namespace" => &object_namespace,
            "object_name" => &object_name);

        // Run all the reconciliation tasks in parallel.
        tokio::spawn(reconciliation_loop(
            logger.new(o!(
                "object_namespace" => object_namespace,
                "object_name" => object_name)),
            autoscaler.clone(),
            kubernetes_object,
            strategy.clone(),
        ));
    }
}

#[allow(clippy::cognitive_complexity)]
async fn reconciliation_loop(
    logger: Logger,
    autoscaler: Arc<RwLock<Option<AutoScaler>>>,
    kubernetes_object: KubernetesObject,
    strategy: AutoScalerStrategy,
) {
    // Ensure the object hasn't been recently modified by another pangolin autoscaler.
    match kubernetes_object.last_modified().await {
        Ok(Some(last_modified)) => {
            // Has it been long enough since our last scaling operation?
            // We subtract 5 seconds to account for any lag in this processes reconciliation loop.
            let utc_now: DateTime<Utc> = Utc::now();
            if utc_now.signed_duration_since(last_modified).num_seconds()
                < autoscaler.read().await.as_ref().unwrap().spec.interval as i64 - 5
            {
                warn!(logger, "AutoScaler skipping object due to having been recently modified by another process");
                return;
            }
        }
        Ok(None) => (),
        Err(err) => {
            warn!(logger, "AutoScaler skipping object due to error retrieving annotations";
                "error" => format!("{}", err));
            return;
        }
    }

    // Get the list of pod ips associated with this deployment.
    let pod_ips = match kubernetes_object.pod_ips().await {
        Ok(pod_ips) => pod_ips,
        Err(err) => {
            warn!(logger, "AutoScaler skipping object due to error retrieving pod ips";
                "error" => format!("{}", err));
            return;
        }
    };

    // Pull metrics from all of the pods.
    let metrics_retriever = MetricsRetriever::new(logger.clone(), pod_ips.clone());
    let current_metric_value = match metrics_retriever
        .retrieve_aggregate_metric(&autoscaler.read().await.as_ref().unwrap().spec.metric)
        .await
    {
        Ok(Some(current_metric_value)) => current_metric_value,
        Ok(None) => {
            warn!(
                logger,
                "AutoScaler skipping object due to no available metrics"
            );
            return;
        }
        Err(err) => {
            warn!(logger, "AutoScaler skipping object due to error pulling metrics";
                "error" => format!("{}", err));
            return;
        }
    };

    info!(logger, "Successfully pulled AutoScaler metric from pods";
        "pod_ips" => format!("{:?}", pod_ips),
        "aggregate_metric_value" => current_metric_value);

    // Get the current number of replicas.
    let current_replicas = match kubernetes_object.replicas().await {
        Ok(current_replicas) => current_replicas,
        Err(err) => {
            warn!(logger, "AutoScaler skipping object due to error retrieving replica count";
                "error" => format!("{}", err));
            return;
        }
    };

    // Evaluate the autoscaling strategy.
    if let Some(delta) = strategy.evaluate(current_replicas, current_metric_value) {
        info!(logger, "Scaling object based on AutoScaler strategy";
                "aggregate_metric_value" => current_metric_value,
                "current_replicas" => current_replicas,
                "delta" => delta);

        // Verify the action wouldn't exceed a maximum replicas limit.
        if let Some(Some(max_replicas)) = autoscaler
            .read()
            .await
            .as_ref()
            .unwrap()
            .spec
            .limits
            .as_ref()
            .map(|limit| limit.replicas.as_ref().map(|replicas| replicas.max))
        {
            if current_replicas as i32 + delta >= max_replicas as i32 {
                warn!(logger, "AutoScaler refusing to scale up due to maximum replicas";
                        "max_replicas" => max_replicas,
                        "current_replicas" => current_replicas,
                        "delta" => delta);
                return;
            }
        }

        // Verify the action wouldn't fall below the minimum replicas limit.
        if let Some(Some(min_replicas)) = autoscaler
            .read()
            .await
            .as_ref()
            .unwrap()
            .spec
            .limits
            .as_ref()
            .map(|limit| limit.replicas.as_ref().map(|replicas| replicas.min))
        {
            if current_replicas as i32 + delta <= min_replicas as i32 {
                warn!(logger, "AutoScaler refusing to scale down due to minimum replicas";
                        "min_replicas" => min_replicas,
                        "current_replicas" => current_replicas,
                        "delta" => delta);
                return;
            }
        }

        // Scale the object.
        if let Err(err) = kubernetes_object
            .scale((current_replicas as i32 + delta) as u32)
            .await
        {
            error!(logger, "AutoScaler encountered error scaling object";
                    "current_replicas" => current_replicas,
                    "delta" => delta,
                    "error" => format!("{}", err));
            return;
        }
    } else {
        info!(logger, "Object does not require scaling";
                "aggregate_metric_value" => current_metric_value,
                "current_replicas" => current_replicas);
    }
}
