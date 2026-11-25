//! 跨平台系统托盘。
//!
//! - Windows / macOS：tray-icon 原生实现（Shell_NotifyIcon / NSStatusItem）
//! - Linux：ksni 后端（纯 DBus 的 StatusNotifierItem，不需要 GTK / libappindicator）
//!
//! 托盘事件通过 `egui::Context` 直接下发视口命令，因此不依赖 winit 事件循环注入。

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

struct TrayState {
    /// 持有 TrayIcon 才会让托盘图标常驻
    _tray: TrayIcon,
    _menu: Menu,
}

// tray-icon / muda 的内部句柄不是 Send，只能在主线程使用，
// 因此状态放在 thread_local 中（init / cleanup / available 都在主线程调用）。
thread_local! {
    static STATE: RefCell<Option<TrayState>> = const { RefCell::new(None) };
}

static EXITING: AtomicBool = AtomicBool::new(false);

/// 托盘是否已创建。没有托盘时（创建失败）允许窗口按普通方式关闭。
pub fn available() -> bool {
    STATE.with(|s| s.borrow().is_some())
}

/// 用户是否通过托盘菜单点击了「退出」
pub fn quit_requested() -> bool {
    EXITING.load(Ordering::Relaxed)
}

/// 创建托盘图标。需要在主线程、event loop 启动后调用（eframe 的 app creator 满足）。
pub fn init(ctx: &egui::Context) -> Result<(), String> {
    if available() {
        return Ok(());
    }

    let icon_data =
        eframe::icon_data::from_png_bytes(include_bytes!("../../src-tauri/icons/icon.png"))
            .map_err(|e| format!("加载托盘图标失败：{e}"))?;
    let icon = tray_icon::Icon::from_rgba(icon_data.rgba, icon_data.width, icon_data.height)
        .map_err(|e| format!("托盘图标无效：{e}"))?;

    let menu = Menu::new();
    let show_item = MenuItem::with_id("show", "显示 HttpDeck", true, None);
    let quit_item = MenuItem::with_id("quit", "退出", true, None);
    menu.append_items(&[&show_item, &PredefinedMenuItem::separator(), &quit_item])
        .map_err(|e| format!("构建托盘菜单失败：{e}"))?;

    let tray = TrayIconBuilder::new()
        .with_tooltip("HttpDeck")
        .with_icon(icon)
        .with_menu(Box::new(menu.clone()))
        .with_menu_on_left_click(false)
        .build()
        .map_err(|e| format!("创建托盘图标失败：{e}"))?;

    // 菜单项：显示窗口 / 退出
    let menu_ctx = ctx.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| match event.id.0.as_str() {
        "show" => show_main_window(&menu_ctx),
        "quit" => {
            EXITING.store(true, Ordering::Relaxed);
            menu_ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            menu_ctx.request_repaint();
        }
        _ => {}
    }));

    // 左键单击托盘图标 → 恢复主窗口
    let click_ctx = ctx.clone();
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            show_main_window(&click_ctx);
        }
    }));

    STATE.with(|s| {
        *s.borrow_mut() = Some(TrayState {
            _tray: tray,
            _menu: menu,
        });
    });
    Ok(())
}

/// 恢复主窗口（隐藏 / 最小化都能还原）
fn show_main_window(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    ctx.request_repaint();
}

/// 移除托盘图标（进程退出前调用）
pub fn cleanup() {
    STATE.with(|s| {
        s.borrow_mut().take();
    });
}
