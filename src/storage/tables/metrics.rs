// Copyright Elasticsearch B.V. and/or licensed to Elasticsearch B.V. under one
// or more contributor license agreements. See the NOTICE file distributed with
// this work for additional information regarding copyright
// ownership. Elasticsearch B.V. licenses this file to you under
// the Apache License, Version 2.0 (the "License"); you may
// not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

use crate::storage::*;
use itertools::Itertools;
use std::collections::HashMap;
use std::iter::FusedIterator;

/// ID of a metric.
///
/// Should probably be a new-type, but for now we are lazy.
pub type MetricId = u32;

/// Uniquely identifies the value of a certain metric at a certain time.
///
/// Note that this does not differentiate between different host agents.
#[derive(Debug, Default, Hash, PartialEq, Eq)]
pub struct MetricKey {
    pub timestamp: UtcTimestamp,
    pub metric_id: MetricId,
}

impl TableKey for MetricKey {
    type B = [u8; 8 + 4];

    fn from_raw(data: Self::B) -> Self {
        Self {
            timestamp: u64::from_be_bytes(data[0..8].try_into().unwrap()),
            metric_id: u32::from_le_bytes(data[8..12].try_into().unwrap()),
        }
    }

    fn into_raw(self) -> Self::B {
        let mut buf = Self::B::default();
        buf[0..8].copy_from_slice(&self.timestamp.to_be_bytes());
        buf[8..12].copy_from_slice(&self.metric_id.to_le_bytes());
        buf
    }
}

fn merge(
    key: MetricKey,
    prev: Option<TableValueRef<i64, &[u8]>>,
    values: &mut dyn Iterator<Item = TableValueRef<i64, &[u8]>>,
) -> Option<i64> {
    let Some(spec) = metric_spec_by_id(key.metric_id) else {
        return values.next().map(|x| x.read());
    };

    let init = prev.map(|x| x.read()).unwrap_or(0);
    Some(match spec.kind {
        MetricKind::Counter => values.fold(init, |a, b| a.saturating_add(b.read())),
        // Cheat and use MAX aggr within buckets: avg aggr isn't associative.
        MetricKind::Gauge => values.fold(init, |a, b| a.max(b.read())),
    })
}

new_table!(Metrics: MetricKey => i64 {
    const STORAGE_OPT: StorageOpt = StorageOpt::SeqRead;
    const MERGE_OP: MergeOperator<Self> = MergeOperator::Associative(merge);
});

impl Metrics {
    /// Select a range of metrics.
    ///
    /// Order is `(timestamp, metric_id)` ascending.
    pub fn time_range<'a>(
        &'a self,
        start: UtcTimestamp,
        end: UtcTimestamp,
    ) -> impl FusedIterator<Item = (MetricKey, i64)> + 'a {
        let start = MetricKey {
            timestamp: start,
            metric_id: 0,
        };
        let end = MetricKey {
            timestamp: end,
            metric_id: u32::MAX,
        };

        self.range(start, end).map(|(k, v)| (k, v.read()))
    }

    /// Create a histogram for each present metric ID in the given time range.
    pub fn histograms(
        &self,
        start: UtcTimestamp,
        end: UtcTimestamp,
        buckets: usize,
    ) -> HashMap<MetricId, Vec<(UtcTimestamp, AggregatedMetric)>> {
        assert!(end >= start);
        assert!(buckets > 0);

        let duration = end - start;
        let div = (duration / buckets as u64).max(1);

        let mut histograms = self
            .time_range(start, end)
            // Aggregate into `(metric_id, time_bucket) -> count` map first.
            .into_grouping_map_by(|(k, _)| (k.metric_id, k.timestamp / div * div))
            .fold(AggregatedMetric::default(), |mut acc, _, (_, count)| {
                acc.sum += count;
                acc.count += 1;
                acc
            })
            .into_iter()
            // Then re-aggregate into `metric_id -> Vec<(time_bucket, count)>` map.
            .into_grouping_map_by(|((id, _), _)| *id)
            .fold(
                Vec::with_capacity(buckets),
                |mut acc, _, ((_, time), count)| {
                    acc.push((time, count));
                    acc
                },
            );

        for histogram in histograms.values_mut() {
            histogram.sort_unstable_by_key(|(time, _)| *time);
        }

        histograms
    }
}

/// Represents `1..n` metric values after aggregation.
#[derive(Debug, Default, Clone)]
pub struct AggregatedMetric {
    count: u64,
    sum: i64,
}

impl AggregatedMetric {
    /// Gets the metrics as a sum.
    ///
    /// Use this for [`MetricKind::Counter`] metrics.
    pub fn sum(&self) -> i64 {
        self.sum
    }

    /// Gets the metric as the average.
    ///
    /// Use this for [`MetricKind::Gauge`] metrics.
    pub fn avg(&self) -> i64 {
        if self.count == 0 {
            0
        } else {
            self.sum / self.count as i64
        }
    }
}
