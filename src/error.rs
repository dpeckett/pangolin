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

use snafu::Snafu;

/// Pangolin Autoscaler errors.
#[derive(Debug, Snafu)]
#[snafu(visibility = "pub(crate)")]
pub enum Error {
    /// General HTTP client errors.
    #[snafu(display("http client error: {}", source))]
    HttpClient { source: reqwest::Error },

    /// JSON serialization errors.
    #[snafu(display("json serialization error: {}", source))]
    JsonSerialization { source: serde_json::Error },

    /// Kubernetes API related errors.
    #[snafu(display("kubernetes error: {}", source))]
    Kube { source: kube::Error },

    /// Kubernetes specification was missing a required field.
    #[snafu(display("kubernetes spec is missing fields"))]
    KubeSpec {},
}
