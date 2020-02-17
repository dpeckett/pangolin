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
use kube::api::{Api, ListParams};
use kube::client::APIClient;
use snafu::ResultExt;
use std::collections::BTreeMap;

pub(crate) async fn get_running_pod_ips(
    kube_client: APIClient,
    namespace: &str,
    match_labels: &BTreeMap<String, String>,
) -> Result<Vec<String>, Error> {
    let label_selector = Some(build_label_selector(match_labels));

    let pods = Api::v1Pod(kube_client)
        .within(namespace)
        .list(&ListParams {
            label_selector,
            ..Default::default()
        })
        .await
        .context(Kube {})?;

    let mut pod_ips: Vec<String> = Vec::new();
    for pod in pods {
        let status = pod.status.as_ref().unwrap();
        if let Some(phase) = &status.phase {
            if !phase.eq_ignore_ascii_case("Running") {
                continue;
            }
        }
        pod_ips.push(pod.status.unwrap().pod_ip.as_ref().unwrap().clone());
    }
    Ok(pod_ips)
}

/// Convert a matchLabels map into a list of labels for the k8s api.
pub(crate) fn build_label_selector(match_labels: &BTreeMap<String, String>) -> String {
    match_labels
        .iter()
        .fold(String::new(), |mut labels, (name, value)| {
            if !labels.is_empty() {
                labels.push(',');
            }
            labels.push_str(&format!("{}={}", name, value));
            labels
        })
}
