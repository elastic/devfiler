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
use crate::collector::Collector;
use crate::storage::dbtypes::UtcTimestamp;
use crate::storage::{RawTable, SampleKind, DB};
use crate::ui::cached::Cached;
use crate::ui::tabs::{Tab, TabWidget};
use chrono::Duration;
use eframe::egui::{Align, Layout};
use eframe::{egui, egui::Ui};
use egui::{Image, Label, Pos2, Rect, RichText, SelectableLabel, Sense, Vec2, Widget};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use egui_plot::{Axis, AxisHints, Line, Plot, PlotBounds};

#[derive(Debug)]
pub struct DevfilerConfig {
    pub dev_mode: bool,
    pub collector: Collector,
}

pub struct DevfilerUi {
    active_tab: Tab,
    tabs: Vec<Box<dyn TabWidget>>,
    sample_agg_cache: Cached<Vec<[f64; 2]>>,
    cfg: DevfilerConfig,
    show_add_data_window: bool,
    md_cache: CommonMarkCache,
    auto_scroll_time: Option<Duration>,
    kind: SampleKind,
    requested_time_range: Option<(UtcTimestamp, UtcTimestamp)>,
}

impl eframe::App for DevfilerUi {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.draw_main_window(ctx);

        if self.show_add_data_window {
            self.draw_add_data_window(ctx);
        }
    }
}

impl DevfilerUi {
    pub fn new(collector: Collector) -> Self {
        DevfilerUi {
            active_tab: Tab::FlameGraph,
            tabs: vec![
                Box::new(tabs::FlameGraphTab::default()),
                Box::new(tabs::FlameScopeTab::default()),
                Box::new(tabs::TopFuncsTab::default()),
                Box::new(tabs::ExecutablesTab::default()),
                Box::new(tabs::LogTab::default()),
                // Keep dev mode tabs below.
                Box::new(tabs::TraceFreqTab::default()),
                Box::new(tabs::DbStatsTab::default()),
                Box::new(tabs::GrpcLogTab::default()),
            ],
            sample_agg_cache: Cached::default(),
            cfg: DevfilerConfig {
                collector,
                #[cfg(feature = "default-dev-mode")]
                dev_mode: true,
                #[cfg(not(feature = "default-dev-mode"))]
                dev_mode: false,
            },
            show_add_data_window: DB.stack_traces.count_estimate() == 0,
            md_cache: CommonMarkCache::default(),
            auto_scroll_time: Some(Duration::try_minutes(15).unwrap()),
            kind: SampleKind::Mixed,
            requested_time_range: None,
        }
    }

    fn draw_main_window(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.columns(2, |ui| {
                ui[0].horizontal(|ui| {
                    let logo = Image::new(egui::include_image!("../../assets/icon.png"));
                    let logo_interaction = ui.add(logo.sense(Sense::click()));

                    #[cfg(feature = "allow-dev-mode")]
                    if logo_interaction.double_clicked() {
                        self.cfg.dev_mode = !self.cfg.dev_mode;
                    }

                    #[cfg(not(feature = "allow-dev-mode"))]
                    let _ = logo_interaction;

                    let heading = RichText::new("devfiler").heading();
                    Label::new(heading).ui(ui);

                    self.tab_selector(ui);
                });
                ui[1].with_layout(Layout::right_to_left(Align::Min), |ui| {
                    self.sample_selector(ui);
                    self.time_selector(ui)
                });
            });

            let (data_start, data_end) = self.samples_widget(ui);

            if let Some(active_tab) = self.tabs.iter_mut().find(|t| t.id() == self.active_tab) {
                ui.push_id(active_tab.id(), |ui| {
                    let action = active_tab.update(ui, &self.cfg, self.kind, data_start, data_end);

                    // Handle any tab action returned
                    if let Some(tabs::TabAction::SwitchTabWithTimeRange { tab, start, end }) =
                        action
                    {
                        self.active_tab = tab;
                        // Disable auto-scroll when switching with a specific time range
                        self.auto_scroll_time = None;
                        // Set the requested time range for the next frame
                        self.requested_time_range = Some((start, end));
                        ctx.request_repaint();
                    }
                });
            }
        });
    }

    fn draw_add_data_window(&mut self, ctx: &egui::Context) {
        const DEFAULT_WIDTH: f32 = 800.0;
        const DEFAULT_HEIGHT: f32 = 600.0;

        let mut still_open = true;

        let screen_rect = ctx.available_rect();
        let default_rect = Rect::from_min_size(
            Pos2::new(screen_rect.center().x - DEFAULT_WIDTH / 2.0, 50.0),
            Vec2::new(DEFAULT_WIDTH, DEFAULT_HEIGHT),
        );

        egui::Window::new("Adding data")
            .collapsible(true)
            .default_rect(default_rect)
            .open(&mut still_open)
            .show(ctx, |ui| {
                ui.vertical(|ui| self.draw_add_data_window_contents(ui));
            });

        if !still_open {
            self.show_add_data_window = false;
        }
    }

    fn draw_add_data_window_contents(&mut self, ui: &mut Ui) {
        static ADD_DATA_MD: &str = include_str!("./add-data.md");

        egui::ScrollArea::vertical().show(ui, |ui| {
            CommonMarkViewer::new().show(ui, &mut self.md_cache, ADD_DATA_MD);
        });
    }

    fn samples_widget(&mut self, ui: &mut Ui) -> (UtcTimestamp, UtcTimestamp) {
        let plot = Plot::new("trace_counts")
            .custom_x_axes(vec![timeaxis::mk_time_axis(Axis::X)])
            .custom_y_axes(vec![AxisHints::new_y().label("Samples")])
            .y_axis_min_width(2.0)
            .x_grid_spacer(timeaxis::mk_time_grid)
            .allow_drag([true, false])
            .height(100.0)
            .label_formatter(|_, val| {
                format!(
                    "Time: {}\nSamples: {:.0}",
                    timeaxis::ts2chrono(val.x as i64),
                    val.y
                )
            });

        let response = plot.show(ui, |pui| {
            let data_start;
            let data_end;

            // If there's a requested time range (from tab action), use it
            if let Some((req_start, req_end)) = self.requested_time_range.take() {
                data_start = req_start;
                data_end = req_end;

                pui.set_plot_bounds(PlotBounds::from_min_max(
                    [data_start as f64, -f64::MIN],
                    [data_end as f64, f64::MAX],
                ));
            } else if let Some(new_lookback) = self.auto_scroll_time {
                let now = chrono::Utc::now();

                data_start = (now - new_lookback).timestamp() as UtcTimestamp;
                data_end = now.timestamp() as UtcTimestamp;

                pui.set_plot_bounds(PlotBounds::from_min_max(
                    [data_start as f64, -f64::MIN],
                    [data_end as f64, f64::MAX],
                ));
            } else {
                let bounds = pui.plot_bounds();
                data_start = bounds.min()[0] as UtcTimestamp;
                data_end = bounds.max()[0] as UtcTimestamp;
            }

            let kind = self.kind.clone();
            let points =
                self.sample_agg_cache
                    .get_or_create((kind, data_start, data_end), move || {
                        DB.trace_events
                            .event_count_buckets(kind, data_start, data_end, 1000)
                            .into_iter()
                            .map(|(time, count)| [time as f64, count as f64])
                            .collect()
                    });

            pui.line(Line::new(points.clone()));
            pui.set_auto_bounds([false, true].into());

            (data_start as UtcTimestamp, data_end as UtcTimestamp)
        });

        // Manually dragged/scrolled/pinched -> disable auto-updates.
        if response.response.hovered() {
            let (scroll, zoom) = ui.input(|x| (x.raw_scroll_delta, x.zoom_delta_2d()));
            if response.response.dragged() || scroll != Vec2::ZERO || zoom != [1.0, 1.0].into() {
                self.auto_scroll_time = None;
            }
        }

        response.inner
    }

    fn tab_selector(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            for tab in &self.tabs {
                if !tab.show_tab_selector(&self.cfg) {
                    continue;
                }

                let id = tab.id();
                ui.selectable_value(&mut self.active_tab, id, id.to_string());
            }

            if ui
                .selectable_label(self.show_add_data_window, "Add data")
                .clicked()
            {
                self.show_add_data_window = !self.show_add_data_window;
            }
        });
    }

    fn time_selector(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            for (text, duration) in [
                ("15m", Duration::try_minutes(15).unwrap()),
                ("1h", Duration::try_hours(1).unwrap()),
                ("24h", Duration::try_days(1).unwrap()),
            ] {
                let is_active = self.auto_scroll_time == Some(duration);
                let label = SelectableLabel::new(is_active, text);
                let response = label.ui(ui);
                if response.clicked() {
                    if is_active {
                        self.auto_scroll_time = None;
                    } else {
                        self.auto_scroll_time = Some(duration);
                    }
                }
            }

            ui.label("Last:");
        });
    }

    fn sample_selector(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            egui::ComboBox::new("sample_kind", "")
                .selected_text(format!("{:?}", self.kind))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.kind, SampleKind::Mixed, "Mixed");
                    ui.selectable_value(&mut self.kind, SampleKind::OnCPU, "On CPU");
                    ui.selectable_value(&mut self.kind, SampleKind::OffCPU, "Off CPU");
                    ui.selectable_value(&mut self.kind, SampleKind::UProbe, "UProbe");
                });

            ui.label("Sample kind:");
        });
    }
}
