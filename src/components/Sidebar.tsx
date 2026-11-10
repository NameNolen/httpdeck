import { useEffect, useState } from "react";
import { api } from "../api";
import type { FileNode, RequestSummary, WorkspaceInfo } from "../api";

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
  onOpenRequest: (node: FileNode, line: number) => void;
  onTogglePin: (root: string, node: FileNode, pinned: boolean) => void;
  onRunFolder: (root: string, node: FileNode) => void;
  onCreateFile: (root: string, parent: string) => void;
  onRenameFile: (root: string, node: FileNode) => void;
  onDeleteFile: (root: string, node: FileNode) => void;
}

type MenuState =
  | { kind: "workspace"; root: string; name: string; x: number; y: number }
  | { kind: "folder"; root: string; node: FileNode; x: number; y: number }
  | {
      kind: "file";
      root: string;
      node: FileNode;
      pinned: boolean;
      x: number;
      y: number;
    };

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
  onOpenRequest,
  onTogglePin,
  onRunFolder,
  onCreateFile,
  onRenameFile,
  onDeleteFile,
}: Props) {
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(new Set());
  const [expandedFiles, setExpandedFiles] = useState<Set<string>>(new Set());
  const [fileRequests, setFileRequests] = useState<Record<string, RequestSummary[]>>(
    {}
  );
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

  const toggleGroup = (root: string) => {
    setCollapsedGroups((prev) => {
      const next = new Set(prev);
      if (next.has(root)) next.delete(root);
      else next.add(root);
      return next;
    });
  };

  const toggleFile = async (node: FileNode) => {
    const path = node.path;
    if (expandedFiles.has(path)) {
      setExpandedFiles((prev) => {
        const next = new Set(prev);
        next.delete(path);
        return next;
      });
      return;
    }
    setExpandedFiles((prev) => new Set(prev).add(path));
    setFileRequests((prev) => {
      const next = { ...prev };
      delete next[path];
      return next;
    });
    try {
      const parsed = await api.parseHttpFile(path);
      setFileRequests((prev) => ({ ...prev, [path]: parsed.requests }));
    } catch {
      setFileRequests((prev) => ({ ...prev, [path]: [] }));
    }
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
            onContextMenu={(event) => {
              event.preventDefault();
              event.stopPropagation();
              setMenu({
                kind: "folder",
                root: workspace.root,
                node,
                x: event.clientX,
                y: event.clientY,
              });
            }}
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
    const expanded = expandedFiles.has(node.path);
    const requests = fileRequests[node.path];

    return (
      <div key={node.path}>
        <div
          className={`tree-row file ${activePath === node.path ? "active" : ""}`}
          style={{ paddingLeft: 10 + depth * 14 }}
          onClick={() => onOpenFile(node)}
          onContextMenu={(event) => {
            event.preventDefault();
            event.stopPropagation();
            setMenu({
              kind: "file",
              root: workspace.root,
              node,
              pinned,
              x: event.clientX,
              y: event.clientY,
            });
          }}
          title={`${node.path}\n右键可展开请求列表`}
        >
          <span className="file-icon">HTTP</span>
          <span className="tree-name">{node.name}</span>
          {dirty[node.path] && <span className="dirty-dot" />}
          <button
            className="row-action file-toggle"
            title={expanded ? "折叠请求列表" : "展开请求列表"}
            onClick={(event) => {
              event.stopPropagation();
              void toggleFile(node);
            }}
          >
            {expanded ? "▾" : "▸"}
          </button>
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

        {expanded && (
          <div className="file-requests">
            {requests === undefined ? (
              <div
                className="request-hint"
                style={{ paddingLeft: 10 + (depth + 1) * 14 + 12 }}
              >
                解析中…
              </div>
            ) : requests.length === 0 ? (
              <div
                className="request-hint"
                style={{ paddingLeft: 10 + (depth + 1) * 14 + 12 }}
              >
                没有可运行的请求
              </div>
            ) : (
              requests.map((request) => (
                <div
                  key={request.index}
                  className="tree-row request"
                  style={{ paddingLeft: 10 + (depth + 1) * 14 + 12 }}
                  onClick={(event) => {
                    event.stopPropagation();
                    onOpenRequest(node, request.line);
                  }}
                  title={`${request.method} ${request.url}`}
                >
                  <span className={`mini-method ${request.method.toLowerCase()}`}>
                    {request.method}
                  </span>
                  <span className="tree-name">{request.name}</span>
                  <span className="request-line">L{request.line}</span>
                </div>
              ))
            )}
          </div>
        )}
      </div>
    );
  };

  const renderWorkspace = (workspace: WorkspaceInfo) => {
    const pinnedNodes = workspace.pinned
      .map((rel) => findNode(workspace.tree, rel))
      .filter((node): node is FileNode => node !== null);
    const isActive = workspace.root === activeRoot;
    const isCollapsed = collapsedGroups.has(workspace.root);

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
            event.stopPropagation();
            setMenu({
              kind: "workspace",
              root: workspace.root,
              name: workspace.name,
              x: event.clientX,
              y: event.clientY,
            });
          }}
        >
          <button
            className="group-toggle"
            title={isCollapsed ? "展开目录" : "折叠目录"}
            onClick={(event) => {
              event.stopPropagation();
              toggleGroup(workspace.root);
            }}
          >
            {isCollapsed ? "▸" : "▾"}
          </button>
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

        {!isCollapsed && pinnedNodes.length > 0 && (
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

        {!isCollapsed && (
          <div className="tree-section">
            {workspace.tree.children.length === 0 ? (
              <div className="empty-tree">该目录下没有 .http / .rest 文件</div>
            ) : (
              <div className="tree">
                {workspace.tree.children.map((node) => renderNode(workspace, node, 0))}
              </div>
            )}
          </div>
        )}
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
          {menu.kind === "workspace" ? (
            <>
              <button
                onClick={() => {
                  const target = menu;
                  setMenu(null);
                  onCreateFile(target.root, target.root);
                }}
              >
                新建 .http 文件
              </button>
              <button
                onClick={() => {
                  const target = menu;
                  setMenu(null);
                  void api.revealInExplorer(target.root);
                }}
              >
                在资源管理器中打开
              </button>
              <button
                className="danger"
                onClick={() => {
                  const target = menu;
                  setMenu(null);
                  onRemoveWorkspace(target.root);
                }}
              >
                移除目录「{menu.name}」
              </button>
            </>
          ) : menu.kind === "folder" ? (
            <>
              <button
                onClick={() => {
                  const target = menu as Extract<MenuState, { kind: "folder" }>;
                  setMenu(null);
                  onCreateFile(target.root, target.node.path);
                }}
              >
                新建 .http 文件
              </button>
              <button
                disabled={running}
                onClick={() => {
                  const target = menu as Extract<MenuState, { kind: "folder" }>;
                  setMenu(null);
                  onRunFolder(target.root, target.node);
                }}
              >
                运行该目录
              </button>
              <button
                onClick={() => {
                  const target = menu as Extract<MenuState, { kind: "folder" }>;
                  setMenu(null);
                  void api.revealInExplorer(target.node.path);
                }}
              >
                在资源管理器中打开
              </button>
            </>
          ) : (
            <>
              <button
                onClick={() => {
                  const target = menu as Extract<MenuState, { kind: "file" }>;
                  setMenu(null);
                  void toggleFile(target.node);
                }}
              >
                {expandedFiles.has(menu.node.path) ? "折叠请求列表" : "展开请求列表"}
              </button>
              <button
                onClick={() => {
                  const target = menu as Extract<MenuState, { kind: "file" }>;
                  setMenu(null);
                  onTogglePin(target.root, target.node, !target.pinned);
                }}
              >
                {menu.pinned ? "取消置顶" : "置顶文件"}
              </button>
              <button
                onClick={() => {
                  const target = menu as Extract<MenuState, { kind: "file" }>;
                  setMenu(null);
                  void api.revealInExplorer(target.node.path);
                }}
              >
                在资源管理器中打开
              </button>
              <button
                onClick={() => {
                  const target = menu as Extract<MenuState, { kind: "file" }>;
                  setMenu(null);
                  onRenameFile(target.root, target.node);
                }}
              >
                重命名
              </button>
              <button
                className="danger"
                onClick={() => {
                  const target = menu as Extract<MenuState, { kind: "file" }>;
                  setMenu(null);
                  onDeleteFile(target.root, target.node);
                }}
              >
                删除文件
              </button>
            </>
          )}
        </div>
      )}
    </aside>
  );
}
