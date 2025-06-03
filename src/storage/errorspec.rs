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

//! Access to the information from `errors.json`.

use lazy_static::lazy_static;
use std::collections::HashMap;

/// Information about an UP error.
#[derive(Debug, serde::Deserialize)]
pub struct ErrorSpec {
    pub id: u64,
    pub name: &'static str,
    #[allow(dead_code)]
    pub description: &'static str,
    #[serde(default)]
    #[allow(dead_code)]
    pub obsolete: bool,
}

/// Get the specification for a given error by its ID.
pub fn error_spec_by_id(id: u64) -> Option<&'static ErrorSpec> {
    SPECS.1.get(&id)
}

/// UP's `errors.json` embedded into this executable.
static ERROR_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/opentelemetry-ebpf-profiler/tools/errors-codegen/errors.json"
));

fn parse_embedded_spec() -> (bool, HashMap<u64, ErrorSpec>) {
    match serde_json::from_str::<Vec<ErrorSpec>>(&ERROR_JSON) {
        Ok(x) => (true, x.into_iter().map(|x| (x.id, x)).collect()),
        Err(e) => {
            tracing::error!("Failed to parse embedded `errors.json`: {e:?}");
            return (false, HashMap::new());
        }
    }
}

lazy_static! {
    static ref SPECS: (bool, HashMap<u64, ErrorSpec>) = parse_embedded_spec();
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses() {
        assert!(super::SPECS.0);
    }
}
