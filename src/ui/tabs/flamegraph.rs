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
use crate::ui::util::{clearable_line_edit_with_status, frame_kind_color, humanize_count};
use base64::Engine;
use egui::emath::RectTransform;
use egui::Stroke;
use egui::{
    pos2, show_tooltip_at_pointer, vec2, Align, Align2, Color32, FontId, Id, Key, Label, Layout,
    Painter, Pos2, Rangef, Rect, Response, Rounding, Sense, Shape, Vec2,
};
use egui_phosphor::regular as icons;
use std::collections::HashMap;
use std::sync::mpsc;

const FLAME_HEIGHT: f32 = 20.0;
const MIN_WIDTH: f32 = 1.0;
const MIN_TEXT_WIDTH: f32 = 7.0;
const MAX_FRAMES: f32 = 1024.0;

pub struct FlameGraphTab {
    cached_root: Cached<FlameGraphNode>,
    widget: FlameGraphWidget,
    show_inline: bool,
}

impl Default for FlameGraphTab {
    fn default() -> Self {
        Self {
            cached_root: Default::default(),
            widget: Default::default(),
            show_inline: true,
        }
    }
}

impl TabWidget for FlameGraphTab {
    fn id(&self) -> Tab {
        Tab::FlameGraph
    }

    fn update(
        &mut self,
        ui: &mut Ui,
        cfg: &DevfilerConfig,
        kind: SampleKind,
        start: UtcTimestamp,
        end: UtcTimestamp,
    ) {
        let show_inline = self.show_inline;
        let root = self
            .cached_root
            .get_or_create((start, end, show_inline), move || {
                build_flame_graph(kind, start, end, show_inline)
            });

        ui.add_space(5.0);
        ui.columns(2, |ui| {
            ui[0].with_layout(Layout::left_to_right(Align::Min), |ui| {
                ui.checkbox(&mut self.show_inline, "Show inline");

                // Show sandwich view indicator
                if self.widget.sandwich_view.is_some() {
                    ui.label(
                        egui::RichText::new(" 🔍 Sandwich View Active (Double-click to exit)")
                            .color(Color32::YELLOW),
                    );
                } else {
                    ui.label(
                        egui::RichText::new(" Ctrl+Click on a frame to show callers/callees")
                            .color(Color32::DARK_GRAY)
                            .italics(),
                    );
                }
            });
            ui[1].with_layout(Layout::right_to_left(Align::Min), |ui| {
                let hint = format!("{} Filter ...", icons::FUNNEL);
                let prev_filter = self.widget.filter.clone();

                let status_text = if self.widget.filter.len() >= 3 {
                    if self.widget.match_count > 0 {
                        let current = self.widget.current_match_index + 1;
                        Some((
                            format!("{}/{}", current, self.widget.match_count),
                            Color32::from_rgb(100, 200, 100),
                        ))
                    } else {
                        Some(("No matches".to_string(), Color32::from_rgb(200, 100, 100)))
                    }
                } else {
                    None
                };

                clearable_line_edit_with_status(
                    ui,
                    &hint,
                    &mut self.widget.filter,
                    status_text
                        .as_ref()
                        .map(|(text, color)| (text.as_str(), *color)),
                );

                if prev_filter != self.widget.filter {
                    self.widget.current_match_index = 0;
                }
            });
        });
        ui.add_space(5.0);

        self.widget.draw(ui, cfg, &*root)
    }
}

/// MatchingFrame is a helper struct to navigate filtered frames.
struct MatchingFrame {
    id: FrameId,
    pos: Pos2,
    width_ratio: f32,
}

/// Widget drawing a flame-graph.
///
/// Separate from [`FlameGraphTab`] to allow reusing it later (e.g. for
/// differential flamegraph / sandwich views).
struct FlameGraphWidget {
    origin: Pos2,
    x_zoom: f32,
    filter: String,
    sandwich_view: Option<SandwichView>,

    matching_frames: Vec<MatchingFrame>,
    current_match_index: usize,
    match_count: usize,
    cached_filter: String,
    rebuild_matches: bool,
}

/// Sandwich view showing callers above and callees below a selected frame
struct SandwichView {
    #[allow(dead_code)]
    selected_frame: FrameId,
    selected_text: String,
    callers: FlameGraphNode,
    callees: FlameGraphNode,
}

impl Default for FlameGraphWidget {
    fn default() -> Self {
        Self {
            origin: Pos2::ZERO,
            x_zoom: 1.0,
            filter: "".to_string(),
            sandwich_view: None,
            matching_frames: Vec::new(),
            current_match_index: 0,
            match_count: 0,
            cached_filter: "".to_string(),
            rebuild_matches: true,
        }
    }
}

impl FlameGraphWidget {
    pub fn draw(&mut self, ui: &mut Ui, cfg: &DevfilerConfig, root: &FlameGraphNode) {
        egui::Frame::canvas(ui.style()).show(ui, |ui| {
            let size = ui.available_size_before_wrap();
            let (response, painter) = ui.allocate_painter(size, Sense::click_and_drag());

            self.process_inputs(ui, size, &response, root);

            if self.filter != self.cached_filter {
                self.matching_frames.clear();
                self.match_count = 0;
                self.cached_filter = self.filter.clone();
                self.rebuild_matches = true;
            }

            let to_screen = RectTransform::from_to(
                Rect::from_min_size(self.origin, response.rect.size()),
                response.rect,
            );

            let visible_x_range = Rangef::new(self.origin.x, self.origin.x + size.x);
            let clicked = response.clicked() && !response.double_clicked();
            let ctrl_held = ui.input(|i| i.modifiers.ctrl);
            let hover_pos = response.hover_pos();

            // Check if we're in sandwich view mode
            let is_sandwich_view = self.sandwich_view.is_some();
            if is_sandwich_view {
                // Draw sandwich view: callers on top, selected in middle, callees on bottom
                self.draw_sandwich_view(
                    ui.ctx(),
                    cfg,
                    &painter,
                    &to_screen,
                    visible_x_range,
                    hover_pos,
                    size,
                );
            } else {
                // Normal flamegraph view
                self.draw_level(
                    ui.ctx(),
                    cfg,
                    &painter,
                    &to_screen,
                    visible_x_range,
                    hover_pos,
                    clicked,
                    ctrl_held,
                    Pos2::ZERO,
                    size.x * self.x_zoom,
                    root,
                    root,
                );
            }

            self.rebuild_matches = false;
        });
    }

    /// Draw the sandwich view with callers above and callees below
    fn draw_sandwich_view(
        &mut self,
        ctx: &egui::Context,
        cfg: &DevfilerConfig,
        painter: &Painter,
        to_screen: &RectTransform,
        visible_x_range: Rangef,
        cursor_hover_pos: Option<Pos2>,
        size: Vec2,
    ) {
        // Take ownership temporarily to avoid borrowing issues
        let sandwich = self.sandwich_view.take().unwrap();
        let width = size.x * self.x_zoom;

        // Draw callers flamegraph upside-down, so flames grow upward from the selected frame
        let selected_height = FLAME_HEIGHT;
        let callers_height = (size.y - selected_height) / 2.0;
        let base_y = callers_height + selected_height;

        // Draw the selected frame at the base
        let selected_rect = Rect::from_min_size(pos2(0.0, base_y), vec2(width, FLAME_HEIGHT));
        let screen_rect = to_screen.transform_rect(selected_rect);
        painter.add(Shape::rect_filled(
            screen_rect,
            Rounding::ZERO,
            Color32::YELLOW,
        ));
        painter.add(Shape::rect_stroke(
            screen_rect,
            Rounding::ZERO,
            Stroke::new(2.0, Color32::BLACK),
        ));
        painter.text(
            to_screen * selected_rect.min + vec2(4.0, 4.0),
            Align2::LEFT_TOP,
            &sandwich.selected_text,
            FontId::monospace(11.0),
            Color32::BLACK,
        );

        // Helper to draw a single inverted caller flame
        fn draw_inverted_caller_flame(
            _widget: &mut FlameGraphWidget,
            _ctx: &egui::Context,
            _cfg: &DevfilerConfig,
            painter: &Painter,
            to_screen: &RectTransform,
            visible_x_range: Rangef,
            _cursor_hover_pos: Option<Pos2>,
            x: f32,
            base_y: f32,
            width: f32,
            node: &FlameGraphNode,
            level: usize,
        ) {
            if width < MIN_WIDTH {
                return;
            }

            let y = base_y - (level as f32 * FLAME_HEIGHT);
            let rect = Rect::from_min_size(pos2(x, y), vec2(width, FLAME_HEIGHT));
            let flame_range = Rangef::new(rect.min.x, rect.max.x);
            if flame_range.intersection(visible_x_range.clone()).span() <= 0.0 {
                return;
            }

            let screen_rect = to_screen.transform_rect(rect);
            painter.add(Shape::rect_filled(
                screen_rect,
                Rounding::ZERO,
                node.bg_color,
            ));
            painter.add(Shape::rect_stroke(
                screen_rect,
                Rounding::ZERO,
                Stroke::new(0.5, Color32::BLACK),
            ));

            if width > MIN_TEXT_WIDTH {
                painter.with_clip_rect(screen_rect).text(
                    to_screen * rect.min + vec2(4.0, 4.0),
                    Align2::LEFT_TOP,
                    &node.text,
                    FontId::monospace(11.0),
                    node.fg_color,
                );
            }

            let mut child_x = x;
            for child in &node.children {
                let child_width = width * (child.weight as f32 / node.weight.max(1) as f32);
                draw_inverted_caller_flame(
                    _widget,
                    _ctx,
                    _cfg,
                    painter,
                    to_screen,
                    visible_x_range.clone(),
                    _cursor_hover_pos,
                    child_x,
                    base_y,
                    child_width,
                    child,
                    level + 1,
                );
                child_x += child_width;
            }
        }

        // Draw each caller flame above the selected frame, side by side
        let mut x_offset = 0.0;
        for child in &sandwich.callers.children {
            let flame_width = width * (child.weight as f32 / sandwich.callers.weight.max(1) as f32);
            draw_inverted_caller_flame(
                self,
                ctx,
                cfg,
                painter,
                to_screen,
                visible_x_range.clone(),
                cursor_hover_pos,
                x_offset,
                base_y,
                flame_width,
                child,
                1,
            );
            x_offset += flame_width;
        }

        // Draw callees (growing downwards from selected frame, in lower half)
        // Skip the "Callees" meta node and draw its children directly
        let callees_y = base_y + FLAME_HEIGHT;
        if !sandwich.callees.children.is_empty() {
            let mut x_offset = 0.0;
            for child in &sandwich.callees.children {
                let child_width =
                    width * (child.weight as f32 / sandwich.callees.weight.max(1) as f32);
                self.draw_level(
                    ctx,
                    cfg,
                    painter,
                    to_screen,
                    visible_x_range.clone(),
                    cursor_hover_pos,
                    false, // No clicking in sandwich view for now
                    false,
                    pos2(x_offset, callees_y),
                    child_width,
                    child,
                    child,
                );
                x_offset += child_width;
            }
        }

        // Restore the sandwich view
        self.sandwich_view = Some(sandwich);
    }

    /// Process dragging, scrolling and zooming.
    fn process_inputs(
        &mut self,
        ui: &mut Ui,
        size: Vec2,
        response: &Response,
        _root: &FlameGraphNode,
    ) {
        let Some(cursor) = response.hover_pos() else {
            // Check for Enter key even when not hovered (for filter navigation)
            if ui.input(|i| i.key_pressed(Key::Enter))
                && self.filter.len() >= 3
                && !self.matching_frames.is_empty()
            {
                let shift_held = ui.input(|i| i.modifiers.shift);
                if shift_held {
                    self.navigate_to_prev_match(size);
                } else {
                    self.navigate_to_next_match(size);
                }
            }
            return;
        };

        // Handle Enter key for navigating through matches
        if ui.input(|i| i.key_pressed(Key::Enter))
            && self.filter.len() >= 3
            && !self.matching_frames.is_empty()
        {
            let shift_held = ui.input(|i| i.modifiers.shift);
            if shift_held {
                self.navigate_to_prev_match(size);
            } else {
                self.navigate_to_next_match(size);
            }
            return;
        }

        // Double-click -> reset the view.
        if response.double_clicked() {
            self.origin = Pos2::ZERO;
            self.x_zoom = 1.0;
            self.sandwich_view = None; // Exit sandwich view on double-click
            return;
        }

        let (scroll, mut zoom) = ui.input(|x| (x.smooth_scroll_delta, x.zoom_delta_2d()));
        self.origin -= response.drag_delta();
        self.origin -= scroll;

        for key in ui.input(|x| x.keys_down.clone()) {
            match key {
                Key::H | Key::ArrowLeft => self.origin.x -= 100.0,
                Key::L | Key::ArrowRight => self.origin.x += 100.0,
                Key::K | Key::ArrowUp => {
                    if ui.input(|x| x.modifiers).command_only() {
                        zoom.x -= 0.25
                    } else {
                        self.origin.y -= 100.0;
                    }
                }
                Key::J | Key::ArrowDown => {
                    if ui.input(|x| x.modifiers).command_only() {
                        zoom.x += 0.25
                    } else {
                        self.origin.y += 100.0;
                    }
                }
                _ => (),
            }
        }

        let rel_cursor_x = cursor.x - response.rect.min.x;
        self.x_zoom = (self.x_zoom * zoom.x).max(1.0);
        self.origin.x += (self.origin.x + rel_cursor_x) * (zoom.x - 1.0);

        // Clamp to visible region: easy to get lost without this.
        let virt_width = size.x * self.x_zoom;
        self.origin.x = self.origin.x.clamp(0.0, (virt_width - size.x).max(0.0));
        self.origin.y = self.origin.y.clamp(0.0, MAX_FRAMES * FLAME_HEIGHT);
    }

    fn draw_level(
        // TODO: way too many args. use struct for static portion?
        &mut self,
        ctx: &egui::Context,
        cfg: &DevfilerConfig,
        painter: &Painter,
        to_screen: &RectTransform,
        visible_x_range: Rangef,
        cursor_hover_pos: Option<Pos2>,
        clicked: bool,
        ctrl_held: bool,
        draw_pos: Pos2,
        avail_width: f32,
        root: &FlameGraphNode,
        flame: &FlameGraphNode,
    ) -> f32 {
        let flame_width = avail_width * (flame.weight as f32 / root.weight.max(1) as f32);
        if flame_width < MIN_WIDTH {
            return flame_width;
        }

        let rect = Rect::from_min_size(draw_pos, vec2(flame_width, FLAME_HEIGHT));
        let screen_rect = to_screen.transform_rect(rect);

        let flame_range = Rangef::new(rect.min.x, rect.max.x);
        if flame_range.intersection(visible_x_range).span() <= 0.0 {
            return flame_width;
        }

        let bg_color = if self.filter.len() >= 3 {
            if flame.text.contains(&self.filter) {
                flame.bg_color
            } else {
                flame.bg_color.gamma_multiply(0.5)
            }
        } else {
            flame.bg_color
        };

        // Track matching frames for navigation (only if filter changed)
        let is_match = self.filter.len() >= 3 && flame.text.contains(&self.filter);
        let is_focused = if is_match {
            if self.rebuild_matches {
                let unscaled_pos = pos2(draw_pos.x / self.x_zoom, draw_pos.y);
                let width_ratio = flame.weight as f32 / root.weight.max(1) as f32;

                self.matching_frames.push(MatchingFrame {
                    id: flame.id,
                    pos: unscaled_pos,
                    width_ratio,
                });
                self.match_count += 1;
            }

            // Check if this frame is the focused one by comparing IDs
            self.matching_frames
                .get(self.current_match_index)
                .map(|m| m.id == flame.id)
                .unwrap_or(false)
        } else {
            false
        };

        // Highlight the currently focused match
        if is_focused {
            painter.add(Shape::rect_stroke(
                screen_rect,
                Rounding::ZERO,
                Stroke::new(3.0, Color32::from_rgb(255, 215, 0)), // Gold color
            ));
        }

        painter.add(Shape::rect_filled(screen_rect, Rounding::ZERO, bg_color));

        painter.add(Shape::rect_stroke(
            screen_rect,
            Rounding::ZERO,
            Stroke::new(0.5, Color32::BLACK),
        ));

        if flame_width > MIN_TEXT_WIDTH {
            painter.with_clip_rect(screen_rect).text(
                to_screen * rect.min + vec2(4.0, 4.0),
                Align2::LEFT_TOP,
                &flame.text,
                FontId::monospace(11.0),
                flame.fg_color,
            );
        }

        if let Some(hover_pos) = cursor_hover_pos {
            if screen_rect.contains(hover_pos) {
                let id = Id::new("flamegraph-tooltip");
                show_tooltip_at_pointer(
                    ctx,
                    egui::LayerId::new(egui::Order::Tooltip, id),
                    id,
                    |ui: &mut Ui| self.draw_tooltip(ui, cfg, root, flame),
                );

                if clicked && flame.weight >= 1 {
                    if ctrl_held {
                        // Ctrl+Click: Enter sandwich view mode
                        self.sandwich_view = Some(build_sandwich_view(root, flame.id));
                        self.origin = Pos2::ZERO;
                        self.x_zoom = 1.0;
                    } else {
                        // Normal click: Zoom to frame
                        self.x_zoom = root.weight as f32 / flame.weight as f32;
                        self.origin.x =
                            draw_pos.x / avail_width * (to_screen.from().width() * self.x_zoom);
                    }
                }
            }
        }

        let mut offset = draw_pos.x;
        for child in &flame.children {
            offset += self.draw_level(
                ctx,
                cfg,
                painter,
                to_screen,
                visible_x_range.clone(),
                cursor_hover_pos,
                clicked,
                ctrl_held,
                pos2(offset, draw_pos.y + FLAME_HEIGHT),
                avail_width,
                root,
                child,
            );
        }

        flame_width
    }

    /// Navigate to the next matching frame
    fn navigate_to_next_match(&mut self, size: Vec2) {
        if self.matching_frames.is_empty() {
            return;
        }

        // Cycle to next match
        self.current_match_index = (self.current_match_index + 1) % self.match_count;
        self.center_on_current_match(size);
    }

    /// Navigate to the previous matching frame
    fn navigate_to_prev_match(&mut self, size: Vec2) {
        if self.matching_frames.is_empty() {
            return;
        }

        // Cycle to previous match (wrap around)
        if self.current_match_index == 0 {
            self.current_match_index = self.match_count - 1;
        } else {
            self.current_match_index -= 1;
        }
        self.center_on_current_match(size);
    }

    /// Center the view on the currently selected match
    fn center_on_current_match(&mut self, size: Vec2) {
        if let Some(frame_info) = self.matching_frames.get(self.current_match_index) {
            let base_width = size.x * frame_info.width_ratio;

            // Make sure text in frame is readable
            let min_visible_width = size.x * 0.3;
            let desired_zoom = if base_width < min_visible_width {
                (min_visible_width / base_width).min(20.0)
            } else {
                1.0
            };

            // Update zoom
            self.x_zoom = desired_zoom;

            // Recalculate frame position and center with new zoom
            let frame_x = frame_info.pos.x * self.x_zoom;
            let frame_width_zoomed = base_width * self.x_zoom;
            let frame_center_x = frame_x + frame_width_zoomed / 2.0;
            let frame_center_y = frame_info.pos.y + FLAME_HEIGHT / 2.0;

            let target_x = frame_center_x - size.x / 2.0;
            let target_y = frame_center_y - size.y / 2.0;

            self.origin.x = target_x.max(0.0);
            self.origin.y = target_y.max(0.0);

            // Clamp to visible region
            let virt_width = size.x * self.x_zoom;
            self.origin.x = self.origin.x.clamp(0.0, (virt_width - size.x).max(0.0));
            self.origin.y = self.origin.y.clamp(0.0, MAX_FRAMES * FLAME_HEIGHT);
        }
    }

    /// Populates the on-hover tooltip UI.
    fn draw_tooltip(
        &self,
        ui: &mut Ui,
        cfg: &DevfilerConfig,
        root: &FlameGraphNode,
        flame: &FlameGraphNode,
    ) {
        ui.vertical(|ui| {
            if cfg.dev_mode {
                ui.horizontal(|ui| {
                    ui.strong("File ID:");
                    ui.monospace(flame.id.file_id.format_hex());
                });
                ui.horizontal(|ui| {
                    ui.strong("Address || Line:");
                    ui.monospace(format!("{:#x}", flame.id.addr_or_line));
                });

                let mut es_frame_id = [0; 16 + 8];
                es_frame_id[0..16].copy_from_slice(&u128::from(flame.id.file_id).to_be_bytes());
                es_frame_id[16..24].copy_from_slice(&flame.id.addr_or_line.to_be_bytes());
                ui.horizontal(|ui| {
                    ui.strong("ES Frame ID:");
                    ui.monospace(ES_B64_ENGINE.encode(&es_frame_id));
                });

                ui.separator();
            }
            ui.horizontal(|ui| {
                ui.strong("Samples (self):");
                let weight_self = flame.weight_self();
                let perc = weight_self as f32 / root.weight as f32 * 100.0;
                ui.label(format!("{} ({:.02}%)", humanize_count(weight_self), perc));
            });
            ui.horizontal(|ui| {
                ui.strong("Samples (w/ children):");
                let perc = flame.weight as f32 / root.weight as f32 * 100.0;
                ui.label(format!("{} ({:.02}%)", humanize_count(flame.weight), perc));
            });
            ui.horizontal(|ui| {
                ui.strong("Location:");
                ui.add(Label::new(&flame.text).wrap());
            });
        });
    }
}

static ES_B64_ENGINE: base64::engine::GeneralPurpose = base64::engine::GeneralPurpose::new(
    &base64::alphabet::URL_SAFE,
    base64::engine::GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent),
);

/// Pull in events and construct a flame graph data structure for them.
fn build_flame_graph(
    kind: SampleKind,
    start: UtcTimestamp,
    end: UtcTimestamp,
    inline_frames: bool,
) -> FlameGraphNode {
    // Thread 1: pull events from the table.
    let (event_tx, event_rx) = mpsc::sync_channel(4096);
    let table_task = tokio::task::spawn_blocking(move || {
        for (_, tc) in DB.trace_events.time_range(start, end, kind) {
            event_tx
                .send(tc)
                .expect("should never be closed on RX side (1)");
        }
    });

    // Thread 2 (this one): aggregate.
    let mut comm_nodes = HashMap::new();
    for tc in event_rx {
        let tc = tc.get();

        let Some(trace) = DB.stack_traces.get(tc.trace_hash) else {
            continue;
        };

        let comm_node = if let Some(node) = comm_nodes.get_mut(tc.comm.as_str()) {
            node
        } else {
            // This insert/get chain is dumb, but `try_insert` (which fixes it)
            // is not yet available on stable Rust. `entry` API also isn't any
            // good here because it requires cloning a string in the hot path.
            comm_nodes.insert(
                tc.comm.to_owned(),
                FlameGraphNode::new_meta_node(
                    format!("{} {}", icons::APP_WINDOW, tc.comm),
                    comm_nodes.len() as u64,
                ),
            );
            comm_nodes.get_mut(tc.comm.as_str()).unwrap()
        };

        comm_node.insert_trace(&trace.get(), tc.count as u64, inline_frames);
    }

    // Wait for table task to exit.
    let rt = tokio::runtime::Handle::current();
    rt.block_on(table_task).expect("table task panicked");

    let mut root = FlameGraphNode::root();
    root.weight = comm_nodes.values().map(|x| x.weight).sum();
    root.children = comm_nodes.into_values().collect();
    root.sort_children();
    root
}

/// Node in the flame graph tree structure.
#[derive(Debug, Clone)]
struct FlameGraphNode {
    pub weight: u64,
    pub fg_color: Color32,
    pub bg_color: Color32,
    pub id: FrameId,
    pub text: String,
    pub inline_skip: u16,
    pub children: Vec<FlameGraphNode>,
}

impl Default for FlameGraphNode {
    fn default() -> Self {
        Self::root()
    }
}

impl FlameGraphNode {
    pub fn root() -> Self {
        Self::new_meta_node(format!("{} 100% of all CPU cycles", icons::CPU), 0)
    }

    pub fn new_meta_node(text: String, addr_or_line: u64) -> Self {
        FlameGraphNode {
            text,
            weight: 0,
            id: FrameId {
                file_id: FileId::from(0),
                addr_or_line: addr_or_line,
            },
            fg_color: Color32::WHITE,
            inline_skip: 0,
            bg_color: Color32::from_rgb(0x39, 0x3D, 0x3F),
            children: Vec::with_capacity(1024),
        }
    }

    /// Node's weight including children.
    pub fn weight_children(&self) -> u64 {
        self.children.iter().map(|x| x.weight).sum()
    }

    /// Node's weight excluding children.
    pub fn weight_self(&self) -> u64 {
        self.weight - self.weight_children()
    }

    /// Insert a trace into the flame graph.
    pub fn insert_trace(&mut self, trace: &[ArchivedFrame], weight: u64, inline_frames: bool) {
        let mut node = self;
        node.weight += weight;

        for frame in trace.iter().rev() {
            let frame: Frame = (*frame).into();

            // WARN: this `find` makes flame graph construction O(n^2) in the
            //       worst case, but I found that in the average case this is
            //       actually quite a bit faster than a hashmap/btreemap based
            //       approach. Most nodes only have one or two nodes.
            // TODO: experiment with a mixed approach that uses linear search for
            //       nodes with <8 nodes and a hashmap for larger ones
            if let Some(mut child) = node.children.iter_mut().find(|x| x.id == frame.id.into()) {
                child.weight += weight;

                for _ in 0..child.inline_skip {
                    child = child.children.first_mut().unwrap();
                    child.weight += weight;
                }

                node = unsafe { &mut *(child as *mut _) };
                continue;
            }

            if let FrameKind::Abort = frame.kind {
                node.children.push(FlameGraphNode {
                    weight,
                    fg_color: Color32::BLACK,
                    bg_color: frame_kind_color(frame.kind),
                    id: frame.id,
                    text: match error_spec_by_id(frame.id.addr_or_line) {
                        Some(spec) => {
                            format!("<unwinding aborted: {}>", spec.name)
                        }
                        None => {
                            format!("<unwinding aborted: error code {}>", frame.id.addr_or_line)
                        }
                    },
                    inline_skip: 0,
                    children: vec![],
                });
                node = node.children.last_mut().unwrap();
                continue;
            }

            let inline_frames = symbolize_frame(frame, inline_frames);
            assert!(!inline_frames.is_empty());
            let mut inline_len = Some((inline_frames.len() - 1) as u16);

            for (i, inline_node) in inline_frames.into_iter().enumerate() {
                assert!(i == 0 || node.children.is_empty());

                node.children.push(FlameGraphNode {
                    weight,
                    fg_color: Color32::BLACK,
                    bg_color: frame_kind_color(frame.kind),
                    id: inline_node.raw.id,
                    text: match frame.kind.interp() {
                        None => inline_node.to_string(),
                        Some(interp) => format!(
                            "{} [{}]{}",
                            inline_node,
                            interp,
                            if i > 0 { " [Inline]" } else { "" },
                        ),
                    },
                    inline_skip: inline_len.take().unwrap_or(0),
                    children: vec![],
                });

                node = node.children.last_mut().unwrap();
            }
        }
    }

    /// Sort all nodes in the graph.
    ///
    /// The sorting key is `(weight, file_id, addr_or_line)`.
    fn sort_children(&mut self) {
        self.children
            .sort_unstable_by_key(|x| (-(x.weight as i64), x.id));
        for child in &mut self.children {
            child.sort_children();
        }
    }
}

/// Build a sandwich view for a given frame ID
fn build_sandwich_view(root: &FlameGraphNode, frame_id: FrameId) -> SandwichView {
    let mut callers = FlameGraphNode::new_meta_node("Callers".to_string(), 0);
    let mut callees = FlameGraphNode::new_meta_node("Callees".to_string(), 0);
    let mut selected_info: Option<String> = None;

    // Traverse the tree and collect all paths that contain the target frame
    collect_paths_through_frame(
        root,
        frame_id,
        &mut Vec::new(),
        &mut callers,
        &mut callees,
        &mut selected_info,
    );

    callers.sort_children();
    callees.sort_children();

    let selected_text = selected_info.unwrap_or_else(|| "Unknown Frame".to_string());

    SandwichView {
        selected_frame: frame_id,
        selected_text,
        callers,
        callees,
    }
}

/// Helper function to collect caller and callee paths through a specific frame
fn collect_paths_through_frame(
    node: &FlameGraphNode,
    target_frame: FrameId,
    path_above: &mut Vec<(FrameId, Color32, Color32, String)>,
    callers_root: &mut FlameGraphNode,
    callees_root: &mut FlameGraphNode,
    selected_info: &mut Option<String>,
) {
    // Check if this node is the target frame
    if node.id == target_frame {
        // Found the target frame!
        // Store the frame's display information
        if selected_info.is_none() {
            *selected_info = Some(node.text.clone());
        }

        // Insert the caller path (inverted) into callers_root
        if !path_above.is_empty() {
            insert_caller_path(callers_root, path_above, node.weight);
        }

        // Insert all callees into callees_root
        insert_callee_subtree(callees_root, node);
        return;
    }

    // Continue searching in children
    // Skip adding the root frame (100% of all CPU cycles) to the path
    let is_root_frame = node.id.addr_or_line == 0 && node.id.file_id == FileId::from(0);
    if !is_root_frame {
        path_above.push((node.id, node.bg_color, node.fg_color, node.text.clone()));
    }
    for child in &node.children {
        collect_paths_through_frame(
            child,
            target_frame,
            path_above,
            callers_root,
            callees_root,
            selected_info,
        );
    }
    if !is_root_frame {
        path_above.pop();
    }
}

/// Insert a caller path (inverted, from target up to root)
/// Insert a caller path so the flamegraph grows downward from the selected frame
fn insert_caller_path(
    root: &mut FlameGraphNode,
    path: &[(FrameId, Color32, Color32, String)],
    weight: u64,
) {
    root.weight += weight;
    // Walk the path in reverse (from the immediate caller down to the root)
    // so that level 1 is the direct caller and the root is at the top
    let mut current = root;
    for (frame_id, bg_color, fg_color, text) in path.iter().rev() {
        // Find or create child with this frame_id
        if let Some(child) = current.children.iter_mut().find(|x| x.id == *frame_id) {
            child.weight += weight;
            current = unsafe { &mut *(child as *mut _) };
        } else {
            current.children.push(FlameGraphNode {
                weight,
                fg_color: *fg_color,
                bg_color: *bg_color,
                id: *frame_id,
                text: text.clone(),
                inline_skip: 0,
                children: vec![],
            });
            current = current.children.last_mut().unwrap();
        }
    }
}

/// Insert all callees of the target frame
fn insert_callee_subtree(root: &mut FlameGraphNode, node: &FlameGraphNode) {
    root.weight += node.weight_children();

    for child in &node.children {
        insert_callee_node(root, child);
    }
}

/// Recursively insert a callee node and its descendants
fn insert_callee_node(parent: &mut FlameGraphNode, node: &FlameGraphNode) {
    // Find or create child with this frame_id
    let child = if let Some(existing) = parent.children.iter_mut().find(|x| x.id == node.id) {
        existing.weight += node.weight;
        existing
    } else {
        parent.children.push(FlameGraphNode {
            weight: node.weight,
            fg_color: node.fg_color,
            bg_color: node.bg_color,
            id: node.id,
            text: node.text.clone(),
            inline_skip: node.inline_skip,
            children: vec![],
        });
        parent.children.last_mut().unwrap()
    };

    // Recursively add all descendants
    for grandchild in &node.children {
        insert_callee_node(child, grandchild);
    }
}
