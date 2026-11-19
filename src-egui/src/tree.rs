use std::path::{Path, PathBuf};

use httpdeck_lib::models::FileNode;

/// 右键菜单触发的文件操作
pub enum TreeAction {
    /// 在指定目录新建文件
    NewFile(PathBuf),
    Rename(PathBuf),
    SaveAs(PathBuf),
    Duplicate(PathBuf),
    Delete(PathBuf),
    SetPinned(PathBuf, bool),
}

/// 递归渲染目录树；点击文件时通过 `open` 返回待打开的路径，
/// 右键菜单通过 `action` 返回文件操作
pub fn render(
    ui: &mut egui::Ui,
    node: &FileNode,
    pinned: &[String],
    selected: Option<&Path>,
    open: &mut Option<PathBuf>,
    action: &mut Option<TreeAction>,
) {
    if node.is_dir {
        let response = egui::CollapsingHeader::new(&node.name)
            .id_salt(&node.path)
            .default_open(true)
            .show(ui, |ui| {
                for child in &node.children {
                    render(ui, child, pinned, selected, open, action);
                }
            });
        response.header_response.context_menu(|ui| {
            if ui.button("新建文件").clicked() {
                *action = Some(TreeAction::NewFile(PathBuf::from(&node.path)));
                ui.close();
            }
        });
    } else {
        render_file(
            ui,
            node,
            pinned.contains(&node.rel_path),
            selected,
            open,
            action,
        );
    }
}

/// 渲染单个文件；目录树与置顶区共用同一套打开和右键操作
pub fn render_file(
    ui: &mut egui::Ui,
    node: &FileNode,
    pinned: bool,
    selected: Option<&Path>,
    open: &mut Option<PathBuf>,
    action: &mut Option<TreeAction>,
) {
    let is_selected = selected == Some(Path::new(&node.path));
    let size = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
    let label = if pinned {
        format!("★ {}", node.name)
    } else {
        node.name.clone()
    };
    // 右侧加一个占满剩余空间的 Atom，让文件名始终靠左，而不是按钮默认居中
    let response = ui.add_sized(
        size,
        egui::Button::selectable(is_selected, (label, egui::Atom::grow())),
    );
    if response.clicked() {
        *open = Some(PathBuf::from(&node.path));
    }
    response.context_menu(|ui| {
        if ui.button("新建文件").clicked() {
            let dir = Path::new(&node.path)
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            *action = Some(TreeAction::NewFile(dir));
            ui.close();
        }
        if ui.button("重命名").clicked() {
            *action = Some(TreeAction::Rename(PathBuf::from(&node.path)));
            ui.close();
        }
        if ui.button("另存为").clicked() {
            *action = Some(TreeAction::SaveAs(PathBuf::from(&node.path)));
            ui.close();
        }
        if ui.button("复制").clicked() {
            *action = Some(TreeAction::Duplicate(PathBuf::from(&node.path)));
            ui.close();
        }
        if ui.button("删除").clicked() {
            *action = Some(TreeAction::Delete(PathBuf::from(&node.path)));
            ui.close();
        }
        let pin_label = if pinned {
            "取消置顶"
        } else {
            "置顶文件"
        };
        if ui.button(pin_label).clicked() {
            *action = Some(TreeAction::SetPinned(PathBuf::from(&node.path), !pinned));
            ui.close();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_context_menu_opens() {
        let node = FileNode {
            name: "api".to_owned(),
            path: "/tmp/api".to_owned(),
            rel_path: "api".to_owned(),
            is_dir: true,
            children: Vec::new(),
        };
        let ctx = egui::Context::default();
        let draw = |events| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 300.0),
                )),
                events,
                ..Default::default()
            };
            let mut open = None;
            let mut action = None;
            let mut output = ctx.run_ui(input, |ui| {
                render(ui, &node, &[], None, &mut open, &mut action);
            });
            output.textures_delta.clear();
        };
        let pos = egui::pos2(20.0, 10.0);
        draw(Vec::new());
        draw(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Secondary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        draw(vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Secondary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        draw(Vec::new());
        assert!(ctx.any_popup_open(), "目录右键应弹出菜单");
    }
}
