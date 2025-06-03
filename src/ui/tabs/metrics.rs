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
use crate::storage::{metric_spec_by_id, AggregatedMetric, MetricId, MetricKind, DB};
use crate::ui::cached::Cached;
use crate::ui::timeaxis;
use egui::{Align, Layout, Slider};
use egui_plot::{Axis, AxisHints, Legend, Line, Plot, PlotPoints};
use itertools::Itertools;

pub struct MetricsTab {
    filter: String,
    buckets: usize,
    cached_metrics: Cached<Vec<(MetricId, Vec<(UtcTimestamp, AggregatedMetric)>)>>,
}

impl Default for MetricsTab {
    fn default() -> Self {
        Self {
            filter: "".to_string(),
            buckets: 500,
            cached_metrics: Cached::default(),
        }
    }
}

impl TabWidget for MetricsTab {
    fn id(&self) -> Tab {
        Tab::Metrics
    }

    fn update(
        &mut self,
        ui: &mut Ui,
        _cfg: &DevfilerConfig,
        _kind: SampleKind,
        start: UtcTimestamp,
        end: UtcTimestamp,
    ) {
        let buckets = self.buckets;
        let histograms = self
            .cached_metrics
            .get_or_create((start, end, buckets), move || {
                DB.metrics
                    .histograms(start, end, buckets)
                    .into_iter()
                    .sorted_by_key(|x| x.0)
                    .collect()
            });

        ui.separator();

        ui.columns(2, |ui| {
            ui[0].with_layout(Layout::left_to_right(Align::Min), |ui| {
                ui.label("Filter");
                ui.text_edit_singleline(&mut self.filter);
            });
            ui[1].with_layout(Layout::right_to_left(Align::Min), |ui| {
                ui.add(Slider::new(&mut self.buckets, 5..=1000));
                ui.label("Aggregation buckets");
            });
        });

        ui.separator();

        Plot::new("metrics")
            .custom_x_axes(vec![timeaxis::mk_time_axis(Axis::X)])
            .custom_y_axes(vec![AxisHints::new_y().label("Value")])
            .y_axis_width(5)
            .x_grid_spacer(timeaxis::mk_time_grid)
            .legend(Legend::default())
            .label_formatter(|name, val| {
                let maybe_name = if !name.is_empty() {
                    format!("Metric: {name}\n")
                } else {
                    String::new()
                };

                format!(
                    "{}Time: {}\nValue: {:.0}",
                    maybe_name,
                    timeaxis::ts2chrono(val.x as i64),
                    val.y
                )
            })
            .show(ui, |pui| {
                for (metric_id, histogram) in &*histograms {
                    let Some(spec) = metric_spec_by_id(*metric_id) else {
                        // TODO: some sane fallback?
                        continue;
                    };

                    let points = histogram
                        .iter()
                        .map(|(time, aggr)| {
                            let value = match spec.kind {
                                MetricKind::Counter => aggr.sum(),
                                MetricKind::Gauge => aggr.avg(),
                            };

                            [*time as f64, value as f64]
                        })
                        .collect::<PlotPoints>();

                    let field = spec
                        .field
                        .as_ref()
                        .map_or_else(|| format!("M:{}", metric_id), |x| x.to_string());

                    if !field.contains(&self.filter) {
                        continue;
                    }

                    pui.line(Line::new(points).name(field));
                }
            });
    }

    fn show_tab_selector(&self, cfg: &DevfilerConfig) -> bool {
        cfg.dev_mode
    }
}
