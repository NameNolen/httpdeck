import { useEffect, useState } from "react";
import type { FileNode, WorkspaceInfo } from "../api";

interface Props {
  workspaces: WorkspaceInfo[];
  activeRoot: string | null;
  activePath: string | null;
  dirty: Record<string, boolean>;
  running: boolean;
  onAddWorkspace: () => void;
  onSelectWorkspace: (root: string) => void;
  onRemoveWorkspace: (root: string) => void;
  onOpenFile: (node: FileNode) => void;
  onTogglePin: (root: string, node: FileNode, pinned: boolean) => void;
  onRunFolder: (root: string, node: FileNode) => void;
}

interface MenuState {
  root: string;
  name: string;
  x: number;
  y: number;
}

function findNode(node: FileNode, relPath: string): FileNode | null {
  if (!node.isDir && node.relPath === relPath) return node;
  for (const child of node.children) {
    const found = findNode(child, relPath);
    if (found) return found;
  }
  return null;
}

export function Sidebar({
  workspaces,
  activeRoot,
  activePath,
  dirty,
  running,
  onAddWorkspace,
  onSelectWorkspace,
  onRemoveWorkspace,
  onOpenFile,
  onTogglePin,
  onRunFolder,
}: Props) {
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [menu, setMenu] = useState<MenuState | null>(null);

  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    window.addEventListener("click", close);
    window.addEventListener("contextmenu", close);
    window.addEventListener("resize", close);
    return () => {
      window.removeEventListener("click", close);
      window.removeEventListener("contextmenu", close);
      window.removeEventListener("resize", close);
    };
  }, [menu]);

  const toggleDir = (path: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };

  const renderNode = (workspace: WorkspaceInfo, node: FileNode, depth: number) => {
    if (node.isDir) {
      const isCollapsed = collapsed.has(node.path);
      return (
        <div key={node.path}>
          <div
            className="tree-row dir"
            style={{ paddingLeft: 10 + depth * 14 }}
            onClick={() => toggleDir(node.path)}
            title={node.path}
          >
            <span className={`chevron ${isCollapsed ? "collapsed" : ""}`}>▸</span>
            <span className="folder-icon">▣</span>
            <span className="tree-name">{node.name}</span>
            <button
              className="row-action run-folder"
              title="运行该目录下所有 .http 文件"
              disabled={running}
              onClick={(event) => {
                event.stopPropagation();
                onRunFolder(workspace.root, node);
              }}
            >
              ▶
            </button>
          </div>
          {!isCollapsed &&
            node.children.map((child) => renderNode(workspace, child, depth + 1))}
        </div>
      );
    }

    const pinned = workspace.pinned.includes(node.relPath);
    return (
      <div
        key={node.path}
        className={`tree-row file ${activePath === node.path ? "active" : ""}`}
        style={{ paddingLeft: 10 + depth * 14 }}
        onClick={() => onOpenFile(node)}
        title={node.path}
      >
        <span className="file-icon">HTTP</span>
        <span className="tree-name">{node.name}</span>
        {dirty[node.path] && <span className="dirty-dot" />}
        <button
          className={`row-action star ${pinned ? "active" : ""}`}
          title={pinned ? "取消置顶" : "置顶到常用"}
          onClick={(event) => {
            event.stopPropagation();
            onTogglePin(workspace.root, node, !pinned);
          }}
        >
          {pinned ? "★" : "☆"}
        </button>
      </div>
    );
  };

  const renderWorkspace = (workspace: WorkspaceInfo) => {
    const pinnedNodes = workspace.pinned
      .map((rel) => findNode(workspace.tree, rel))
      .filter((node): node is FileNode => node !== null);
    const isActive = workspace.root === activeRoot;

    return (
      <section
        key={workspace.root}
        className={`workspace-group ${isActive ? "active" : ""}`}
      >
        <div
          className="workspace-head"
          title={`${workspace.root}\n单击切换，右键可移除`}
          onClick={() => onSelectWorkspace(workspace.root)}
          onContextMenu={(event) => {
            event.preventDefault();
            setMenu({
              root: workspace.root,
              name: workspace.name,
              x: event.clientX,
              y: event.clientY,
            });
          }}
        >
          <div className="workspace-title">
            <span className="workspace-badge">
              {workspace.name.slice(0, 1).toUpperCase()}
            </span>
            <div className="workspace-meta">
              <div className="workspace-name">{workspace.name}</div>
              <div className="workspace-path">{workspace.root}</div>
            </div>
          </div>
        </div>

        {pinnedNodes.length > 0 && (
          <div className="pinned-section">
            <div className="section-label">置顶常用</div>
            {pinnedNodes.map((node) => (
              <div
                key={`pin-${node.path}`}
                className={`tree-row file pinned-row ${
                  activePath === node.path ? "active" : ""
                }`}
                onClick={() => onOpenFile(node)}
                title={node.path}
              >
                <span className="pin-star">★</span>
                <span className="tree-name">{node.name}</span>
                <span className="pinned-dir">
                  {node.relPath.split("/").slice(0, -1).join("/")}
                </span>
                <button
                  className="row-action star active"
                  title="取消置顶"
                  onClick={(event) => {
                    event.stopPropagation();
                    onTogglePin(workspace.root, node, false);
                  }}
                >
                  ★
                </button>
              </div>
            ))}
          </div>
        )}

        <div className="tree-section">
          {workspace.tree.children.length === 0 ? (
            <div className="empty-tree">该目录下没有 .http / .rest 文件</div>
          ) : (
            <div className="tree">
              {workspace.tree.children.map((node) => renderNode(workspace, node, 0))}
            </div>
          )}
        </div>
      </section>
    );
  };

  return (
    <aside className="sidebar">
      <div className="sidebar-scroll">
        {workspaces.length === 0 ? (
          <div className="empty-tree">还没有添加目录</div>
        ) : (
          workspaces.map(renderWorkspace)
        )}
      </div>
      <button className="sidebar-add" onClick={onAddWorkspace} title="添加一个本地目录">
        ＋ 添加目录
      </button>

      {menu && (
        <div
          className="context-menu"
          style={{ left: menu.x, top: menu.y }}
          onClick={(event) => event.stopPropagation()}
        >
          <button
            onClick={() => {
              setMenu(null);
              onRemoveWorkspace(menu.root);
            }}
          >
            移除目录「{menu.name}」
          </button>
        </div>
      )}
    </aside>
  );
}
