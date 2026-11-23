use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use httpdeck_lib::models::{FileNode, RequestSummary};

/// 右键菜单/行内按钮触发的文件操作
pub enum TreeAction {
    /// 在指定目录新建文件
    NewFile(PathBuf),
    Rename(PathBuf),
    SaveAs(PathBuf),
    Duplicate(PathBuf),
    Delete(PathBuf),
    SetPinned(PathBuf, bool),
    /// 递归运行目录下所有 .http 文件
    RunFolder(PathBuf),
    /// 展开 / 折叠文件的请求列表
    ToggleRequests(PathBuf),
    /// 打开文件并选中第 n 个请求
    OpenRequest(PathBuf, usize),
}

/// 目录树渲染所需的共享状态
pub struct TreeState<'a> {
    pub pinned: &'a [String],
    pub selected: Option<&'a Path>,
    /// 已展开请求列表的文件路径
    pub expanded: &'a HashSet<String>,
    /// 已解析的请求列表（懒解析，展开时才写入）
    pub file_requests: &'a HashMap<String, Vec<RequestSummary>>,
}

/// 递归渲染目录树；点击文件时通过 `open` 返回待打开的路径，
/// 其余操作通过 `action` 返回
pub fn render(
    ui: &mut egui::Ui,
    node: &FileNode,
    state: &TreeState<'_>,
    open: &mut Option<PathBuf>,
    action: &mut Option<TreeAction>,
) {
    if node.is_dir {
        let response = egui::CollapsingHeader::new(&node.name)
            .id_salt(&node.path)
            .default_open(true)
            .show(ui, |ui| {
                for child in &node.children {
                    render(ui, child, state, open, action);
                }
            });
        response.header_response.context_menu(|ui| {
            if ui.button("新建文件").clicked() {
                *action = Some(TreeAction::NewFile(PathBuf::from(&node.path)));
                ui.close();
            }
            if ui
                .button("运行文件夹")
                .on_hover_text("递归执行该目录下所有 .http 文件")
                .clicked()
            {
                *action = Some(TreeAction::RunFolder(PathBuf::from(&node.path)));
                ui.close();
            }
        });
    } else {
        render_file(
            ui,
            node,
            state.pinned.contains(&node.rel_path),
            state,
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
    state: &TreeState<'_>,
    open: &mut Option<PathBuf>,
    action: &mut Option<TreeAction>,
) {
    let is_selected = state.selected == Some(Path::new(&node.path));
    let expanded = state.expanded.contains(&node.path);
    let label = if pinned {
        format!("★ {}", node.name)
    } else {
        node.name.clone()
    };
    let mut open_clicked = false;
    let mut toggle_clicked = false;
    let mut pin_clicked = false;
    let mut context = None;
    ui.horizontal(|ui| {
        // 用左右分栏布局避免手算宽度：右侧按钮先占位，左侧文件名自动填满剩余空间
        egui::Sides::new()
            .height(ui.spacing().interact_size.y)
            .show(
                ui,
                |ui| {
                    // 右侧加一个占满剩余空间的 Atom，让文件名始终靠左，而不是按钮默认居中
                    let response = ui.add(egui::Button::selectable(
                        is_selected,
                        (label, egui::Atom::grow()),
                    ));
                    if response.clicked() {
                        open_clicked = true;
                    }
                    context = Some(response);
                },
                |ui| {
                    if ui
                        .small_button(if expanded { "▾" } else { "▸" })
                        .on_hover_text(if expanded {
                            "折叠请求列表"
                        } else {
                            "展开请求列表"
                        })
                        .clicked()
                    {
                        toggle_clicked = true;
                    }
                    if ui
                        .small_button(if pinned { "★" } else { "☆" })
                        .on_hover_text(if pinned {
                            "取消置顶"
                        } else {
                            "置顶文件"
                        })
                        .clicked()
                    {
                        pin_clicked = true;
                    }
                },
            );
    });
    if let Some(response) = context {
        response.context_menu(|ui| {
            file_context_menu(ui, node, pinned, action);
        });
    }
    if open_clicked {
        *open = Some(PathBuf::from(&node.path));
    }
    if toggle_clicked {
        *action = Some(TreeAction::ToggleRequests(PathBuf::from(&node.path)));
    }
    if pin_clicked {
        *action = Some(TreeAction::SetPinned(PathBuf::from(&node.path), !pinned));
    }

    if !expanded {
        return;
    }
    ui.indent(&node.path, |ui| match state.file_requests.get(&node.path) {
        None => {
            ui.weak("没有可运行的请求");
        }
        Some(requests) if requests.is_empty() => {
            ui.weak("没有可运行的请求");
        }
        Some(requests) => {
            for request in requests {
                let text = format!("{}  {}  L{}", request.method, request.name, request.line);
                if ui
                    .selectable_label(false, text)
                    .on_hover_text(format!("{} {}", request.method, request.url))
                    .clicked()
                {
                    *action = Some(TreeAction::OpenRequest(
                        PathBuf::from(&node.path),
                        request.index,
                    ));
                }
            }
        }
    });
}

fn file_context_menu(
    ui: &mut egui::Ui,
    node: &FileNode,
    pinned: bool,
    action: &mut Option<TreeAction>,
) {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_node(name: &str) -> FileNode {
        FileNode {
            name: name.to_owned(),
            path: format!("/tmp/{name}"),
            rel_path: name.to_owned(),
            is_dir: false,
            children: Vec::new(),
        }
    }

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
        let mut texts = Vec::new();
        let mut draw = |events| {
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
                let state = TreeState {
                    pinned: &[],
                    selected: None,
                    expanded: &HashSet::new(),
                    file_requests: &HashMap::new(),
                };
                render(ui, &node, &state, &mut open, &mut action);
            });
            output.textures_delta.clear();
            texts = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                    _ => None,
                })
                .collect();
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
        assert!(
            texts.iter().any(|text| text == "运行文件夹"),
            "目录菜单应包含运行文件夹: {texts:?}"
        );
    }

    #[test]
    fn expanded_file_lists_requests() {
        let node = file_node("demo.http");
        let expanded: HashSet<String> = [node.path.clone()].into_iter().collect();
        let mut requests = HashMap::new();
        requests.insert(
            node.path.clone(),
            vec![RequestSummary {
                index: 0,
                name: "登录".to_owned(),
                method: "POST".to_owned(),
                url: "http://localhost/login".to_owned(),
                line: 3,
                has_pre_script: false,
                has_post_script: false,
                body_kind: "inline".to_owned(),
            }],
        );
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 300.0),
                )),
                ..Default::default()
            },
            |ui| {
                let state = TreeState {
                    pinned: &[],
                    selected: None,
                    expanded: &expanded,
                    file_requests: &requests,
                };
                let mut open = None;
                let mut action = None;
                render_file(ui, &node, false, &state, &mut open, &mut action);
            },
        );
        output.textures_delta.clear();
        let texts: Vec<String> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        assert!(
            texts.iter().any(|text| text.contains("L3")),
            "展开后应显示请求行号，实际渲染: {texts:?}"
        );
    }

    fn dir_node(name: &str, path: &str, children: Vec<FileNode>) -> FileNode {
        FileNode {
            name: name.to_owned(),
            path: path.to_owned(),
            rel_path: name.to_owned(),
            is_dir: true,
            children,
        }
    }

    /// 不同路径下的同名目录应独立展开/折叠
    #[test]
    fn same_named_dirs_toggle_independently() {
        let tree = dir_node(
            "root",
            "/tmp/root",
            vec![
                dir_node("api", "/tmp/root/api", vec![file_node("login.http")]),
                dir_node(
                    "v1",
                    "/tmp/root/v1",
                    vec![dir_node(
                        "api",
                        "/tmp/root/v1/api",
                        vec![file_node("other.http")],
                    )],
                ),
            ],
        );
        let ctx = egui::Context::default();
        let draw = |events| -> Vec<(String, egui::Pos2)> {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 400.0),
                )),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                let state = TreeState {
                    pinned: &[],
                    selected: None,
                    expanded: &HashSet::new(),
                    file_requests: &HashMap::new(),
                };
                let mut open = None;
                let mut action = None;
                render(ui, &tree, &state, &mut open, &mut action);
            });
            output.textures_delta.clear();
            output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) => Some((text.galley.text().to_owned(), text.pos)),
                    _ => None,
                })
                .collect()
        };

        let texts = draw(Vec::new());
        assert!(
            texts.iter().filter(|(t, _)| t == "api").count() == 2,
            "两个同名目录都应渲染: {texts:?}"
        );
        assert!(texts.iter().any(|(t, _)| t == "login.http"));
        assert!(texts.iter().any(|(t, _)| t == "other.http"));

        // 点击第一个 api 目录折叠
        let pos = texts
            .iter()
            .find(|(t, _)| t == "api")
            .map(|(_, pos)| *pos)
            .unwrap()
            + egui::vec2(10.0, 5.0);
        draw(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        draw(vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        // 折叠有动画，多跑几帧等动画结束
        let mut texts = Vec::new();
        for _ in 0..40 {
            texts = draw(Vec::new());
        }

        assert!(
            !texts.iter().any(|(t, _)| t == "login.http"),
            "被折叠目录的子文件应隐藏: {texts:?}"
        );
        assert!(
            texts.iter().any(|(t, _)| t == "other.http"),
            "同名但不同路径的目录不应一起折叠: {texts:?}"
        );
    }
}
