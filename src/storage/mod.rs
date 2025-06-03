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

//! Defines the schema of our tables and abstracts access to the underlying
//! storage solution.

use std::sync::Arc;
use tracing::warn;

/// DB schema version.
///
/// Bump this on any breaking schema change. Both the serialization scheme for
/// our keys and our values doesn't support schema evolution, so essentially any
/// change other than adding or deleting tables is a breaking one.
const DB_VERSION: u32 = 4;

lazy_static::lazy_static! {
    /// Global database instance.
    pub static ref DB: Arc<Db> = Db::open().unwrap();
}

pub struct Db {
    // RocksDB tables.
    pub trace_events: TraceEvents,
    pub stack_traces: StackTraces,
    pub stack_frames: StackFrames,
    pub executables: Executables,
    pub metrics: Metrics,

    // Custom data storage.
    pub symbols: SymDb,
}

impl Db {
    /// Number of tables.
    pub const NUM_TABLES: usize = 5;

    /// Create or open the database.
    fn open() -> anyhow::Result<Arc<Self>> {
        let home = home::home_dir().unwrap_or_else(|| {
            warn!("Unable to determine home directory: fallback to /tmp.");
            "/tmp".into()
        });

        let db_dir = &home
            .join(".cache")
            .join("devfiler")
            .join(DB_VERSION.to_string());

        std::fs::create_dir_all(db_dir)?;

        Ok(Arc::new(Db {
            trace_events: open_or_create(db_dir)?,
            stack_traces: open_or_create(db_dir)?,
            stack_frames: open_or_create(db_dir)?,
            executables: open_or_create(db_dir)?,
            metrics: open_or_create(db_dir)?,
            symbols: SymDb::open_at(db_dir.join("symbols"))?,
        }))
    }

    /// Remove all event data.
    pub fn flush_events(&self) {
        for (key, _) in self.trace_events.iter() {
            self.trace_events.remove(key);
        }
        for (key, _) in self.stack_traces.iter() {
            self.stack_traces.remove(key);
        }
        for (key, _) in self.stack_frames.iter() {
            self.stack_frames.remove(key);
        }
        for (key, _) in self.executables.iter() {
            self.executables.remove(key);
        }
    }

    /// Generate a unique ID.
    pub fn generate_id(&self) -> u64 {
        // TODO: rework this to make sure keys are actually unique
        rand::random()
    }

    /// Iterator over all tables.
    pub fn tables(&self) -> [&dyn RawTable; Self::NUM_TABLES] {
        [
            &self.trace_events,
            &self.stack_traces,
            &self.stack_frames,
            &self.executables,
            &self.metrics,
        ]
    }
}

#[macro_use]
mod table;
pub use table::*;

pub mod dbtypes;
pub use dbtypes::*;

mod tables;
pub use tables::*;

mod metricspec;
pub use metricspec::*;

mod errorspec;
pub use errorspec::*;

mod notify;
pub use notify::*;

pub mod rkyvtree; // intentionally no wildcard import

mod symdb;
pub use symdb::*;
