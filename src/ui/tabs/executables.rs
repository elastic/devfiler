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
use crate::storage::{ArchivedSymbStatus, ExecutableMeta, FileId, SymbStatus, Table, DB};
use crate::symbolizer::IngestTask;
use crate::ui::util::{clearable_line_edit, humanize_count};
use egui::emath::RectTransform;
use egui::{
    show_tooltip_at_pointer, Align, Color32, Direction, Id, Layout, Pos2, Rect, Rounding, Sense,
    Stroke, Vec2,
};
use egui_extras::{Column, TableBuilder};
use egui_phosphor::regular as icons;
use std::path::PathBuf;

const NO_NAME: &str = "<none>";

const SYMB_STATUS_BAR_HEIGHT: f32 = 25.0;
const CLR_SYMBOLIZED: Color32 = Color32::from_rgb(0x7a, 0xc7, 0x4f);
const CLR_NO_SYMS: Color32 = Color32::from_rgb(0xff, 0xe7, 0x4c);
const CLR_PENDING: Color32 = Color32::from_rgb(0x4f, 0xc3, 0xf7);
const CLR_TEMP_ERR: Color32 = Color32::from_rgb(0xf2, 0x42, 0x36);

#[derive(Debug, Default, Clone, Copy, Hash, PartialEq)]
enum SortColumn {
    #[default]
    Symbols,
    FileName,
    BuildId,
    FileId,
}

#[derive(Default)]
pub struct ExecutablesTab {
    ingest_queue: Vec<PathBuf>,
    active_ingest_task: Option<IngestTask>,
    filter: String,
    sort_field: SortColumn,
    last_exe_count: usize,
}

impl TabWidget for ExecutablesTab {
    fn id(&self) -> Tab {
        Tab::Executables
    }

    fn update(
        &mut self,
        ui: &mut Ui,
        _cfg: &DevfilerConfig,
        _kind: SampleKind,
        _start: UtcTimestamp,
        _end: UtcTimestamp,
    ) -> Option<TabAction> {
        self.handle_executable_drops(ui.ctx());
        self.draw_sym_status_bar(ui);
        self.draw_symbol_ingest_area(ui);
        self.last_exe_count = self.draw_executable_table(ui);
        None
    }
}

impl ExecutablesTab {
    fn handle_executable_drops(&mut self, ctx: &egui::Context) {
        ctx.input(|i| {
            self.ingest_queue
                .extend(i.raw.dropped_files.iter().filter_map(|x| x.path.clone()))
        });

        if matches!(&self.active_ingest_task, Some(task) if task.done()) {
            if let Err(e) = self.active_ingest_task.take().unwrap().join() {
                tracing::error!("Executable ingestion failed: {e:?}")
            }
        }
    }

    fn draw_symbol_ingest_area(&mut self, ui: &mut Ui) {
        let ingest_status = if let Some(ref active_task) = self.active_ingest_task {
            format!(
                "Processing executable: {} symbols extracted, {} ingested ...",
                active_task.num_ranges_extracted(),
                active_task.num_ranges_ingested()
            )
        } else {
            if let Some(new_task) = self.ingest_queue.pop() {
                self.active_ingest_task = Some(IngestTask::spawn(new_task));
            }

            format!(
                "{} Drop executables anywhere within this tab to ingest symbols!",
                icons::INFO
            )
        };

        ui.separator();
        let bar_size = Vec2::new(ui.available_width(), 20.0);

        // Allocate space for the entire bar
        let (_rect, _) = ui.allocate_space(bar_size);

        // Create the horizontal layout directly inside the main UI
        ui.horizontal(|ui| {
            // Set the width of each column
            let available_width = ui.available_width();
            let col_width = available_width / 3.0;

            // First column - left aligned
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new(col_width, bar_size.y),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.label(format!("{} executables", self.last_exe_count));
                    },
                );
            });

            // Second column - centered
            ui.with_layout(Layout::centered_and_justified(Direction::TopDown), |ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new(col_width, bar_size.y),
                    Layout::centered_and_justified(Direction::TopDown),
                    |ui| {
                        ui.label(ingest_status);
                    },
                );
            });

            // Third column - right aligned
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new(col_width, bar_size.y),
                    Layout::right_to_left(Align::Center),
                    |ui| {
                        let hint = format!("{} Filter ...", icons::FUNNEL);
                        clearable_line_edit(ui, &hint, &mut self.filter);
                    },
                );
            });
        });

        ui.separator();
    }

    fn draw_sym_status_bar(&self, ui: &mut Ui) {
        let mut pending = 0;
        let mut not_present = 0;
        let mut symbolized = 0;
        let mut temp_err = 0;

        for (_, meta) in DB.executables.iter() {
            match meta.get().symb_status {
                ArchivedSymbStatus::NotAttempted => pending += 1,
                ArchivedSymbStatus::TempError { .. } => temp_err += 1,
                ArchivedSymbStatus::NotPresentGlobally => not_present += 1,
                ArchivedSymbStatus::Complete { .. } => symbolized += 1,
            }
        }

        let size = Vec2::new(ui.available_width(), SYMB_STATUS_BAR_HEIGHT);
        let (response, painter) = ui.allocate_painter(size, Sense::hover());

        let trans = RectTransform::from_to(
            Rect::from_min_size(Pos2::ZERO, response.rect.size()),
            response.rect,
        );

        let style = ui.ctx().style();
        let total = pending + not_present + symbolized + temp_err;
        let avail_width = response.rect.width();
        let mut offset = 0.0;
        for (name, value, color) in [
            ("Symbolized", symbolized, CLR_SYMBOLIZED),
            ("No symbols found", not_present, CLR_NO_SYMS),
            ("Pending", pending, CLR_PENDING),
            ("Temporary error", temp_err, CLR_TEMP_ERR),
        ] {
            let width = avail_width * (value as f32 / total as f32);
            let pos = Pos2::new(offset, 0.0);
            let size = Vec2::new(width, SYMB_STATUS_BAR_HEIGHT);
            let rect = trans.transform_rect(Rect::from_min_size(pos, size));

            painter.rect_filled(rect, Rounding::ZERO, color.gamma_multiply(0.8));
            painter.rect_stroke(rect, Rounding::ZERO, Stroke::new(1.0, Color32::BLACK));

            if matches!(response.hover_pos(), Some(p) if rect.contains(p)) {
                let tooltip_id = Id::new("executable-bar-tooltip");
                show_tooltip_at_pointer(
                    ui.ctx(),
                    egui::LayerId::new(egui::Order::Tooltip, tooltip_id),
                    tooltip_id,
                    |ui: &mut Ui| {
                        ui.label(format!("{}: {:.0}", name, humanize_count(value)));
                    },
                );
            }

            offset += width;
        }

        painter.rect_stroke(
            response.rect,
            Rounding::same(1.0),
            style.visuals.widgets.noninteractive.bg_stroke,
        );
    }

    fn draw_executable_table(&mut self, ui: &mut Ui) -> usize {
        let mut exe_count = 0;

        let table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::initial(235.0))
            .column(Column::initial(290.0))
            .column(Column::initial(180.0))
            .column(Column::remainder().clip(true))
            .max_scroll_height(f32::INFINITY);

        table
            .header(20.0, |mut header| {
                for (text, selected_value) in [
                    ("File ID", SortColumn::FileId),
                    ("Build ID", SortColumn::BuildId),
                    ("Symbols", SortColumn::Symbols),
                    ("File Name", SortColumn::FileName),
                ] {
                    header.col(|ui| {
                        ui.selectable_value(&mut self.sort_field, selected_value, text);
                    });
                }
            })
            .body(|mut body| {
                let execs = query_executables(&self.filter, &self.sort_field);

                for (file_id, meta) in execs.iter() {
                    exe_count += 1;
                    let name = meta.file_name.as_deref().unwrap_or(NO_NAME);

                    body.row(20.0, |mut row| {
                        row.col(|ui| {
                            ui.monospace(file_id.format_hex());
                        });
                        row.col(|ui| {
                            ui.monospace(meta.build_id.as_deref().unwrap_or("<none>"));
                        });
                        row.col(|ui| {
                            ui.label(symb_status_text(meta.symb_status));
                        });
                        row.col(|ui| {
                            ui.label(name);
                        });
                    });
                }
            });
        exe_count
    }
}

fn symb_status_text(status: SymbStatus) -> String {
    match status {
        SymbStatus::NotAttempted => "not attempted yet".into(),
        SymbStatus::TempError { .. } => "temporary error".into(),
        SymbStatus::NotPresentGlobally => "not present globally".into(),
        SymbStatus::Complete { num_symbols, .. } => {
            format!("{} symbols", humanize_count(num_symbols))
        }
    }
}

fn query_executables(filter: &String, sort_field: &SortColumn) -> Vec<(FileId, ExecutableMeta)> {
    let mut execs: Vec<_> = DB
        .executables
        .iter()
        .filter_map(|(file_id, value_ref)| {
            let meta = value_ref.read();
            let name = meta.file_name.as_deref().unwrap_or(NO_NAME);
            if name.contains(filter) {
                return Some((file_id, meta));
            }
            None
        })
        .collect();

    // Apply sorting.
    execs.sort_unstable_by(
        |(lhs_file_id, lhs_metas), (rhs_file_id, rhs_metas)| match sort_field {
            SortColumn::Symbols => lhs_metas.symb_status.cmp(&rhs_metas.symb_status).reverse(),
            SortColumn::FileName => {
                let lhs_name = lhs_metas.file_name.as_deref().unwrap_or(NO_NAME);
                let rhs_name = rhs_metas.file_name.as_deref().unwrap_or(NO_NAME);
                lhs_name.cmp(&rhs_name)
            }
            SortColumn::BuildId => lhs_metas.build_id.cmp(&rhs_metas.build_id).reverse(),
            SortColumn::FileId => u128::from(*lhs_file_id).cmp(&u128::from(*rhs_file_id)),
        },
    );

    return execs;
}
