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
use crate::kubernetes::deployment::{KubernetesDeploymentObject, KubernetesDeploymentResource};
use crate::kubernetes::replicaset::{KubernetesReplicaSetObject, KubernetesReplicaSetResource};
use crate::kubernetes::statefulset::{KubernetesStatefulSetObject, KubernetesStatefulSetResource};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use enum_dispatch::enum_dispatch;

mod common;
pub mod deployment;
pub mod replicaset;
pub mod statefulset;

/// Kubernetes resource families.
#[enum_dispatch]
pub enum KubernetesResource {
    /// A list of apps/v1 Deployment resources.
    Deployment(KubernetesDeploymentResource),
    /// A list of apps/v1 ReplicaSet resources.
    ReplicaSet(KubernetesReplicaSetResource),
    /// A list of apps/v1 StatefulSet resources.
    StatefulSet(KubernetesStatefulSetResource),
}

/// Kubernetes objects, eg deployments etc.
#[enum_dispatch]
pub enum KubernetesObject {
    /// An apps/v1 Deployment object.
    Deployment(KubernetesDeploymentObject),
    /// An apps/v1 ReplicaSet object.
    ReplicaSet(KubernetesReplicaSetObject),
    /// An apps/v1 StatefulSet object.
    StatefulSet(KubernetesStatefulSetObject),
}

#[async_trait]
#[enum_dispatch(KubernetesResource)]
pub trait KubernetesResourceTrait {
    /// Retrieve a list of matching objects from the k8s api.
    async fn list(&self) -> Result<Vec<KubernetesObject>, Error>;
}

#[async_trait]
#[enum_dispatch(KubernetesObject)]
pub trait KubernetesObjectTrait {
    /// The namespace and name of the object.
    fn namespace_and_name(&self) -> (String, String);
    /// The last time the object was modified by the autoscaler.
    async fn last_modified(&self) -> Result<Option<DateTime<Utc>>, Error>;
    /// The current number of replicas.
    async fn replicas(&self) -> Result<u32, Error>;
    /// The pod ips of every running pod belonging to this object.
    async fn pod_ips(&self) -> Result<Vec<String>, Error>;
    /// Update the number of replicas associated with this object.
    async fn scale(&self, replicas: u32) -> Result<(), Error>;
}
