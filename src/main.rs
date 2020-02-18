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
use crate::metrics::retrieve_aggregate_metric;
use crate::resource::{AutoScaler, AutoScalerKubernetesResourceKind, AutoScalerStrategyKind};
use crate::strategy::bang_bang::BangBangAutoScalerStrategy;
use crate::strategy::AutoScalerStrategy;
use crate::strategy::AutoScalerStrategyTrait;
use chrono::{DateTime, Utc};
use clap::{
    arg_enum, crate_authors, crate_description, crate_name, crate_version, value_t, App, Arg,
};
use futures::channel::mpsc::unbounded;
use futures::channel::mpsc::UnboundedSender;
use futures::{SinkExt, StreamExt};
use kube::api::{Api, Informer, ListParams, WatchEvent};
use kube::client::APIClient;
use kube::config;
use slog::{crit, debug, error, info, o, warn, Drain, Level, LevelFilter, Logger};
use snafu::ResultExt;
use std::collections::HashMap;
use std::panic;
use std::process::exit;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use stream_cancel::TakeUntil;
use stream_cancel::{StreamExt as StreamCancelExt, Tripwire};
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tokio::time::interval;
use tokio::time::Interval;

/// Pangolin error types.
mod error;
/// Kubernetes api abstraction.
#[allow(clippy::type_complexity)]
mod kubernetes;
/// Prometheus metrics related functions.
mod metrics;
/// AutoScaler specification types.
mod resource;
/// AutoScaler control strategies.
mod strategy;

arg_enum! {
    /// Log level command line argument.
    #[derive(PartialEq, Debug)]
    pub enum LogLevelArgument {
        Critical,
        Error,
        Warning,
        Info,
        Debug,
        Trace,
    }
}

impl From<LogLevelArgument> for Level {
    fn from(level_arg: LogLevelArgument) -> Level {
        match level_arg {
            LogLevelArgument::Critical => Level::Critical,
            LogLevelArgument::Error => Level::Error,
            LogLevelArgument::Warning => Level::Warning,
            LogLevelArgument::Info => Level::Info,
            LogLevelArgument::Debug => Level::Debug,
            LogLevelArgument::Trace => Level::Trace,
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let matches = App::new(crate_name!())
        .version(crate_version!())
        .about(crate_description!())
        .author(crate_authors!())
        .arg(
            Arg::with_name("LOG_LEVEL")
                .long("log-level")
                .help("set the application log level")
                .takes_value(true)
                .possible_values(&LogLevelArgument::variants())
                .case_insensitive(true)
                .default_value("Info"),
        )
        .get_matches();

    let log_level = value_t!(matches, "LOG_LEVEL", LogLevelArgument).unwrap_or_else(|e| e.exit());
    let logger = Logger::root(
        StdMutex::new(LevelFilter::new(
            slog_json::Json::default(std::io::stdout()),
            log_level.into(),
        ))
        .map(slog::Fuse),
        o!("application" => crate_name!(), "version" => crate_version!()),
    );

    // Replace the panic handler with one that will exit the process on panics (in any thread).
    // This lets Kubernetes restart the process if we hit anything unexpected.
    let panic_logger = logger.clone();
    let _ = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        crit!(panic_logger, "Thread panicked"; "error" => format!("{}", panic_info));
        exit(1);
    }));

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
            autoscaler_loop(
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

        // Loop over all AutoScaler object changes.
        while let Some(Ok(event)) = events.next().await {
            match event {
                WatchEvent::Added(autoscaler) => {
                    // New AutoScaler has been added. Start a fresh AutoScaler loop.
                    let autoscaler_namespace = autoscaler.metadata.namespace.as_ref().unwrap();
                    info!(logger, "Added autoscaler";
                        "autoscaler_namespace" => autoscaler_namespace,
                        "autoscaler_name" => &autoscaler.metadata.name);
                    let task_key = format!("{}/{}", autoscaler_namespace, autoscaler.metadata.name);
                    task_handle.insert(
                        task_key,
                        autoscaler_loop(
                            logger.new(o!(
                                "autoscaler_namespace" => autoscaler_namespace.clone(),
                                "autoscaler_name" => autoscaler.metadata.name.clone())),
                            kube_config.clone(),
                            autoscaler.clone(),
                        )?,
                    );
                }
                WatchEvent::Modified(autoscaler) => {
                    // AutoScaler object has been modified. Send across the updated object.
                    let autoscaler_namespace = autoscaler.metadata.namespace.as_ref().unwrap();
                    info!(logger, "Modified autoscaler";
                        "autoscaler_namespace" => autoscaler_namespace,
                        "autoscaler_name" => &autoscaler.metadata.name);
                    let task_key = format!("{}/{}", autoscaler_namespace, autoscaler.metadata.name);
                    if let Some(task_handle) = task_handle.get_mut(&task_key) {
                        task_handle.send(autoscaler.clone()).await.unwrap();
                    } else {
                        // Somehow we received an update for an object we haven't already created.
                        // we must have missed the addition of an AutoScaler somehow. Concerning...
                        warn!(logger, "Adding missing autoscaler";
                        "autoscaler_namespace" => autoscaler_namespace,
                        "autoscaler_name" => &autoscaler.metadata.name);
                        task_handle.insert(
                            task_key,
                            autoscaler_loop(
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
                    // AutoScaler object has been deleted. Shutdown all subtasks.
                    let autoscaler_namespace = autoscaler.metadata.namespace.as_ref().unwrap();
                    info!(logger, "Deleted autoscaler";
                        "autoscaler_namespace" => autoscaler_namespace,
                        "autoscaler_name" => &autoscaler.metadata.name);
                    let task_key = format!("{}/{}", autoscaler_namespace, autoscaler.metadata.name);
                    // Dropping the handle will terminate the task.
                    task_handle.remove(&task_key);
                }
                WatchEvent::Error(err) => {
                    // AutoScaler object error.
                    error!(logger, "Autoscaler error"; "error" => format!("{}", err))
                }
            }
        }
    }
}

fn autoscaler_loop(
    logger: Logger,
    kube_config: kube::config::Configuration,
    autoscaler: AutoScaler,
) -> Result<UnboundedSender<AutoScaler>, Error> {
    // Create a channel for receiving updated AutoScaler specifications.
    let (update_sender, mut update_receiver) = unbounded::<AutoScaler>();

    // Create an interval timer for the reconciliation loop.
    let period = autoscaler.spec.interval;
    let (timer_cancel, timer_tripwire) = Tripwire::new();
    let mut timer_cancel = Some(timer_cancel);

    // Create an interval timer for the metrics retrieval subtask.
    let metric_period = autoscaler.spec.metric.interval;
    let (metric_timer_cancel, metric_timer_tripwire) = Tripwire::new();
    let mut metric_timer_cancel = Some(metric_timer_cancel);

    let autoscaler_namespace = String::from(autoscaler.metadata.namespace.as_ref().unwrap());
    let autoscaler = Arc::new(RwLock::new(Some(autoscaler)));

    // A repository for storing a window worth of retrieved metrics.
    // This will leak a small amount of memory when matching objects get deleted.
    let metric_repository: Arc<Mutex<HashMap<String, Vec<f64>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // AutoScaler metrics retrieval subtask.
    tokio::spawn(metric_retriever_loop(
        logger.clone(),
        kube_config.clone(),
        autoscaler_namespace.clone(),
        autoscaler.clone(),
        interval(Duration::from_secs(metric_period as u64)).take_until(metric_timer_tripwire),
        metric_repository.clone(),
    ));

    // AutoScaler reconciliation subtask.
    tokio::spawn(reconciliation_loop(
        logger.clone(),
        kube_config.clone(),
        autoscaler_namespace.clone(),
        autoscaler.clone(),
        interval(Duration::from_secs(period as u64)).take_until(timer_tripwire),
        metric_repository.clone(),
    ));

    // AutoScaler update receiver subtask.
    tokio::spawn(async move {
        let initial_period = autoscaler.read().await.as_ref().unwrap().spec.interval;

        debug!(logger, "Starting autoscaler interval source";
            "period" => initial_period);

        // Process any changes to an AutoScalers timing. And update our shared AutoScaler object.
        while let Some(updated_autoscaler) = update_receiver.next().await {
            debug!(logger, "Received autoscaler update event");

            drop(timer_cancel.take());
            drop(metric_timer_cancel.take());

            debug!(logger, "Cancelled existing timer driven tasks");

            autoscaler.write().await.replace(updated_autoscaler.clone());

            debug!(logger, "Updated autoscaler spec");

            // Create new timer sources.
            let (updated_timer_cancel, updated_timer_tripwire) = Tripwire::new();
            let (updated_metric_timer_cancel, updated_metric_timer_tripwire) = Tripwire::new();
            timer_cancel.replace(updated_timer_cancel);
            metric_timer_cancel.replace(updated_metric_timer_cancel);

            debug!(logger, "Spawning updated timer driven tasks");

            // Updated AutoScaler metrics retrieval subtask.
            tokio::spawn(metric_retriever_loop(
                logger.clone(),
                kube_config.clone(),
                autoscaler_namespace.clone(),
                autoscaler.clone(),
                interval(Duration::from_secs(
                    updated_autoscaler.spec.metric.interval as u64,
                ))
                .take_until(updated_metric_timer_tripwire),
                metric_repository.clone(),
            ));

            // Updated AutoScaler reconciliation subtask.
            tokio::spawn(reconciliation_loop(
                logger.clone(),
                kube_config.clone(),
                autoscaler_namespace.clone(),
                autoscaler.clone(),
                interval(Duration::from_secs(updated_autoscaler.spec.interval as u64))
                    .take_until(updated_timer_tripwire),
                metric_repository.clone(),
            ));
        }

        // Shutdown interval sources, will lead to the completion of subtasks that depend upon them.
        // Eg. is used to propagate a full shutdown.
        drop(timer_cancel.take());
        drop(metric_timer_cancel.take());
        debug!(logger, "Cancelled timer driven tasks");
    });

    Ok(update_sender)
}

async fn reconciliation_loop(
    logger: Logger,
    kube_config: kube::config::Configuration,
    autoscaler_namespace: String,
    autoscaler: Arc<RwLock<Option<AutoScaler>>>,
    mut timer: TakeUntil<Interval, Tripwire>,
    metric_repository: Arc<Mutex<HashMap<String, Vec<f64>>>>,
) {
    debug!(logger, "Starting autoscaler task");

    while let Some(_) = timer.next().await {
        // Create the strategy fresh each time, to simplify handling autoscaler spec changes.
        let strategy = match &autoscaler.read().await.as_ref().unwrap().spec.strategy {
            AutoScalerStrategyKind::BangBang => {
                if let Some(bang_bang) = &autoscaler.read().await.as_ref().unwrap().spec.bang_bang {
                    Some(AutoScalerStrategy::BangBang(
                        BangBangAutoScalerStrategy::new(bang_bang.clone()),
                    ))
                } else {
                    None
                }
            }
        };

        if let Some(strategy) = strategy {
            // Spawn subtasks to handle reconciliation of each matching object.
            spawn_reconciliation_tasks(
                logger.clone(),
                kube_config.clone(),
                autoscaler.clone(),
                autoscaler_namespace.clone(),
                strategy,
                metric_repository.clone(),
            )
            .await;
        } else {
            error!(
                logger,
                "Autoscaler is missing required autoscaling strategy configuration"
            );
        }
    }

    debug!(logger, "Stopped autoscaler task");
}

/// Task to pull metrics from all objects associated with an AutoScaler.
async fn metric_retriever_loop(
    logger: Logger,
    kube_config: kube::config::Configuration,
    autoscaler_namespace: String,
    autoscaler: Arc<RwLock<Option<AutoScaler>>>,
    mut metric_timer: TakeUntil<Interval, Tripwire>,
    metric_repository: Arc<Mutex<HashMap<String, Vec<f64>>>>,
) {
    debug!(logger, "Starting autoscaler metric task");

    while let Some(_) = metric_timer.next().await {
        let metric_name = autoscaler
            .read()
            .await
            .as_ref()
            .unwrap()
            .spec
            .metric
            .name
            .clone();

        if let Ok(kubernetes_objects) = matching_objects(
            logger.clone(),
            kube_config.clone(),
            autoscaler_namespace.clone(),
            autoscaler.clone(),
        )
        .await
        {
            for kubernetes_object in kubernetes_objects {
                // Resolve the name and namespace of the object.
                let (object_namespace, object_name) = kubernetes_object.namespace_and_name();
                debug!(logger.clone(), "Autoscaler metric task found matching object";
                    "object_namespace" => &object_namespace,
                    "object_name" => &object_name);

                // Get the list of pod ips associated with this deployment.
                let pod_ips_and_ports = match kubernetes_object.pod_ips().await {
                    Ok(pod_ips) => pod_ips.iter().map(|pod_ip| format!("{}:9090", pod_ip)).collect::<Vec<_>>(),
                    Err(err) => {
                        warn!(logger, "Autoscaler metric task skipping object due to error retrieving pod ips";
                            "error" => format!("{}", err));
                        return;
                    }
                };

                // Pull metrics from all of the pods.
                let current_metric_value = match retrieve_aggregate_metric(
                    logger.clone(),
                    pod_ips_and_ports.clone(),
                    &metric_name,
                )
                .await
                {
                    Ok(Some(current_metric_value)) => current_metric_value,
                    Ok(None) => {
                        warn!(
                            logger,
                            "Autoscaler metric task skipping object due to no available metrics"
                        );
                        return;
                    }
                    Err(err) => {
                        error!(logger, "Autoscaler metric task skipping object due to error pulling metrics";
                            "error" => format!("{}", err));
                        return;
                    }
                };

                info!(logger, "Successfully pulled autoscaler metric from pods";
                    "pod_ips_and_ports" => format!("{:?}", pod_ips_and_ports),
                    "aggregate_metric_value" => current_metric_value);

                // Add the received metrics into our shared queue for later pickup by the
                // reconciliation subtask.
                let metric_key = format!("{}/{}/{}", object_namespace, object_name, metric_name);
                {
                    let mut metric_repository_writer = metric_repository.lock().await;
                    metric_repository_writer
                        .entry(metric_key)
                        .or_insert_with(Vec::new)
                        .push(current_metric_value);
                }
            }
        }
    }

    debug!(logger, "Stopped autoscaler metric task");
}

/// For each matching object, spawn a new reconciliation subtask.
async fn spawn_reconciliation_tasks(
    logger: Logger,
    kube_config: kube::config::Configuration,
    autoscaler: Arc<RwLock<Option<AutoScaler>>>,
    autoscaler_namespace: String,
    strategy: AutoScalerStrategy,
    metric_repository: Arc<Mutex<HashMap<String, Vec<f64>>>>,
) {
    // For each matching object run the reconciliation task.
    if let Ok(kubernetes_objects) = matching_objects(
        logger.clone(),
        kube_config,
        autoscaler_namespace,
        autoscaler.clone(),
    )
    .await
    {
        for kubernetes_object in kubernetes_objects {
            // Resolve the name and namespace of the object.
            let (object_namespace, object_name) = kubernetes_object.namespace_and_name();
            debug!(logger.clone(), "Autoscaler reconciliation task found matching object";
                "object_namespace" => &object_namespace,
                "object_name" => &object_name);

            // Run all the reconciliation tasks in parallel.
            tokio::spawn(reconciliation_task(
                logger.new(o!(
                    "object_namespace" => object_namespace.clone(),
                    "object_name" => object_name.clone())),
                autoscaler.clone(),
                kubernetes_object,
                object_namespace,
                object_name,
                strategy.clone(),
                metric_repository.clone(),
            ));
        }
    }
}

/// Perform any reconciliation tasks required in this iteration.
#[allow(clippy::cognitive_complexity)]
async fn reconciliation_task(
    logger: Logger,
    autoscaler: Arc<RwLock<Option<AutoScaler>>>,
    kubernetes_object: KubernetesObject,
    object_namespace: String,
    object_name: String,
    strategy: AutoScalerStrategy,
    metric_repository: Arc<Mutex<HashMap<String, Vec<f64>>>>,
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
                warn!(logger, "Autoscaler skipping object due to having been recently modified by another process");
                return;
            }
        }
        Ok(None) => (),
        Err(err) => {
            error!(logger, "Autoscaler skipping object due to error retrieving annotations";
                "error" => format!("{}", err));
            return;
        }
    }

    // Get the current number of replicas.
    let current_replicas = match kubernetes_object.replicas().await {
        Ok(current_replicas) => current_replicas,
        Err(err) => {
            error!(logger, "Autoscaler skipping object due to error retrieving replica count";
                "error" => format!("{}", err));
            return;
        }
    };

    // Retrieve the latest window of metrics from the repository
    let metric_name = autoscaler
        .read()
        .await
        .as_ref()
        .unwrap()
        .spec
        .metric
        .name
        .clone();
    let metric_key = format!("{}/{}/{}", object_namespace, object_name, metric_name);
    let current_metric_value = {
        if let Some(metric_values) = metric_repository.lock().await.remove(&metric_key) {
            debug!(logger, "Found collected metrics for object";
                "count" => metric_values.len());
            Some(metric_values.iter().sum::<f64>() / metric_values.len() as f64)
        } else {
            None
        }
    };

    // Do we have any metrics for this object?
    if let Some(current_metric_value) = current_metric_value {
        // Evaluate the autoscaling strategy.
        if let Some(delta) = strategy.evaluate(current_replicas, current_metric_value) {
            info!(logger, "Scaling object based on autoscaler strategy";
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
                    warn!(logger, "Autoscaler refusing to scale up due to maximum replicas";
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
                    warn!(logger, "Autoscaler refusing to scale down due to minimum replicas";
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
                error!(logger, "Autoscaler encountered error scaling object";
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
    } else {
        // No metrics are available, there are some innocent causes for this, but most of the time
        // it is concerning.
        error!(logger, "Skipping scaling object due to no available metrics";
            "aggregate_metric_value" => current_metric_value,
            "current_replicas" => current_replicas,
            "metric_name" => metric_name);
    }
}

/// Find a list of matching objects for an AutoScaler.
async fn matching_objects(
    logger: Logger,
    kube_config: kube::config::Configuration,
    autoscaler_namespace: String,
    autoscaler: Arc<RwLock<Option<AutoScaler>>>,
) -> Result<Vec<KubernetesObject>, Error> {
    let resource_kind = autoscaler.read().await.as_ref().unwrap().spec.kind.clone();
    let match_labels = autoscaler
        .read()
        .await
        .as_ref()
        .unwrap()
        .spec
        .selector
        .match_labels
        .clone();

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
    match kubernetes_resource.list().await {
        Ok(kubernetes_objects) => Ok(kubernetes_objects),
        Err(err) => {
            warn!(logger, "Autoscaler failed to list objects";
                "error" => format!("{}", err));
            Err(err)
        }
    }
}
