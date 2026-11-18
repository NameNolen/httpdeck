#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod response;
mod tree;

use std::sync::Arc;

fn main() -> eframe::Result<()> {
    // GUI 子系统下看不到 panic 输出，写入临时日志便于排查
    std::panic::set_hook(Box::new(|info| {
        let log = std::env::temp_dir().join("httpdeck-egui-panic.log");
        let _ = std::fs::write(log, format!("{info}\n"));
    }));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([760.0, 520.0])
            // 高 DPI 缩放下 1280x820 逻辑像素可能超出屏幕，默认最大化避免面板被挤出屏幕
            .with_maximized(true),
        ..Default::default()
    };
    eframe::run_native(
        "HttpDeck",
        options,
        Box::new(|cc| {
            install_cjk_fonts(&cc.egui_ctx);
            Ok(Box::new(app::HttpDeckApp::new()))
        }),
    )
}

/// egui 自带字体不含中文，这里加载系统中文字体作为兜底
fn install_cjk_fonts(ctx: &egui::Context) {
    let candidates = [
        "C:/Windows/Fonts/simhei.ttf",
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/simsun.ttc",
    ];
    for path in candidates {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "cjk".to_owned(),
            Arc::new(egui::FontData::from_owned(bytes)),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .push("cjk".to_owned());
        }
        ctx.set_fonts(fonts);
        return;
    }
}
