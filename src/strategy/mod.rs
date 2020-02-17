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

use crate::strategy::bang_bang::BangBangAutoScalerStrategy;
use enum_dispatch::enum_dispatch;

/// Bang-bang autoscaling strategy implementation.
pub mod bang_bang;

/// Autoscaling strategies / control algorithms.
#[enum_dispatch]
#[derive(Clone, Debug)]
pub enum AutoScalerStrategy {
    BangBang(BangBangAutoScalerStrategy),
}

/// Autoscaling strtategy trait.
#[enum_dispatch(AutoScalerStrategy)]
pub trait AutoScalerStrategyTrait {
    /// What is the next desired state? Return the delta in terms of the number of replicas.
    fn evaluate(&self, replicas: u32, value: f64) -> Option<i32>;
}
