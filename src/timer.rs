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

use futures::ready;
use futures::task::{Context, Poll};
use futures::Stream;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::time::delay_until;
use tokio::time::Delay;
use tokio::time::Instant;

/// An interval stream used as a variable time base for the autoscaler reconciliation loop.
pub(crate) struct CancellableInterval {
    delay: Delay,
    period: AtomicU32,
    active: AtomicBool,
}

impl CancellableInterval {
    /// Create a new interval stream with the specified period.
    pub(crate) fn new(period: Duration) -> Self {
        Self {
            period: AtomicU32::new(period.as_secs() as u32),
            delay: delay_until(Instant::now()),
            active: AtomicBool::new(true),
        }
    }

    /// Change the period of the interval stream.
    pub(crate) fn set_period(&self, period: Duration) {
        self.period
            .store(period.as_secs() as u32, Ordering::Relaxed);
    }

    /// Cancel the interval stream.
    pub(crate) fn cancel(&self) {
        self.active.store(false, Ordering::Relaxed);
    }

    fn poll_tick(&mut self, cx: &mut Context<'_>) -> Poll<Instant> {
        ready!(Pin::new(&mut self.delay).poll(cx));

        let now = self.delay.deadline();
        let next = now + Duration::from_secs(self.period.load(Ordering::Relaxed) as u64);
        self.delay.reset(next);

        Poll::Ready(now)
    }
}

impl Stream for CancellableInterval {
    type Item = Instant;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Instant>> {
        if !self.active.load(Ordering::Relaxed) {
            Poll::Ready(None)
        } else {
            Poll::Ready(Some(ready!(self.poll_tick(cx))))
        }
    }
}
