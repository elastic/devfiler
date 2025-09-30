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

use crate::storage::{SampleKind, UtcTimestamp};
use crate::ui::app::DevfilerConfig;
use eframe::egui::Ui;
use std::fmt;

#[derive(Debug, PartialEq, Eq, Copy, Clone, Hash)]
pub enum Tab {
    FlameGraph,
    TopFunctions,
    Executables,
    Log,

    // dev-mode tabs
    TraceFreq,
    DbStats,
    GrpcLog,
}

impl fmt::Display for Tab {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Tab::FlameGraph => "Flamegraph",
            Tab::TopFunctions => "Top functions",
            Tab::Executables => "Executables",
            Tab::Log => "Log",
            Tab::TraceFreq => "Trace frequency",
            Tab::DbStats => "DB Stats",
            Tab::GrpcLog => "gRPC",
        })
    }
}

pub trait TabWidget {
    /// Returns the unique ID for this tab.
    fn id(&self) -> Tab;

    /// Update and draw the tab UI.
    ///
    /// Only invoked by the main app if this tab is active.
    fn update(
        &mut self,
        ui: &mut Ui,
        cfg: &DevfilerConfig,
        kind: SampleKind,
        start: UtcTimestamp,
        end: UtcTimestamp,
    );

    /// Whether the main view should show the button that enables this tab.
    fn show_tab_selector(&self, _cfg: &DevfilerConfig) -> bool {
        true
    }
}

mod executables;
pub use executables::*;

mod top_funcs;
pub use top_funcs::*;

mod trace_freq;
pub use trace_freq::*;

mod flamegraph;
pub use flamegraph::*;

mod dbstats;
pub use dbstats::*;

mod grpclog;
pub use grpclog::*;

mod log;
pub use log::*;
