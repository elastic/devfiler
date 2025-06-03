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
use crate::storage::{Table, DB};
use crate::ui::cached::Cached;
use egui::ScrollArea;
use egui_plot::{AxisHints, Bar, BarChart, Plot};
use itertools::Itertools;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

#[derive(Default)]
pub struct TraceFreqTab {
    global_cache: Cached<Vec<Bar>>,
    global_frame_dedup_rate: Cached<f64>,
    global_dedup_rate_cache: Cached<f64>,
    global_no_leaf_dedup_rate_cache: Cached<f64>,
    global_no_leafs_cache: Cached<Vec<Bar>>,
    local_cache: Cached<Vec<Bar>>,
}

impl TabWidget for TraceFreqTab {
    fn id(&self) -> Tab {
        Tab::TraceFreq
    }

    fn update(
        &mut self,
        ui: &mut Ui,
        _cfg: &DevfilerConfig,
        _kind: SampleKind,
        start: UtcTimestamp,
        end: UtcTimestamp,
    ) {
        ScrollArea::vertical().show(ui, |ui| {
            ui.collapsing("Deduplication rates", |ui| {
                if !ui.is_visible() {
                    return;
                }

                ui.horizontal(|ui| {
                    ui.strong("Global dedup rate (w/ leafs):");
                    self.draw_global_dedup_rate(ui, start, end);
                });
                ui.horizontal(|ui| {
                    ui.strong("Global dedup rate (w/o leafs):");
                    self.draw_global_no_leaf_dedup_rate(ui, start, end);
                });
                ui.horizontal(|ui| {
                    ui.strong("Global frame dedup rate:");
                    self.draw_global_frame_dedup_rate(ui, start, end);
                });
            });

            ui.collapsing("Deduplication plots", |ui| {
                if !ui.is_visible() {
                    return;
                }

                self.draw_global_freq(ui, start, end);
                self.draw_global_freq_without_leaf(ui, start, end);
                self.draw_per_event_freq(ui, start, end);
            });
        });
    }

    fn show_tab_selector(&self, cfg: &DevfilerConfig) -> bool {
        cfg.dev_mode
    }
}

impl TraceFreqTab {
    fn trace_freq_hist(ui: &mut Ui, title: &str, descr: &str, bars: &Vec<Bar>) {
        ui.add_space(10.0);
        ui.heading(title);
        ui.label(descr);
        ui.add_space(15.0);

        Plot::new(title)
            .custom_y_axes(vec![AxisHints::new_y().label("Trace Count")])
            .custom_x_axes(vec![AxisHints::new_x().label("Times Count Seen")])
            .clamp_grid(true)
            .height(600.0)
            .label_formatter(|_, point| {
                format!(
                    "Trace Count: {:.0}\nTimes Count Seen: {:.0}",
                    point.x, point.y
                )
            })
            .show(ui, |pui| {
                pui.bar_chart(BarChart::new(bars.clone()));
                pui.plot_bounds()
            });
    }

    fn draw_global_frame_dedup_rate(
        &mut self,
        ui: &mut Ui,
        start: UtcTimestamp,
        end: UtcTimestamp,
    ) {
        let value = self
            .global_frame_dedup_rate
            .get_or_create((start, end), move || {
                let aggr = DB
                    .trace_events
                    .time_range(start, end)
                    .flat_map(|(_, tc)| {
                        let tc = tc.get();
                        let Some(trace) = DB.stack_traces.get(tc.trace_hash) else {
                            return vec![];
                        };

                        trace
                            .get()
                            .iter()
                            .map(|frame| {
                                (
                                    (u128::from(frame.id.file_id), frame.id.addr_or_line),
                                    tc.count,
                                )
                            })
                            .collect_vec()
                    })
                    .into_grouping_map_by(|(id, _)| *id)
                    .fold(0, |acc, _, (_, count)| acc + count as u64);

                let count = aggr.len();
                let sum: u64 = aggr.values().cloned().sum();

                sum as f64 / count as f64
            });

        ui.label(format!("{:.02}:1", *value));
    }

    fn draw_global_dedup_rate(&mut self, ui: &mut Ui, start: UtcTimestamp, end: UtcTimestamp) {
        let value = self
            .global_dedup_rate_cache
            .get_or_create((start, end), move || {
                let events = DB.trace_events.sample_events(start, end);
                let count = events.len();
                let sum: u64 = events.values().map(|x| x.count).sum();
                sum as f64 / count as f64
            });

        ui.label(format!("{:.02}:1", *value));
    }

    fn draw_global_no_leaf_dedup_rate(
        &mut self,
        ui: &mut Ui,
        start: UtcTimestamp,
        end: UtcTimestamp,
    ) {
        let value = self
            .global_no_leaf_dedup_rate_cache
            .get_or_create((start, end), move || {
                let trace_counts = DB
                    .trace_events
                    .time_range(start, end)
                    // Aggregation #1: rehash trace counts without leaf and sum(count)
                    .into_grouping_map_by(|(_, tc)| {
                        // Query corresponding trace.
                        let hash = tc.get().trace_hash;
                        let Some(trace) = DB.stack_traces.get(hash) else {
                            return 0;
                        };

                        // Rehash without leaf.
                        let trace = trace.get();
                        let mut hasher = DefaultHasher::default();
                        for frame_id in trace.iter().skip(1 /* leaf */) {
                            frame_id.id.file_id.hash(&mut hasher);
                            frame_id.id.addr_or_line.hash(&mut hasher);
                        }
                        hasher.finish()
                    })
                    .fold(0, |acc, _, (_, tc)| acc + tc.get().count);

                let count = trace_counts.len();
                let sum: u64 = trace_counts.values().map(|&x| x as u64).sum();
                sum as f64 / count as f64
            });

        ui.label(format!("{:.02}:1", *value));
    }

    fn draw_global_freq(&mut self, ui: &mut Ui, start: UtcTimestamp, end: UtcTimestamp) {
        let bars = self.global_cache.get_or_create((start, end), move || {
            DB.trace_events
                .sample_events(start, end)
                .into_iter()
                .into_grouping_map_by(|x| x.1.count)
                .fold(0, |acc, _, _| acc + 1)
                .into_iter()
                .sorted_by_key(|(trace_count, _)| *trace_count)
                .into_iter()
                .map(|(trace_count, count_seen)| Bar::new(trace_count as f64, count_seen as f64))
                .collect()
        });

        Self::trace_freq_hist(
            ui,
            "Global Trace Frequency",
            "Frequencies of counts for each trace in the entire selected time range.",
            &*bars,
        );
    }

    fn draw_per_event_freq(&mut self, ui: &mut Ui, start: UtcTimestamp, end: UtcTimestamp) {
        let bars = self.local_cache.get_or_create((start, end), move || {
            DB.trace_events
                .time_range(start, end)
                .into_grouping_map_by(|(_, tc)| tc.get().count)
                .fold(0, |acc, _, _| acc + 1)
                .into_iter()
                .sorted_by_key(|(_, count_seen)| *count_seen)
                .map(|(trace_count, count_seen)| Bar::new(trace_count as f64, count_seen as f64))
                .collect()
        });

        Self::trace_freq_hist(
            ui,
            "Event Trace Frequency",
            "Frequencies of counts within an individual trace event.",
            &*bars,
        );
    }

    fn draw_global_freq_without_leaf(
        &mut self,
        ui: &mut Ui,
        start: UtcTimestamp,
        end: UtcTimestamp,
    ) {
        let bars = self
            .global_no_leafs_cache
            .get_or_create((start, end), move || {
                DB.trace_events
                    .time_range(start, end)
                    // Aggregation #1: rehash trace counts without leaf and sum(count)
                    .into_grouping_map_by(|(_, tc)| {
                        // Query corresponding trace.
                        let hash = tc.get().trace_hash;
                        let Some(trace) = DB.stack_traces.get(hash) else {
                            return 0;
                        };

                        // Rehash without leaf.
                        let trace = trace.get();
                        let mut hasher = DefaultHasher::default();
                        for frame_id in trace.iter().skip(1 /* leaf */) {
                            frame_id.id.file_id.hash(&mut hasher);
                            frame_id.id.addr_or_line.hash(&mut hasher);
                        }
                        hasher.finish()
                    })
                    .fold(0, |acc, _, (_, tc)| acc + tc.get().count)
                    .into_iter()
                    // Aggregation #2: group by count and count how often we've seen each count
                    .into_grouping_map_by(|(_no_leaf_hash, count)| *count)
                    .fold(0, |acc, _, _| acc + 1)
                    .into_iter()
                    // Sort descending by count.
                    .sorted_by_key(|(_trace_count, count_seen)| *count_seen)
                    // Create bar chart bars.
                    .map(|(trace_count, count_seen)| {
                        Bar::new(trace_count as f64, count_seen as f64)
                    })
                    .collect_vec()
            });

        Self::trace_freq_hist(
            ui,
            "Global Trace Frequency (no leafs)",
            "Frequencies of counts for each trace in the entire selected time range (no leafs).",
            &bars,
        );
    }
}
