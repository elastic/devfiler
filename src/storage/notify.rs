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

//! Notification mechanism about table changes.

use crate::storage::{Db, DB};

/// Tracks table sequence numbers for change detection.
#[derive(Debug, Default)]
pub struct UpdateWatcher {
    prev_seq: [u64; Db::NUM_TABLES],
}

impl UpdateWatcher {
    /// Detect whether any table changed since the last call.
    ///
    /// The first call will always return `true`.
    pub fn any_changes(&mut self) -> bool {
        let mut any_change = false;

        for (table, entry) in DB.tables().into_iter().zip(&mut self.prev_seq) {
            let new_seq = table.last_seq();
            let old_seq = std::mem::replace(entry, new_seq);
            any_change |= old_seq != new_seq;
        }

        any_change
    }
}
