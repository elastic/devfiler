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

use crate::storage::{FrameKind, InterpKind};
use eframe::emath::{Pos2, Rect, Vec2};
use eframe::epaint::{Color32, Stroke};
use egui::{Button, TextEdit, Ui};
use egui_phosphor::regular as icons;
use std::fmt;

/// Draw a line edit with a button for clearing it.
pub fn clearable_line_edit(ui: &mut Ui, hint: &str, input: &mut String) {
    let elem = TextEdit::singleline(input).hint_text(hint);
    let edit_rect = ui.add(elem).rect;

    if !input.is_empty() {
        let mut clear_origin = edit_rect.right_center();
        clear_origin.x -= 10.0;

        let clear_rect = Rect::from_center_size(clear_origin, Vec2::splat(15.0));
        let clear_widget = Button::new(icons::X).small().frame(false);

        let clear_resp = ui.put(clear_rect, clear_widget);

        if clear_resp.clicked() {
            input.clear();
        }
    }
}

/// Suggest a color for the given frame kind.
pub fn frame_kind_color(kind: FrameKind) -> Color32 {
    let interp = match kind {
        FrameKind::Regular(x) => x,
        FrameKind::Error(_) => return Color32::from_rgb(0xfd, 0x84, 0x84),
        FrameKind::Abort => return Color32::from_rgb(0xfc, 0x4f, 0x4f),

        // Intentionally horrible color to make it obvious that something is wrong:
        FrameKind::Unknown(_) | FrameKind::UnknownError(_) => return Color32::RED,
    };

    match interp {
        InterpKind::Python => Color32::from_rgb(0xfc, 0xae, 0x6b),
        InterpKind::Php => Color32::from_rgb(0xfc, 0xdb, 0x82),
        InterpKind::Native => Color32::from_rgb(0x6d, 0xd0, 0xdc),
        InterpKind::Kernel => Color32::from_rgb(0x7c, 0x9e, 0xff),
        InterpKind::Jvm => Color32::from_rgb(0x65, 0xd3, 0xac),
        InterpKind::Ruby => Color32::from_rgb(0xd7, 0x9f, 0xfc),
        InterpKind::Perl => Color32::from_rgb(0xf9, 0x8b, 0xb9),
        InterpKind::Js => Color32::from_rgb(0xcb, 0xc3, 0xe3),
        InterpKind::PhpJit => Color32::from_rgb(0xcc, 0xfc, 0x82),
        InterpKind::Beam => Color32::from_rgb(0xda, 0x70, 0xd6),
        InterpKind::Go => Color32::from_rgb(0x00, 0xad, 0xd8),

        // TODO: sync color with Kibana once one is assigned
        InterpKind::DotNet => Color32::from_rgb(0x6c, 0x60, 0xe1),
    }
}

/// Format a count to a nice representation optimized for human readability.
pub fn humanize_count(x: u64) -> HumanCount {
    if x > 10u64.pow(9) {
        HumanCount(x as f32 / 1e9, 2, "B")
    } else if x > 10u64.pow(6) {
        HumanCount(x as f32 / 1e6, 2, "M")
    } else if x > 10u64.pow(3) {
        HumanCount(x as f32 / 1e3, 2, "K")
    } else {
        HumanCount(x as f32, 0, "")
    }
}

#[derive(Debug)]
pub struct HumanCount(f32, usize, &'static str);

impl fmt::Display for HumanCount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self(s, d, u) = *self;
        write!(f, "{s:.d$}{u}")
    }
}

/// Generate a nice color the same way as [`egui_plot`] does it.
#[inline(always)] // should be `const`, but currently can't do float arith in const fn
pub fn plot_color(idx: usize) -> Color32 {
    let golden_ratio = (5.0_f32.sqrt() - 1.0) / 2.0;
    let hue = idx as f32 * golden_ratio;
    egui::ecolor::Hsva::new(hue, 0.85, 0.5, 1.0).into()
}

/// Draws a heat-map.
pub fn draw_heat_map<I>(ui: &mut Ui, rows: usize, columns: usize, col_iter: I)
where
    I: Iterator,
    I::Item: Iterator<Item = Color32>,
{
    let mut rect = ui.available_rect_before_wrap();

    if !ui.is_rect_visible(rect) {
        return;
    }

    let painter = ui.painter_at(rect);
    let bg_stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    painter.rect(rect, 0.0, ui.visuals().extreme_bg_color, bg_stroke);
    rect = rect.shrink(bg_stroke.width);

    let tile_size = Vec2::new(rect.width() / columns as f32, rect.height() / rows as f32);

    for (col_idx, col) in col_iter.enumerate() {
        for (row_idx, color) in col.enumerate().take(rows) {
            if color == Color32::TRANSPARENT {
                continue;
            }

            let min = Pos2::new(
                rect.min.x + tile_size.x * col_idx as f32,
                rect.min.y + tile_size.y * row_idx as f32,
            );

            let tile = Rect::from_min_max(
                painter.round_pos_to_pixels(min),
                painter.round_pos_to_pixels(min + tile_size),
            );

            painter.rect(tile, 0.0, color, Stroke::NONE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanize() {
        assert_eq!(humanize_count(12).to_string(), "12");
        assert_eq!(humanize_count(1_234).to_string(), "1.23K");
        assert_eq!(humanize_count(12_344_000).to_string(), "12.34M");
    }
}
