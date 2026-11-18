use std::path::{Path, PathBuf};

use httpdeck_lib::models::FileNode;

/// 递归渲染目录树；点击文件时通过 `open` 返回待打开的路径
pub fn render(
    ui: &mut egui::Ui,
    node: &FileNode,
    selected: Option<&Path>,
    open: &mut Option<PathBuf>,
) {
    if node.is_dir {
        egui::CollapsingHeader::new(&node.name)
            .id_salt(&node.path)
            .default_open(true)
            .show(ui, |ui| {
                for child in &node.children {
                    render(ui, child, selected, open);
                }
            });
    } else {
        let is_selected = selected == Some(Path::new(&node.path));
        let size = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
        if ui
            .add_sized(size, egui::Button::selectable(is_selected, &node.name))
            .clicked()
        {
            *open = Some(PathBuf::from(&node.path));
        }
    }
}
