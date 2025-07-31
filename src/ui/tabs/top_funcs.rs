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
use crate::storage::{symbolize_frame, FrameKind, Table, DB};
use crate::ui::cached::Cached;
use crate::ui::util::{
    clearable_line_edit, draw_heat_map, frame_kind_color, humanize_count, plot_color,
};
use egui::{Align, Color32, Layout, Sense, Stroke};
use egui_extras::{Column, TableBuilder};
use egui_phosphor::regular as icons;
use nohash_hasher::IntSet;
use std::cmp::min;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::iter;
use std::iter::FusedIterator;
use std::sync::mpsc;

/// Maximum length of the top function table.
const MAX_LOCATIONS: usize = 500;

#[derive(Default)]
pub struct TopFuncsTab {
    sort_field: SortField,
    cache: Cached<TopFuncs>,
    filter: String,
}

impl TabWidget for TopFuncsTab {
    fn id(&self) -> Tab {
        Tab::TopFunctions
    }

    fn update(
        &mut self,
        ui: &mut Ui,
        _cfg: &DevfilerConfig,
        _kind: SampleKind,
        start: UtcTimestamp,
        end: UtcTimestamp,
    ) {
        let sort_field = self.sort_field;
        let filter = self.filter.clone();

        let TopFuncs {
            total_funcs,
            total_samples,
            ref top,
        } = *self
            .cache
            .get_or_create((start, end, sort_field, &self.filter), move || {
                query_top_funcs(start, end, sort_field, filter)
            });

        ui.add_space(5.0);
        ui.columns(2, |ui| {
            ui[0].with_layout(Layout::left_to_right(Align::Min), |ui| {
                ui.label(if total_funcs > top.len() {
                    format!(
                        "{} functions total. List truncated to {} entries.",
                        total_funcs,
                        top.len(),
                    )
                } else {
                    format!("{} functions", top.len())
                });
            });
            ui[1].with_layout(Layout::right_to_left(Align::Min), |ui| {
                let hint = format!("{} Filter ...", icons::FUNNEL);
                clearable_line_edit(ui, &hint, &mut self.filter);
            });
        });
        ui.separator();

        let table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::exact(85.0))
            .column(Column::exact(85.0))
            .column(Column::exact(85.0))
            .column(Column::exact(85.0))
            .column(Column::initial(300.0).clip(true))
            .column(Column::initial(300.0).clip(true))
            .column(Column::remainder().clip(true))
            .max_scroll_height(f32::INFINITY);

        table
            .header(20.0, |mut header| {
                for _ in 0..2 {
                    header.col(|ui| {
                        #[rustfmt::skip]
                        ui.selectable_value(
                            &mut self.sort_field,
                            SortField::Zelf,
                            "Self",
                        );
                    });
                    header.col(|ui| {
                        ui.selectable_value(
                            &mut self.sort_field,
                            SortField::WithChildren,
                            "With Children",
                        );
                    });
                }
                for misc_col in ["Function", "Source File", "Heat Map"] {
                    header.col(|ui| drop(ui.strong(misc_col)));
                }
            })
            .body(|mut body| {
                for (location, counts) in top {
                    // Intentionally doing double filtering: this filter here
                    // ensures quick response time while the new query is still
                    // running in the background, the other one in the query
                    // makes sure that user can also search for functions that
                    // would otherwise be truncated away by our function limit.
                    if !location.matches_filter(&self.filter) {
                        continue;
                    }

                    body.row(20.0, |mut row| {
                        // Self (%)
                        row.col(|ui| {
                            let ratio = counts.zelf as f32 / total_samples.max(1) as f32;
                            draw_percent_column(ui, ratio);
                        });
                        // With children (%)
                        row.col(|ui| {
                            let ratio = counts.with_children as f32 / total_samples.max(1) as f32;
                            draw_percent_column(ui, ratio);
                        });
                        // Self (count)
                        row.col(|ui| {
                            draw_count_column(ui, counts.zelf);
                        });
                        // With children (count)
                        row.col(|ui| {
                            draw_count_column(ui, counts.with_children);
                        });
                        // Function name
                        row.col(|ui| {
                            ui.add_space(3.0);
                            draw_frame_type_square(ui, location.kind);
                            ui.label(&location.func);
                        });
                        // File name
                        row.col(|ui| {
                            if let Some(ref file) = location.file {
                                ui.label(file);
                            }
                        });
                        // Heat map
                        row.col(|ui| {
                            draw_func_heatmap(ui, &counts);
                        });
                    });
                }
            });
    }
}

/// Draws an humanized count column.
fn draw_count_column(ui: &mut Ui, count: u64) {
    let layout = Layout::right_to_left(Align::Center);
    let text = humanize_count(count).to_string();
    ui.with_layout(layout, |ui| ui.label(text));
}

/// Draws a percentage column field, with a "progress bar" in the background.
fn draw_percent_column(ui: &mut Ui, perc: f32) {
    if perc > 0.00_1 {
        let mut rect = ui.available_rect_before_wrap();
        rect = rect.shrink(3.0);
        rect.set_width(rect.width() * perc);
        let painter = ui.painter_at(rect);
        let color = ui.visuals().selection.bg_fill;
        painter.rect(rect, 0.0, color, Stroke::NONE);
    }

    let text = format!("{:.02}%", perc * 100.0);
    let num_col_layout = Layout::right_to_left(Align::Center);
    ui.with_layout(num_col_layout, |ui| ui.label(text));
}

/// Draw a little square for the frame kind color.
fn draw_frame_type_square(ui: &mut Ui, kind: FrameKind) {
    let color = frame_kind_color(kind);
    let stroke_color = color.gamma_multiply(0.5).to_opaque();
    let size = [10.0, 10.0].into();
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect(rect, 0.0, color, Stroke::new(1.0, stroke_color));
}

/// Draw a heatmap visualizing when within the filter period the function was invoked.
fn draw_func_heatmap(ui: &mut Ui, counts: &Counts) {
    let self_color = plot_color(0);
    let with_children_color = ui.visuals().selection.bg_fill;

    let iter = counts
        .heatmap_self
        .bits()
        .zip(counts.heatmap_with_children.bits())
        .map(|(zelf, with_children)| match (zelf, with_children) {
            (true, true | false) => self_color,
            (false, true) => with_children_color,
            (false, false) => Color32::TRANSPARENT,
        })
        .map(iter::once);

    draw_heat_map(ui, 1, HEATMAP_BITS, iter);
}

#[derive(Debug, Default, Clone, Copy, Hash, PartialEq)]
enum SortField {
    Zelf,
    #[default]
    WithChildren,
}

#[derive(Debug, Default)]
struct TopFuncs {
    /// Total number of functions before truncation.
    pub total_funcs: usize,
    /// Total number of samples before truncation.
    pub total_samples: u64,
    /// Truncated list top functions.
    pub top: Vec<(Location, Counts)>,
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct Location {
    pub kind: FrameKind,
    pub func: String,
    pub file: Option<String>,
}

impl Location {
    fn matches_filter(&self, filter: &str) -> bool {
        if self.func.contains(filter) {
            return true;
        }

        self.file.as_ref().map_or(false, |x| x.contains(filter))
    }
}

const HEATMAP_BITS: usize = 256;

/// Minimal fixed-size bit vector implementation.
#[derive(Debug)]
struct BitVec(pub [u8; HEATMAP_BITS / 8]);

impl Default for BitVec {
    fn default() -> Self {
        BitVec([0; HEATMAP_BITS / 8])
    }
}

impl BitVec {
    /// Set the nth bit.
    ///
    /// # Panics
    ///
    /// If `n` is out of bounds.
    pub fn set(&mut self, n: usize) {
        self.0[n / 8] |= 1 << (n % 8);
    }

    /// Set bit at percentage position (`0.0`..`1.0`).
    pub fn set_f64(&mut self, pos: f64) {
        let p = pos.clamp(0.0, 1.0) * (HEATMAP_BITS as f64 - 1.0);
        self.set(min(p as usize, HEATMAP_BITS - 1))
    }

    /// Iterate over the individual bits in this bitvec.
    pub fn bits(&self) -> impl FusedIterator<Item = bool> + '_ {
        let expand = |byte| (0..8).map(move |x| byte & (1 << x) != 0);
        self.0.iter().flat_map(expand)
    }
}

#[derive(Debug, Default)]
struct Counts {
    pub zelf: u64,
    pub with_children: u64,
    pub heatmap_self: BitVec,
    pub heatmap_with_children: BitVec,
}

fn query_top_funcs(
    start: UtcTimestamp,
    end: UtcTimestamp,
    sort_field: SortField,
    filter: String,
) -> TopFuncs {
    let Some(duration) = end.checked_sub(start) else {
        return TopFuncs::default();
    };

    // Thread 1: pull events from the table.
    let (event_tx, event_rx) = mpsc::sync_channel(4096);
    let table_task = tokio::task::spawn_blocking(move || {
        let mut total_samples = 0;
        for (id, trace) in DB.trace_events.time_range(start, end, SampleKind::Mixed) {
            let trace = trace.get();
            total_samples += u64::from(trace.count);

            event_tx
                .send((id.timestamp, trace.trace_hash, trace.count))
                .expect("should never be closed on RX side (1)");
        }
        total_samples
    });

    // Thread 2: pull in the frame lists for the events.
    let (raw_frame_tx, raw_frame_rx) = mpsc::sync_channel(4096);
    let frame_task = tokio::task::spawn_blocking(move || {
        for (ts, trace_hash, count) in event_rx.iter() {
            let Some(trace) = DB.stack_traces.get(trace_hash) else {
                continue;
            };

            for (frame_idx, frame) in trace.get().iter().enumerate() {
                raw_frame_tx
                    .send((ts, *frame, frame_idx, count))
                    .expect("should never be closed on RX side (2)");
            }
        }
    });

    // Thread 3: pull in symbols.
    let (frame_tx, frame_rx) = mpsc::sync_channel(4096);
    let symb_task = tokio::task::spawn_blocking(move || {
        let mut cache = lru::LruCache::new((16 * 1024).try_into().unwrap());
        let mut new_trace = false;

        for (ts, frame, frame_idx, count) in raw_frame_rx {
            if frame_idx == 0 {
                new_trace = true;
            }

            for inline in cache
                .get_or_insert(frame, || symbolize_frame(frame.into(), true))
                .iter()
                .rev()
            {
                let Some(ref func) = inline.func else {
                    // Can't reasonably represent frames without function in this view.
                    continue;
                };

                let location = Location {
                    kind: frame.kind,
                    func: func.to_owned(),
                    file: inline.file.clone(),
                };

                if !filter.is_empty() && !location.matches_filter(&filter) {
                    continue;
                }

                frame_tx
                    .send((ts, location, std::mem::take(&mut new_trace), count))
                    .expect("should never be closed on RX side (3)");
            }
        }
    });

    // Thread 4 (this one): deduplicate equal frames within each trace and
    // aggregate the counts. The deduplication is required to ensure that
    // we don't account a sample to a location more than once when it occurs
    // more than once within a single trace.
    let mut aggr = HashMap::<_, Counts>::with_capacity(16 * 1024);
    let mut seen_this_trace = IntSet::with_capacity_and_hasher(128, Default::default());

    for (ts, top_func, new_trace_start, count) in frame_rx {
        // Entering next trace? Reset seen traces.
        if new_trace_start {
            seen_this_trace.clear();
        }

        // Trace seen before? Skip. The leaf frame comes first, so it will never
        // exit here. That ensures that we always increment the self count.
        if !seen_this_trace.insert(hash(&top_func)) {
            continue;
        }

        // Update counts and heat-map.
        let counts = aggr.entry(top_func).or_default();
        let hm_pos = (ts - start) as f64 / duration as f64;

        counts.with_children += u64::from(count);
        counts.heatmap_with_children.set_f64(hm_pos);

        if new_trace_start {
            counts.zelf += u64::from(count);
            counts.heatmap_self.set_f64(hm_pos);
        }
    }

    // Check whether any of our tasks died.
    let async_rt = tokio::runtime::Handle::current();
    let total_samples = async_rt.block_on(async {
        frame_task.await.expect("frame task panicked");
        symb_task.await.expect("symb task panicked");
        table_task.await.expect("table task panicked")
    });

    // Apply sorting.
    let mut top: Vec<_> = aggr.into_iter().collect();
    top.sort_unstable_by(|(lhs_loc, lhs_counts), (rhs_loc, rhs_counts)| {
        let (lhs_count, rhs_count) = match sort_field {
            SortField::Zelf => (lhs_counts.zelf, rhs_counts.zelf),
            SortField::WithChildren => (lhs_counts.with_children, rhs_counts.with_children),
        };

        lhs_count
            .cmp(&rhs_count)
            .reverse()
            .then_with(|| lhs_loc.func.cmp(&rhs_loc.func))
            .then_with(|| lhs_loc.file.cmp(&rhs_loc.file))
    });

    // Truncate to reduce memory use after construction.
    let total_funcs = top.len();
    top.truncate(MAX_LOCATIONS);
    top.shrink_to_fit();

    TopFuncs {
        total_funcs,
        total_samples,
        top,
    }
}

fn hash(location: impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    location.hash(&mut hasher);
    hasher.finish()
}
