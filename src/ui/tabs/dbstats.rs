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

use super::*;
use crate::storage::DB;
use egui::ScrollArea;

#[derive(Default)]
pub struct DbStatsTab;

impl TabWidget for DbStatsTab {
    fn id(&self) -> Tab {
        Tab::DbStats
    }

    fn update(
        &mut self,
        ui: &mut Ui,
        _cfg: &DevfilerConfig,
        _kind: SampleKind,
        _start: UtcTimestamp,
        _end: UtcTimestamp,
    ) {
        ScrollArea::vertical().show(ui, |ui| {
            let clicked = ui.small_button("Flush Event Data").clicked();
            if clicked {
                tracing::info!("Flushing event data");
                DB.flush_events();
            }
            for table in DB.tables() {
                ui.collapsing(table.pretty_name(), |ui| {
                    ui.monospace(table.rocksdb_statistics());
                });
            }
        });
    }

    fn show_tab_selector(&self, cfg: &DevfilerConfig) -> bool {
        cfg.dev_mode
    }
}
