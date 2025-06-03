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
use smallvec::SmallVec;
use std::cmp::max;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::iter::FusedIterator;

#[derive(Debug, PartialEq, Eq, Hash, Default, Copy, Clone)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[archive_attr(derive(Debug, PartialEq, Eq, Hash))]
pub enum SampleKind {
    #[default]
    Unknown,
    Mixed,
    OnCPU,
    OffCPU,
}
impl SampleKind {
    pub fn as_archived(&self) -> ArchivedSampleKind {
        match self {
            SampleKind::Unknown => ArchivedSampleKind::Unknown,
            SampleKind::Mixed => ArchivedSampleKind::Mixed,
            SampleKind::OnCPU => ArchivedSampleKind::OnCPU,
            SampleKind::OffCPU => ArchivedSampleKind::OffCPU,
        }
    }
}

/// Unique identifier for a trace event.
///
/// Does not correspond to the random ID that we use in the ES schema. We need
/// to use an alternative key format here to ensure that the table is ordered by
/// timestamp to allow for efficient range queries.
#[derive(Debug, PartialEq, Eq, Hash, Default, Copy, Clone)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[archive_attr(derive(Debug, PartialEq, Eq, Hash))]
pub struct TraceCountId {
    pub timestamp: UtcTimestamp,
    pub id: u64,
}

impl TableKey for TraceCountId {
    type B = [u8; 16];

    fn from_raw(data: Self::B) -> Self {
        Self {
            timestamp: u64::from_be_bytes(data[0..8].try_into().unwrap()),
            id: u64::from_le_bytes(data[8..16].try_into().unwrap()),
        }
    }

    fn into_raw(self) -> Self::B {
        let mut buf = Self::B::default();
        buf[0..8].copy_from_slice(&self.timestamp.to_be_bytes());
        buf[8..16].copy_from_slice(&self.id.to_le_bytes());
        buf
    }
}

/// Stack trace event.
#[derive(Debug, Default)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[archive_attr(derive(Debug, PartialEq, Eq, Hash))]
pub struct TraceCount {
    pub timestamp: UtcTimestamp,
    pub trace_hash: TraceHash,
    pub count: u32,
    pub comm: String,
    pub pod_name: Option<String>,
    pub container_name: Option<String>,
    pub kind: SampleKind,
}

new_table!(TraceEvents: TraceCountId => TraceCount {
    const STORAGE_OPT: StorageOpt = StorageOpt::SeqRead;
});

impl TraceEvents {
    /// Iterate over events in the given time range.
    ///
    /// Iteration is ascending by timestamp.
    pub fn time_range<'a>(
        &'a self,
        start: UtcTimestamp,
        end: UtcTimestamp,
    ) -> impl FusedIterator<Item = (TraceCountId, TableValueRef<TraceCount, SmallVec<[u8; 64]>>)> + 'a
    {
        let start = TraceCountId {
            timestamp: start.into(),
            id: 0,
        };
        let end = TraceCountId {
            timestamp: end.into(),
            id: u64::MAX,
        };

        self.range(start, end)
    }

    /// Iterate over events in the given time range with specific sample kind.
    ///
    /// Iteration is ascending by timestamp.
    pub fn time_range_with_kind<'a>(
        &'a self,
        kind: SampleKind,
        start: UtcTimestamp,
        end: UtcTimestamp,
    ) -> impl FusedIterator<Item = (TraceCountId, TableValueRef<TraceCount, SmallVec<[u8; 64]>>)> + 'a
    {
        let start = TraceCountId {
            timestamp: start.into(),
            id: 0,
        };
        let end = TraceCountId {
            timestamp: end.into(),
            id: u64::MAX,
        };

        self.range(start, end)
            .filter(move |(_id, value)| value.get().kind == kind.as_archived())
    }

    /// Group the given time range into buckets and count the number of events
    /// in each bucket.
    pub fn event_count_buckets(
        &self,
        kind: SampleKind,
        start: UtcTimestamp,
        end: UtcTimestamp,
        buckets: usize,
    ) -> EventCountBuckets {
        if start >= end || buckets == 0 {
            return vec![];
        }

        let duration = end - start;
        let step = max(duration / buckets as u64, 1);
        let start = start.next_multiple_of(step) - step;
        let end = end.next_multiple_of(step);

        let mut buckets: Vec<_> = (start..=end)
            .step_by(step as usize)
            .map(|x| (x, 0))
            .collect();

        match kind {
            SampleKind::Mixed | SampleKind::Unknown => {
                for (k, v) in self.time_range(start, end) {
                    let idx = (k.timestamp - start) / step;
                    buckets[idx as usize].1 += v.get().count as u64;
                }
            }
            _ => {
                for (k, v) in self.time_range_with_kind(kind, start, end) {
                    let idx = (k.timestamp - start) / step;
                    buckets[idx as usize].1 += v.get().count as u64;
                }
            }
        }

        buckets
    }

    /// Sample trace events and merge them by their trace hash.
    ///
    /// Other than the UP backend, this currently doesn't perform any
    /// down-sampling and aggregates all matching events.
    pub fn sample_events(
        &self,
        start: UtcTimestamp,
        end: UtcTimestamp,
    ) -> HashMap<TraceHash, SampledTrace> {
        let mut traces = HashMap::<TraceHash, SampledTrace>::new();

        for (_, trace_count) in self.time_range(start, end) {
            let tc = trace_count.get();

            let spot = match traces.entry(tc.trace_hash) {
                Entry::Occupied(x) => {
                    x.into_mut().count += tc.count as u64;
                    continue;
                }

                Entry::Vacant(x) => x,
            };

            let Some(trace) = DB.stack_traces.get(tc.trace_hash) else {
                continue;
            };

            spot.insert(SampledTrace {
                count: tc.count as u64,
                trace: trace.read(),
            });
        }

        traces
    }
}

/// Frame list and how often we've seen it.
#[derive(Debug)]
pub struct SampledTrace {
    pub count: u64,
    pub trace: Vec<Frame>,
}

/// List of `(timestamp, count)` buckets.
pub type EventCountBuckets = Vec<(UtcTimestamp, u64)>;
