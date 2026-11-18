use egui::{Color32, RichText};
use httpdeck_lib::models::RequestResult;

const BODY_PREVIEW_LIMIT: usize = 200_000;

pub fn ui(ui: &mut egui::Ui, result: Option<&RequestResult>, error: Option<&str>) {
    ui.horizontal(|ui| {
        ui.strong("响应");
        if let Some(result) = result {
            let color = if result.error.is_some() {
                Color32::from_rgb(230, 100, 100)
            } else if result.status >= 400 {
                Color32::from_rgb(230, 170, 70)
            } else {
                Color32::from_rgb(90, 190, 120)
            };
            ui.colored_label(color, format!("{} {}", result.status, result.status_text));
            ui.label(format!("{} ms", result.duration_ms));
            ui.label(format_size(result.size));
            if let Some(mime) = &result.mime_type {
                ui.weak(mime);
            }
        }
        if let Some(error) = error {
            ui.colored_label(Color32::from_rgb(230, 100, 100), error);
        }
    });

    ui.separator();

    let Some(result) = result else {
        ui.weak("尚未执行请求");
        return;
    };

    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if let Some(error) = &result.error {
                ui.colored_label(Color32::from_rgb(230, 100, 100), error);
            }

            for test in &result.tests {
                let color = if test.passed {
                    Color32::from_rgb(90, 190, 120)
                } else {
                    Color32::from_rgb(230, 100, 100)
                };
                ui.colored_label(
                    color,
                    format!(
                        "{} {}",
                        if test.passed { "PASS" } else { "FAIL" },
                        test.name
                    ),
                );
            }

            egui::CollapsingHeader::new(format!("响应头（{}）", result.response_headers.len()))
                .default_open(false)
                .show(ui, |ui| {
                    for header in &result.response_headers {
                        ui.monospace(format!("{}: {}", header.name, header.value));
                    }
                });

            let mut text = result.body.clone();
            if text.len() > BODY_PREVIEW_LIMIT {
                text.truncate(BODY_PREVIEW_LIMIT);
                text.push_str("\n\n…（响应体过大，仅显示前 200 KB）");
            }
            ui.add(
                egui::Label::new(RichText::new(text).monospace())
                    .selectable(true)
                    .wrap_mode(egui::TextWrapMode::Extend),
            );
        });
}

fn format_size(size: usize) -> String {
    if size >= 1024 * 1024 {
        format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
    } else if size >= 1024 {
        format!("{:.1} KB", size as f64 / 1024.0)
    } else {
        format!("{size} B")
    }
}
