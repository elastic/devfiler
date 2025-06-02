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
use crate::collector::{Collector, LoggedRequest};
use eframe::emath::Align;
use egui::{CollapsingHeader, Label, Layout, RichText, ScrollArea, Sense};
use egui_extras::{Column, TableBuilder};
use egui_phosphor::regular as icons;
use serde_json::{Map as JsonMap, Value as JsonValue};
use std::sync::Arc;
use tonic::metadata::KeyAndValueRef;

#[derive(Default)]
pub struct GrpcLogTab {
    selected_request: Option<Arc<LoggedRequest>>,
}

impl TabWidget for GrpcLogTab {
    fn id(&self) -> Tab {
        Tab::GrpcLog
    }

    fn update(
        &mut self,
        ui: &mut Ui,
        cfg: &DevfilerConfig,
        _kind: SampleKind,
        _start: UtcTimestamp,
        _end: UtcTimestamp,
    ) {
        ui.columns(2, |ui| {
            ui[0].push_id("grpc-msg-list", |ui| self.draw_msg_list(ui, &cfg.collector));
            ui[1].push_id("grpc-msg-inspector", |ui| self.draw_msg_info(ui));
        });
    }

    fn show_tab_selector(&self, cfg: &DevfilerConfig) -> bool {
        cfg.dev_mode
    }
}

impl GrpcLogTab {
    fn draw_msg_list(&mut self, ui: &mut Ui, collector: &Collector) {
        ui.heading(format!("{} Request list", icons::LIST));
        ui.separator();

        let table = TableBuilder::new(ui)
            .striped(true)
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::auto())
            .column(Column::remainder().clip(true))
            .max_scroll_height(f32::INFINITY);

        table
            .header(20.0, |mut header| {
                for column in ["Time", "Kind"] {
                    header.col(|ui| drop(ui.strong(column)));
                }
            })
            .body(|mut body| {
                let ring = collector.stats().ring.read().unwrap();
                for logged_msg in ring.iter().rev() {
                    body.row(20.0, |mut row| {
                        row.col(|ui| {
                            let text = RichText::new(logged_msg.timestamp.to_string()).strong();
                            let label = Label::new(text).sense(Sense::click());
                            let response = ui.add(label);
                            if response.clicked() {
                                self.selected_request = Some(Arc::clone(&logged_msg));
                            }
                        });
                        row.col(|ui| drop(ui.label(logged_msg.kind)));
                    });
                }
            });
    }

    fn draw_msg_info(&self, ui: &mut Ui) {
        let Some(selected) = &self.selected_request else {
            ui.centered_and_justified(|ui| {
                ui.label("<select a message to inspect details>");
            });
            return;
        };

        ui.add_space(20.0);

        ui.push_id("grpc-req-meta", |ui| {
            ui.heading(format!("{} gRPC meta-data", icons::TABLE));
            ui.separator();
            Self::draw_meta_table(ui, selected);
        });

        ui.push_id("grpc-req-payload", |ui| {
            ui.add_space(20.0);
            ui.heading(format!("{} gRPC request payload", icons::TREE_STRUCTURE));
            ui.separator();
            ScrollArea::vertical().show(ui, |ui| {
                Self::recurse_msg_contents(
                    ui,
                    true,
                    &selected.kind,
                    Categorized::new(&selected.payload),
                );
            });
        });
    }

    fn draw_meta_table(ui: &mut Ui, selected: &LoggedRequest) {
        let meta_table = TableBuilder::new(ui)
            .striped(true)
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::auto())
            .column(Column::remainder().clip(true))
            .max_scroll_height(100.0);

        meta_table
            .header(20.0, |mut header| {
                for column in ["Key", "Value"] {
                    header.col(|ui| drop(ui.strong(column)));
                }
            })
            .body(|mut body| {
                for kv in selected.meta.iter() {
                    let (k, v) = match kv {
                        KeyAndValueRef::Ascii(k, v) => (k.as_str(), v.to_str().unwrap_or("<bad>")),
                        KeyAndValueRef::Binary(k, _) => (k.as_str(), "<binary>"),
                    };

                    body.row(20.0, |mut row| {
                        row.col(|ui| drop(ui.monospace(k)));
                        row.col(|ui| drop(ui.monospace(v)));
                    });
                }
            });
    }

    fn recurse_msg_contents(ui: &mut Ui, default_open: bool, key: &str, value: Categorized<'_>) {
        let node_text = format!("{} {}", value.icon(), key);
        let node_text = RichText::new(node_text).monospace();

        CollapsingHeader::new(node_text)
            .default_open(default_open)
            .show(ui, |ui| match value {
                Categorized::Scalar(scalar) => Self::draw_scalar(ui, scalar.as_str()),
                Categorized::Array(array) => Self::draw_array_contents(ui, array),
                Categorized::Object(obj) => Self::draw_obj_contents(ui, obj),
            });
    }

    fn draw_scalar(ui: &mut Ui, scalar: &str) {
        ui.indent(0, |ui| {
            // Intent with same depth as the collapsable header.
            ui.expand_to_include_x(ui.cursor().left() + 40.0);
            ui.monospace(scalar);
        });
    }

    fn draw_array_contents(ui: &mut Ui, array: &Vec<JsonValue>) {
        let key_width = array.len().ilog10() as usize + 1;
        for (i, entry) in array.iter().enumerate() {
            let child = Categorized::new(entry);
            if let Categorized::Scalar(scalar) = &child {
                Self::draw_scalar(ui, &format!("[{i:>key_width$}] = {scalar}"));
            } else {
                Self::recurse_msg_contents(ui, false, &format!("[{i:>key_width$}]"), child);
            }
        }
    }

    fn draw_obj_contents(ui: &mut Ui, obj: &JsonMap<String, JsonValue>) {
        for (k, v) in obj {
            let child = Categorized::new(v);
            if let Categorized::Scalar(scalar) = &child {
                Self::draw_scalar(ui, &format!("{k} = {scalar}"));
            } else {
                Self::recurse_msg_contents(ui, false, k, child);
            }
        }
    }
}

enum Categorized<'v> {
    Scalar(String),
    Array(&'v Vec<JsonValue>),
    Object(&'v JsonMap<String, JsonValue>),
}

impl<'v> Categorized<'v> {
    fn new(value: &'v JsonValue) -> Self {
        match value {
            JsonValue::Null => Categorized::Scalar("<null>".to_owned()),
            JsonValue::Bool(x) => Categorized::Scalar(format!("{x:?}")),
            JsonValue::Number(x) => Categorized::Scalar(format!("{x}")),
            JsonValue::String(x) => Categorized::Scalar(format!("{x:?}")),
            JsonValue::Array(x) if x.is_empty() => Categorized::Scalar("<empty>".to_owned()),
            JsonValue::Array(x) => Categorized::Array(x),
            JsonValue::Object(x) if x.is_empty() => Categorized::Scalar("<empty>".to_owned()),
            JsonValue::Object(x) => Categorized::Object(x),
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            Categorized::Scalar(_) => icons::ASTERISK_SIMPLE,
            Categorized::Array(_) => icons::BRACKETS_SQUARE,
            Categorized::Object(_) => icons::BRACKETS_CURLY,
        }
    }
}
