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

use kube::api::{Object, Void};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type AutoScaler = Object<AutoScalerSpec, Void>;

/// Prefix to use for all object annotations.
pub const ANNOTATION_BASE: &str = "pangolinscaler.com";

/// Kubernetes resource type to scale.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum AutoScalerKubernetesResourceKind {
    Deployment,
    ReplicaSet,
    StatefulSet,
}

/// Strategy to use for autoscaling.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum AutoScalerStrategyKind {
    /// A bang-bang control strategy.
    BangBang,
}

/// Deployment selector.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AutoScalerSelector {
    /// Autoscaling deployments matching the supplied labels.
    #[serde(rename = "matchLabels")]
    pub match_labels: BTreeMap<String, String>,
}

/// Prometheus metrics configuration.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AutoScalerMetric {
    /// Prometheus metric for autoscaling decisions.
    pub name: String,
    /// How often to pull Prometheus metrics (seconds).
    pub interval: u32,
}

/// Maximum and minimum number of replicas configuration.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AutoScalerReplicaLimit {
    /// Minimum allowed number of replicas.
    pub min: u32,
    /// Maximum allowed number of replicas.
    pub max: u32,
}

/// Resource limit configuration.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AutoScalerLimits {
    /// Maximum and minimum number of replicas.
    pub replicas: Option<AutoScalerReplicaLimit>,
}

/// Bang-bang controller specific configuration.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AutoScalerBangBangStrategyConfiguration {
    /// Bang-bang controller lower threshold.
    pub lower: f64,
    /// Bang-bang controller upper threshold.
    pub upper: f64,
}

/// Pangolin AutoScaler resource specification.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AutoScalerSpec {
    /// Autoscaling strategy / control algorithm.
    pub strategy: AutoScalerStrategyKind,
    /// Kubernetes resource kind of the autoscaling target.
    pub kind: AutoScalerKubernetesResourceKind,
    /// Selector for the autoscaling target.
    pub selector: AutoScalerSelector,
    /// Prometheus metrics configuration.
    pub metric: AutoScalerMetric,
    /// How often to evaluate the autoscaling strategy (seconds).
    pub interval: u32,
    /// Any autoscaling limits, eg the number of replicas.
    pub limits: Option<AutoScalerLimits>,
    /// Bang-bang controller configuration.
    #[serde(rename = "bangBang")]
    pub bang_bang: Option<AutoScalerBangBangStrategyConfiguration>,
}
