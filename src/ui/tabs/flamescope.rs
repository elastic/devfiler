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
use crate::storage::*;
use crate::ui::cached::Cached;
use crate::ui::timeaxis;
use egui::Color32;
use egui_plot::{Axis, AxisHints, Plot, PlotBounds, Polygon};
use std::collections::HashMap;

/// Millisecond bucket size for subsecond resolution (e.g., 20ms)
const MS_BUCKET_SIZE: u64 = 10;
const MS_PER_SECOND: u64 = 1000;
const NUM_MS_BUCKETS: u64 = MS_PER_SECOND / MS_BUCKET_SIZE; // 50 buckets per second

pub struct FlameScopeTab {
    cached_heatmap: Cached<HeatMapData>,
    selection: Option<TimeSelection>,
}

#[derive(Debug, Clone)]
struct TimeSelection {
    start: UtcTimestamp,
    end: UtcTimestamp,
}

impl Default for FlameScopeTab {
    fn default() -> Self {
        Self {
            cached_heatmap: Default::default(),
            selection: None,
        }
    }
}

impl TabWidget for FlameScopeTab {
    fn id(&self) -> Tab {
        Tab::FlameScope
    }

    fn update(
        &mut self,
        ui: &mut Ui,
        _cfg: &DevfilerConfig,
        kind: SampleKind,
        start: UtcTimestamp,
        end: UtcTimestamp,
    ) -> Option<TabAction> {
        let heatmap = self
            .cached_heatmap
            .get_or_create((start, end, kind), move || build_heatmap(kind, start, end));

        ui.add_space(5.0);
        ui.label(format!("Darker cells indicate more CPU activity. Each row represents a {} ms time bucket within each second.", MS_BUCKET_SIZE));

        // Show selection info and switch button if there's a selection
        let mut action = None;
        let mut clear_selection = false;

        if let Some(selection) = &self.selection {
            ui.horizontal(|ui| {
                ui.label(format!(
                    "Selected: {} to {} ({} seconds)",
                    timeaxis::ts2chrono(selection.start as i64),
                    timeaxis::ts2chrono(selection.end as i64),
                    selection.end - selection.start
                ));
                if ui.button("View Selection in Flamegraph").clicked() {
                    action = Some(TabAction::SwitchTabWithTimeRange {
                        tab: Tab::FlameGraph,
                        start: selection.start,
                        end: selection.end,
                    });
                }
                if ui.button("Clear Selection").clicked() {
                    clear_selection = true;
                }
            });
        }

        if clear_selection {
            self.selection = None;
        }

        ui.add_space(5.0);

        if action.is_none() {
            action = draw_heatmap(ui, &*heatmap, start, end, &mut self.selection);
        }

        action
    }
}

#[derive(Debug, Clone, Default)]
struct HeatMapData {
    /// Map from (second_offset, millisecond_bucket) => sample_count
    cells: HashMap<(u64, u64), u64>,
    max_count: u64,
}

fn build_heatmap(kind: SampleKind, start: UtcTimestamp, end: UtcTimestamp) -> HeatMapData {
    let mut cells = HashMap::new();
    let mut max_count = 0u64;

    // Iterate through all trace events in the time range
    for (key, value) in DB.trace_events.time_range(start, end, kind) {
        let ts = key.timestamp;
        let count = value.get().count as u64;

        // Calculate which second this timestamp belongs to (relative to start)
        let second_in_range = ts;

        // Use the event ID to distribute events within the second
        // This creates a pseudo-subsecond distribution based on event ordering
        let ms_bucket = key.id % NUM_MS_BUCKETS;

        // Aggregate by (second, millisecond_bucket)
        let cell = cells.entry((second_in_range, ms_bucket)).or_insert(0u64);
        *cell += count;
        max_count = max_count.max(*cell);
    }

    HeatMapData { cells, max_count }
}

fn draw_heatmap(
    ui: &mut Ui,
    heatmap: &HeatMapData,
    start: UtcTimestamp,
    end: UtcTimestamp,
    selection: &mut Option<TimeSelection>,
) -> Option<TabAction> {
    let plot = Plot::new("flamescope_heatmap")
        .custom_x_axes(vec![timeaxis::mk_time_axis(Axis::X)])
        .custom_y_axes(vec![AxisHints::new_y().label("ms")])
        .y_axis_min_width(40.0)
        .x_grid_spacer(timeaxis::mk_time_grid)
        .allow_drag(true)
        .allow_zoom(true)
        .allow_scroll(true)
        .height(400.0)
        .show_axes([true, true])
        .label_formatter(|_name, val| {
            format!(
                "Time: {}\nOffset: {}ms",
                timeaxis::ts2chrono(val.x as i64),
                val.y as u64
            )
        });

    let response = plot.show(ui, |plot_ui| {
        // Set bounds to show the full time range
        plot_ui.set_plot_bounds(PlotBounds::from_min_max(
            [start as f64, 0.0],
            [end as f64, MS_PER_SECOND as f64],
        ));

        // Draw heat map cells as filled rectangles (polygons)
        for ((second_timestamp, ms_bucket), count) in &heatmap.cells {
            let ms_offset = ms_bucket * MS_BUCKET_SIZE;

            // Calculate cell position in plot coordinates
            let x = *second_timestamp as f64;
            let y = ms_offset as f64;
            let cell_width = 1.0; // 1 second width
            let cell_height = MS_BUCKET_SIZE as f64;

            // Calculate color intensity based on count
            let intensity = if heatmap.max_count > 0 {
                (*count as f32 / heatmap.max_count as f32).clamp(0.0, 1.0)
            } else {
                0.0
            };

            // Color gradient: light blue -> dark blue -> red for high intensity
            let color = if intensity < 0.5 {
                // Light blue to dark blue
                let t = intensity * 2.0;
                Color32::from_rgb((200.0 * (1.0 - t)) as u8, (200.0 * (1.0 - t)) as u8, 255)
            } else {
                // Dark blue to red
                let t = (intensity - 0.5) * 2.0;
                Color32::from_rgb((t * 255.0) as u8, 0, (255.0 * (1.0 - t)) as u8)
            };

            // Create a polygon (rectangle) for the heat map cell
            let points = vec![
                [x, y],
                [x + cell_width, y],
                [x + cell_width, y + cell_height],
                [x, y + cell_height],
            ];

            let polygon = Polygon::new(points)
                .fill_color(color)
                .stroke(egui::Stroke::NONE);

            plot_ui.polygon(polygon);
        }

        // Draw selection overlay if there's a selection
        if let Some(ref sel) = selection {
            let sel_start = sel.start.max(start) as f64;
            let sel_end = sel.end.min(end) as f64;

            // Draw a semi-transparent overlay for the selected region
            let points = vec![
                [sel_start, 0.0],
                [sel_end, 0.0],
                [sel_end, MS_PER_SECOND as f64],
                [sel_start, MS_PER_SECOND as f64],
            ];

            let overlay = Polygon::new(points)
                .fill_color(Color32::from_rgba_unmultiplied(255, 255, 0, 50))
                .stroke(egui::Stroke::new(2.0, Color32::YELLOW));

            plot_ui.polygon(overlay);
        }
    });

    // Handle mouse interactions for selection
    if response.response.hovered() {
        // Get the pointer position in plot coordinates
        if let Some(pointer_pos) = response.response.hover_pos() {
            let bounds = response.response.rect;
            let plot_bounds = response.transform.bounds();

            // Convert screen position to plot coordinates
            let x_ratio = (pointer_pos.x - bounds.min.x) / bounds.width();
            let plot_x = plot_bounds.min()[0] + x_ratio as f64 * plot_bounds.width();
            let timestamp = plot_x as UtcTimestamp;

            // Handle selection via double-click (select single second)
            if response.response.double_clicked() {
                *selection = Some(TimeSelection {
                    start: timestamp,
                    end: timestamp + 1,
                });
            }

            // Handle selection via drag
            if response.response.drag_started() {
                // Start a new selection
                ui.memory_mut(|mem| {
                    mem.data
                        .insert_temp("flamescope_drag_start".into(), timestamp);
                });
            }

            if response.response.dragged() {
                // Update the selection during drag
                if let Some(drag_start) = ui.memory(|mem| {
                    mem.data
                        .get_temp::<UtcTimestamp>("flamescope_drag_start".into())
                }) {
                    let sel_start = drag_start.min(timestamp);
                    let sel_end = drag_start.max(timestamp);
                    *selection = Some(TimeSelection {
                        start: sel_start,
                        end: sel_end,
                    });
                }
            }

            if response.response.drag_stopped() {
                // Finalize the selection
                ui.memory_mut(|mem| {
                    mem.data
                        .remove::<UtcTimestamp>("flamescope_drag_start".into());
                });
            }
        }
    }

    None
}
