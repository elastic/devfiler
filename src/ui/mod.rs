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

mod app;
mod cached;
mod tabs;
mod timeaxis;
mod util;

static ICON_BYTES: &[u8] = include_bytes!("../../assets/icon.png");

pub fn gui_thread(collector: crate::collector::Collector) -> Result<(), eframe::Error> {
    let icon = eframe::icon_data::from_png_bytes(ICON_BYTES);
    let icon = icon.expect("corrupted icon");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_icon(icon)
            .with_inner_size([1500., 900.]),
        ..Default::default()
    };

    let mut title_exts = Vec::new();
    if cfg!(debug_assertions) {
        title_exts.push("debug build, slow!".to_owned());
    }
    let title_ext = if !title_exts.is_empty() {
        format!(" ({})", title_exts.join(" "))
    } else {
        String::new()
    };

    eframe::run_native(
        &format!(
            "Elastic devfiler v{}{}",
            env!("CARGO_PKG_VERSION"),
            title_ext,
        ),
        options,
        Box::new(move |cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            load_phosphor_icons(&cc.egui_ctx);
            tokio::spawn(background_ui_waker(cc.egui_ctx.clone()));
            Box::new(app::DevfilerUi::new(collector))
        }),
    )
}

async fn background_ui_waker(ctx: egui::Context) {
    let mut db_watcher = crate::storage::UpdateWatcher::default();
    let mut cache_watcher = cached::UpdateWatcher::default();
    let freq = std::time::Duration::from_millis(50);

    loop {
        if db_watcher.any_changes() || cache_watcher.any_caches() {
            ctx.request_repaint();
        }

        tokio::time::sleep(freq).await;
    }
}

fn load_phosphor_icons(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let data = egui_phosphor::Variant::Regular.font_data();
    fonts.font_data.insert("phosphor".into(), data);
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        if let Some(font_keys) = fonts.families.get_mut(&family) {
            font_keys.push("phosphor".into());
        }
    }
    ctx.set_fonts(fonts);
}
