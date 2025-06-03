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
use eframe::emath::Align;
use egui::Layout;
use egui_extras::{Column, TableBuilder};

#[derive(Default)]
pub struct LogTab;

impl TabWidget for LogTab {
    fn id(&self) -> Tab {
        Tab::Log
    }

    fn update(
        &mut self,
        ui: &mut Ui,
        _cfg: &DevfilerConfig,
        _kind: SampleKind,
        _start: UtcTimestamp,
        _end: UtcTimestamp,
    ) {
        let table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::auto())
            .column(Column::auto())
            .column(Column::auto())
            .column(Column::remainder().clip(true))
            .max_scroll_height(f32::INFINITY);

        let messages = crate::log::tail(1000);

        table
            .header(20.0, |mut header| {
                for text in ["Time", "Level", "Source", "Message"] {
                    header.col(|ui| {
                        ui.strong(text);
                    });
                }
            })
            .body(|body| {
                body.rows(20.0, messages.len(), |mut row| {
                    let msg = &messages[row.index()];
                    row.col(|ui| {
                        ui.label(msg.time.to_string());
                    });
                    row.col(|ui| {
                        ui.label(msg.level.to_string());
                    });
                    row.col(|ui| {
                        ui.label(msg.target.to_string());
                    });
                    row.col(|ui| {
                        ui.label(msg.message.to_string());
                    });
                })
            });
    }

    fn show_tab_selector(&self, _cfg: &DevfilerConfig) -> bool {
        true
    }
}
