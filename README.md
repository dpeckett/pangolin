# Pangolin

An enhanced Horizontal Pod Autoscaler for Kubernetes. Pangolin scales deployments based on their Prometheus metrics,
using a variety of highly configurable control strategies.

## Why?

* Kubernetes HPA only supports a single scaling strategy that is not applicable to all use-cases.
* Kubernetes HPA has a limited set of configuration options, which can make it difficult to tune.
* Kubernetes HPA has limited support for scaling on custom metrics.
* Existing third party Kubernetes autoscaling tools are limited and generally tailored toward niche usecases.
* None of the existing third party Kubernetes autoscaling tools have good test coverage or are battle tested.
* None of the existing third party Kubernetes autoscaling tools appear to based on a solid control theory foundations.

There's a few major sources of potential risk when it comes to autoscaling. Many of these sources of risk fall within
the scope of the application providing control authority. The hypothesis behind Pangolin is that more robust control
authority will lead to a significant reduction in autoscaling risk. Eg. loop stability, resilient monitoring, 
measurement latency, outlier detection etc.

## Why Rust?

Because it's a fantastic systems programming language and we need to see more of it in the container space.

## Requirements

* Kubernetes v1.17.0 cluster with RBAC enabled.
* The target Deployment / Pods have Prometheus metrics available on port 9090.

## Installation

The latest stable release of Pangolin can be installed from GitHub:

```console
foo@bar:~$ kubectl apply -f https://raw.githubusercontent.com/dpeckett/pangolin/master/manifest.yaml
```

## Usage

Simply create an `AutoScaler` object in the same namespace as the deployment you wish to be autoscaled:

```yaml
apiVersion: "pangolinscaler.com/v1alpha1"
kind: AutoScaler
metadata:
  name: my-new-autoscaler
spec:
  # Autoscaling strategy / control algorithm.
  strategy: BangBang
  # Kubernetes resource kind of the autoscaling target.
  kind: Deployment
  # Selector for the autoscaling target.
  selector:
    matchLabels:
      app: my-application
  metric:
    # Prometheus metric for autoscaling decisions.
    name: response_latency_ms
    # How often to pull metrics (seconds).
    interval: 10
  # How often to evaluate the autoscaling strategy (seconds).
  interval: 60
  # Any autoscaling limits, eg the number of replicas.
  limits:
    replicas:
      min: 1
      max: 5
  # Bang-bang controller configuration.
  bangBang:
    # Bang-bang controller lower threshold.
    lower: 100.0
    # Bang-bang controller upper threshold.
    upper: 250.0
```

For more details about the AutoScaler resource look at `manifest.yaml` and `src/resource.rs` in this repository.

## Building

### Locally

You will need the latest Rust stable toolchain installed on your machine. Refer to [rustup](https://rustup.rs/) for 
more details.

```console
foo@bar:~$ cargo build --release
```

### Docker

To build an Alpine based Docker image from source:

```console
foo@bar:~$ docker build -t pangolinscaler/pangolin:v0.1.0 .
```

## Control Strategies

Currently Pangolin only supports bang-bang control, however additional strategies are under development.

- [x] Bang-bang control.
- [ ] PID control.