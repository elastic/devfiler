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

//! Access to the information from `metrics.json`.

use lazy_static::lazy_static;
use serde::Deserialize;
use std::collections::HashMap;

/// Determines whether the metric is a counter or a gauge.
#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MetricKind {
    Counter,
    Gauge,
}

/// Information about a metric.
#[derive(Debug, Deserialize)]
pub struct MetricSpec {
    pub id: u32,
    #[allow(dead_code)]
    pub unit: Option<&'static str>,
    #[allow(dead_code)]
    pub name: &'static str,
    pub field: Option<&'static str>,
    #[serde(rename = "type")]
    pub kind: MetricKind,
}

/// Get the specification for a given metric by its ID.
pub fn metric_spec_by_id(id: u32) -> Option<&'static MetricSpec> {
    SPECS.1.get(id as usize).map(Option::as_ref).flatten()
}

/// UP's `metrics.json` embedded into this executable.
static METRICS_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/opentelemetry-ebpf-profiler/metrics/metrics.json"
));

fn parse_embedded_spec() -> (bool, Vec<Option<MetricSpec>>) {
    let parsed: Vec<MetricSpec> = match serde_json::from_str(&METRICS_JSON) {
        Ok(x) => x,
        Err(e) => {
            tracing::error!("Failed to parse embedded `metrics.json`: {e:?}");
            return (false, vec![]);
        }
    };

    let mut max_id = 0;
    let mut spec_map: HashMap<_, _> = parsed
        .into_iter()
        .inspect(|x| max_id = max_id.max(x.id))
        .map(|x| (x.id, x))
        .collect();

    (true, (0..=max_id).map(|id| spec_map.remove(&id)).collect())
}

lazy_static! {
    static ref SPECS: (bool, Vec<Option<MetricSpec>>) = parse_embedded_spec();
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses() {
        assert!(super::SPECS.0);
    }
}
