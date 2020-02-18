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

use crate::resource::AutoScalerBangBangStrategyConfiguration;
use crate::strategy::AutoScalerStrategyTrait;

/// Implementation of a bang-bang controller.
#[derive(Clone, Debug)]
pub struct BangBangAutoScalerStrategy {
    configuration: AutoScalerBangBangStrategyConfiguration,
}

impl BangBangAutoScalerStrategy {
    pub fn new(configuration: AutoScalerBangBangStrategyConfiguration) -> Self {
        Self { configuration }
    }
}

impl AutoScalerStrategyTrait for BangBangAutoScalerStrategy {
    fn evaluate(&self, _replicas: u32, value: f64) -> Option<i32> {
        if value <= self.configuration.lower {
            // Below the configured lower threshold, scale down one replica.
            Some(-1)
        } else if value >= self.configuration.upper {
            // Above the configured upper threshold, scale up one replica.
            Some(1)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_bang_bang_strategy() {
        let strategy = BangBangAutoScalerStrategy::new(AutoScalerBangBangStrategyConfiguration {
            lower: 10.0,
            upper: 30.0
        });

        assert_eq!(strategy.evaluate(1, 32.0).unwrap(), 1);
        assert!(strategy.evaluate(1, 20.0).is_none());
        assert_eq!(strategy.evaluate(1, 8.0).unwrap(), -1);
    }
}