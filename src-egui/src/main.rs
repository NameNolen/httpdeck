#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod response;
mod tray;
mod tree;

fn main() -> eframe::Result<()> {
    // GUI 子系统下看不到 panic 输出，写入临时日志便于排查
    std::panic::set_hook(Box::new(|info| {
        let log = std::env::temp_dir().join("httpdeck-egui-panic.log");
        let _ = std::fs::write(log, format!("{info}\n"));
    }));

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 820.0])
        .with_min_inner_size([760.0, 520.0])
        // 高 DPI 缩放下 1280x820 逻辑像素可能超出屏幕，默认最大化避免面板被挤出屏幕
        .with_maximized(true);
    // 窗口/任务栏图标与 exe 图标保持一致
    if let Ok(icon) =
        eframe::icon_data::from_png_bytes(include_bytes!("../../src-tauri/icons/icon.png"))
    {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let result = eframe::run_native(
        "HttpDeck",
        options,
        Box::new(|cc| {
            install_cjk_fonts(&cc.egui_ctx);
            // 托盘需在主线程、event loop 启动后创建；失败时退化为普通窗口
            if let Err(err) = tray::init(&cc.egui_ctx) {
                eprintln!("系统托盘初始化失败：{err}");
            }
            Ok(Box::new(app::HttpDeckApp::new()))
        }),
    );

    // 正常退出时清理托盘
    tray::cleanup();

    result
}

/// egui 自带字体不含中文，这里加载系统中文字体作为兜底。
/// 返回是否成功加载到中文字体。
fn install_cjk_fonts(ctx: &egui::Context) -> bool {
    let mut fonts = egui::FontDefinitions::default();
    if !install_font(&mut fonts, "cjk", cjk_font_candidates()) {
        return false;
    }
    // 中文字体通常不含 ▶ ▸ ▾ ✖ 等几何符号，再追加一个符号字体兜底
    install_font(&mut fonts, "symbols", symbol_font_candidates());
    ctx.set_fonts(fonts);
    true
}

#[cfg(target_os = "windows")]
fn cjk_font_candidates() -> &'static [&'static str] {
    &[
        "C:/Windows/Fonts/simhei.ttf",
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/simsun.ttc",
    ]
}

#[cfg(target_os = "windows")]
fn symbol_font_candidates() -> &'static [&'static str] {
    &["C:/Windows/Fonts/seguisym.ttf"]
}

#[cfg(target_os = "macos")]
fn cjk_font_candidates() -> &'static [&'static str] {
    &[
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/System/Library/Fonts/STHeiti Medium.ttc",
        "/System/Library/Fonts/Supplemental/Songti.ttc",
    ]
}

#[cfg(target_os = "macos")]
fn symbol_font_candidates() -> &'static [&'static str] {
    &[
        "/System/Library/Fonts/Apple Symbols.ttf",
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    ]
}

#[cfg(all(unix, not(target_os = "macos")))]
fn cjk_font_candidates() -> &'static [&'static str] {
    &[
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
        "/usr/share/fonts/truetype/arphic/uming.ttc",
    ]
}

#[cfg(all(unix, not(target_os = "macos")))]
fn symbol_font_candidates() -> &'static [&'static str] {
    &[
        "/usr/share/fonts/truetype/noto/NotoSansSymbols-Regular.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ]
}

/// 依次尝试候选路径，加载第一个可读字体并追加为两个字体族的兜底
fn install_font(fonts: &mut egui::FontDefinitions, name: &str, candidates: &[&str]) -> bool {
    for path in candidates {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let arc = std::sync::Arc::new(egui::FontData::from_owned(bytes));
        fonts.font_data.insert(name.to_owned(), arc);
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .push(name.to_owned());
        }
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    /// 中文字符必须有可用字形，否则界面会显示为方框
    #[test]
    fn cjk_glyphs_are_available() {
        let ctx = egui::Context::default();
        // 系统没装中文字体时无从校验，直接跳过（例如精简容器环境）
        if !super::install_cjk_fonts(&ctx) {
            return;
        }
        let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        output.textures_delta.clear();
        let ok = ctx.fonts_mut(|fonts| {
            fonts.has_glyphs(&egui::FontId::new(14.0, egui::FontFamily::Proportional), "中文测试")
                && fonts
                    .has_glyphs(&egui::FontId::new(14.0, egui::FontFamily::Monospace), "中文测试")
        });
        assert!(ok, "中文字符应能渲染，否则会显示为方框");
    }

    /// Windows 上额外校验界面用到的几何符号字形
    #[cfg(target_os = "windows")]
    #[test]
    fn symbol_glyphs_are_available() {
        let ctx = egui::Context::default();
        super::install_cjk_fonts(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        output.textures_delta.clear();
        let ok = ctx.fonts_mut(|fonts| {
            fonts.has_glyphs(
                &egui::FontId::new(14.0, egui::FontFamily::Proportional),
                "▸▾✖▶★☆●×",
            ) && fonts.has_glyphs(
                &egui::FontId::new(14.0, egui::FontFamily::Monospace),
                "▸▾✖▶★☆●×",
            )
        });
        assert!(ok, "图标字符应能渲染，否则会显示为方框");
    }
}
