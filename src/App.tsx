import { useCallback, useEffect, useMemo, useState, type MouseEvent as ReactMouseEvent } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  api,
  type CookieInfo,
  type FileNode,
  type HistoryEntry,
  type KeyValue,
  type ParsedFile,
  type RequestResult,
  type WorkspaceInfo,
} from "./api";
import { Sidebar } from "./components/Sidebar";
import { CodeEditor } from "./components/CodeEditor";
import { ResponsePanel } from "./components/ResponsePanel";

const LAST_ACTIVE_ROOT = "httpdeck.lastActiveRoot";
const PANEL_HEIGHT_KEY = "httpdeck.panelHeight";

const basename = (path: string) => path.split(/[\\/]/).pop() ?? path;

const relOf = (root: string, path: string) =>
  path
    .slice(root.length)
    .replace(/^[\\/]+/, "")
    .replace(/\\/g, "/");

function errText(err: unknown): string {
  if (typeof err === "string") return err;
  if (err instanceof Error) return err.message;
  return String(err);
}

/// 把 Record 中的 key 从 from 迁移到 to（用于文件重命名）
function moveKey<T>(source: Record<string, T>, from: string, to: string): Record<string, T> {
  if (!(from in source)) return source;
  const next = { ...source };
  next[to] = next[from];
  delete next[from];
  return next;
}

/// 删除 Record 中的 key（用于文件删除）
function dropKey<T>(source: Record<string, T>, key: string): Record<string, T> {
  if (!(key in source)) return source;
  const next = { ...source };
  delete next[key];
  return next;
}

function dropPaths<T>(
  map: Record<string, T>,
  drop: (path: string) => boolean
): Record<string, T> {
  return Object.fromEntries(
    Object.entries(map).filter(([path]) => !drop(path))
  ) as Record<string, T>;
}

export default function App() {
  const [workspaces, setWorkspaces] = useState<WorkspaceInfo[]>([]);
  const [activeRoot, setActiveRoot] = useState<string | null>(null);
  const [openFiles, setOpenFiles] = useState<string[]>([]);
  const [activePath, setActivePath] = useState<string | null>(null);
  const [contents, setContents] = useState<Record<string, string>>({});
  const [dirty, setDirty] = useState<Record<string, boolean>>({});
  const [parsed, setParsed] = useState<Record<string, ParsedFile>>({});
  const [results, setResults] = useState<RequestResult[]>([]);
  const [resultIndex, setResultIndex] = useState(0);
  const [runLabel, setRunLabel] = useState("响应");
  const [running, setRunning] = useState(false);
  const [notice, setNotice] = useState<{ kind: "error" | "info"; message: string } | null>(
    null
  );
  const [globalsOpen, setGlobalsOpen] = useState(false);
  const [globals, setGlobals] = useState<KeyValue[]>([]);
  const [envVars, setEnvVars] = useState<KeyValue[]>([]);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [history, setHistory] = useState<HistoryEntry[]>([]);
  const [historyQuery, setHistoryQuery] = useState("");
  const [cookiesOpen, setCookiesOpen] = useState(false);
  const [cookies, setCookies] = useState<CookieInfo[]>([]);
  const [proxyOpen, setProxyOpen] = useState(false);
  const [proxyInput, setProxyInput] = useState("");
  const [fileDialog, setFileDialog] = useState<
    | { kind: "create"; root: string; parent: string; value: string }
    | { kind: "rename"; root: string; path: string; value: string }
    | { kind: "delete"; root: string; path: string }
    | null
  >(null);
  const [cursorLine, setCursorLine] = useState(1);
  const [scrollTarget, setScrollTarget] = useState<{ line: number; nonce: number } | null>(
    null
  );
  const [panelHeight, setPanelHeight] = useState<number | null>(() => {
    const saved = Number(localStorage.getItem(PANEL_HEIGHT_KEY));
    return Number.isFinite(saved) && saved > 0 ? saved : null;
  });

  // 响应面板高度拖拽：在拖动期间监听全局鼠标事件
  const startPanelDrag = useCallback((event: ReactMouseEvent<HTMLDivElement>) => {
    event.preventDefault();
    const panel = event.currentTarget.nextElementSibling as HTMLElement | null;
    if (!panel) return;
    const startY = event.clientY;
    const startHeight = panel.offsetHeight;
    const maxHeight = (panel.parentElement?.offsetHeight ?? window.innerHeight) - 220;
    document.body.classList.add("v-drag");
    const onMove = (e: MouseEvent) => {
      const next = Math.min(Math.max(startHeight + (startY - e.clientY), 140), maxHeight);
      setPanelHeight(next);
    };
    const onUp = () => {
      document.body.classList.remove("v-drag");
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      setPanelHeight((current) => {
        if (current) localStorage.setItem(PANEL_HEIGHT_KEY, String(current));
        return current;
      });
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }, []);

  const workspace = useMemo(
    () => workspaces.find((item) => item.root === activeRoot) ?? null,
    [workspaces, activeRoot]
  );
  const env = workspace?.selectedEnv ?? null;

  const upsertWorkspace = useCallback((info: WorkspaceInfo) => {
    setWorkspaces((prev) => {
      const index = prev.findIndex((item) => item.root === info.root);
      if (index < 0) return [...prev, info];
      const next = [...prev];
      next[index] = info;
      return next;
    });
  }, []);

  const addWorkspace = useCallback(
    async (path: string) => {
      try {
        const info = await api.openWorkspace(path);
        upsertWorkspace(info);
        setActiveRoot(info.root);
      } catch (err) {
        setNotice({ kind: "error", message: errText(err) });
      }
    },
    [upsertWorkspace]
  );

  const removeWorkspace = useCallback(
    async (root: string) => {
      const prefix =
        root.endsWith("\\") || root.endsWith("/") ? root : `${root}\\`;
      const underRoot = (path: string) =>
        path === root || path.startsWith(prefix) || path.startsWith(`${root}/`);
      try {
        await api.closeWorkspace(root);
      } catch (err) {
        setNotice({ kind: "error", message: errText(err) });
      }
      const rest = workspaces.filter((item) => item.root !== root);
      setWorkspaces(rest);
      setActiveRoot((current) =>
        current === root ? rest[0]?.root ?? null : current
      );
      setOpenFiles((prev) => prev.filter((path) => !underRoot(path)));
      setActivePath((prev) => (prev && underRoot(prev) ? null : prev));
      setContents((prev) => dropPaths(prev, underRoot));
      setDirty((prev) => dropPaths(prev, underRoot));
      setParsed((prev) => dropPaths(prev, underRoot));
      setResults([]);
      setRunLabel("响应");
    },
    [workspaces]
  );

  useEffect(() => {
    let cancelled = false;
    const bootstrap = async () => {
      let initial: string | null = null;
      try {
        initial = await api.initialWorkspace();
      } catch {
        // 忽略命令行目录参数错误
      }
      let recent: string[] = [];
      try {
        recent = await api.recentWorkspaces();
      } catch {
        // 历史目录不可用时忽略
      }
      const candidates: string[] = [];
      for (const path of [initial, ...recent]) {
        if (!path) continue;
        if (!candidates.some((item) => item.toLowerCase() === path.toLowerCase())) {
          candidates.push(path);
        }
      }
      const infos: WorkspaceInfo[] = [];
      for (const path of candidates.slice(0, 8)) {
        try {
          const info = await api.openWorkspace(path);
          if (cancelled) return;
          infos.push(info);
        } catch {
          // 目录已不可用
        }
      }
      if (cancelled || infos.length === 0) return;
      setWorkspaces(infos);
      const saved = localStorage.getItem(LAST_ACTIVE_ROOT);
      const preferred =
        infos.find((info) => info.root === initial) ??
        infos.find((info) => info.root === saved) ??
        infos[0];
      setActiveRoot(preferred.root);
    };
    void bootstrap();
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (activeRoot) localStorage.setItem(LAST_ACTIVE_ROOT, activeRoot);
  }, [activeRoot]);

  const pickWorkspace = useCallback(async () => {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "选择要添加的目录",
      });
      if (typeof selected === "string") {
        await addWorkspace(selected);
      }
    } catch (err) {
      setNotice({ kind: "error", message: errText(err) });
    }
  }, [addWorkspace]);

  const refreshGlobals = useCallback(async () => {
    if (!workspace) return;
    try {
      setGlobals(await api.getGlobals(workspace.root));
    } catch {
      setGlobals([]);
    }
  }, [workspace]);

  // 编辑器补全用的环境变量（按当前文件目录层级解析）
  useEffect(() => {
    if (!workspace || !activePath) {
      setEnvVars([]);
      return;
    }
    let cancelled = false;
    void api
      .listVariables(workspace.root, activePath, workspace.selectedEnv)
      .then((vars) => {
        if (!cancelled) setEnvVars(vars);
      })
      .catch(() => {
        if (!cancelled) setEnvVars([]);
      });
    return () => {
      cancelled = true;
    };
  }, [workspace, activePath]);

  const refreshHistory = useCallback(async () => {
    if (!workspace) return;
    try {
      setHistory(await api.listHistory(workspace.root));
    } catch {
      setHistory([]);
    }
  }, [workspace]);

  useEffect(() => {
    void refreshHistory();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workspace?.root]);

  useEffect(() => {
    void refreshGlobals();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workspace?.root]);

  useEffect(() => {
    if (!notice) return;
    const timer = setTimeout(() => setNotice(null), 7000);
    return () => clearTimeout(timer);
  }, [notice]);

  const openFile = useCallback(
    async (node: FileNode) => {
      const path = node.path;
      setActivePath(path);
      setOpenFiles((prev) => (prev.includes(path) ? prev : [...prev, path]));
      if (contents[path] === undefined) {
        try {
          const text = await api.readHttpFile(path);
          setContents((prev) => ({ ...prev, [path]: text }));
          setDirty((prev) => ({ ...prev, [path]: false }));
        } catch (err) {
          setNotice({ kind: "error", message: errText(err) });
          return;
        }
      }
      try {
        const fileInfo = await api.parseHttpFile(path);
        setParsed((prev) => ({ ...prev, [path]: fileInfo }));
      } catch {
        // 解析错误在编辑时提示
      }
    },
    [contents]
  );

  const openFileFromWorkspace = useCallback(
    (node: FileNode) => {
      const owner = workspaces.find(
        (item) =>
          node.path === item.root ||
          node.path.startsWith(`${item.root}\\`) ||
          node.path.startsWith(`${item.root}/`)
      );
      if (owner && owner.root !== activeRoot) setActiveRoot(owner.root);
      void openFile(node);
    },
    [workspaces, activeRoot, openFile]
  );

  const openRequestFromTree = useCallback(
    (node: FileNode, line: number) => {
      openFileFromWorkspace(node);
      setScrollTarget({ line, nonce: Date.now() });
    },
    [openFileFromWorkspace]
  );

  // 文件重命名后同步已打开的标签与缓冲区
  const migrateOpenPath = (from: string, to: string) => {
    setOpenFiles((prev) => prev.map((path) => (path === from ? to : path)));
    setActivePath((prev) => (prev === from ? to : prev));
    setContents((prev) => moveKey(prev, from, to));
    setDirty((prev) => moveKey(prev, from, to));
    setParsed((prev) => moveKey(prev, from, to));
  };

  // 文件删除后关闭对应标签
  const closeOpenPath = (path: string) => {
    const remaining = openFiles.filter((item) => item !== path);
    setOpenFiles(remaining);
    setActivePath((prev) =>
      prev === path ? remaining[remaining.length - 1] ?? null : prev
    );
    setContents((prev) => dropKey(prev, path));
    setDirty((prev) => dropKey(prev, path));
    setParsed((prev) => dropKey(prev, path));
  };

  const submitFileDialog = async () => {
    if (!fileDialog) return;
    try {
      if (fileDialog.kind === "create") {
        const [info, path] = await api.createHttpFile(
          fileDialog.root,
          fileDialog.parent,
          fileDialog.value
        );
        upsertWorkspace(info);
        setFileDialog(null);
        await openFile({
          name: basename(path),
          path,
          relPath: relOf(info.root, path),
          isDir: false,
          children: [],
        });
        return;
      }
      if (fileDialog.kind === "rename") {
        const [info, path] = await api.renameHttpFile(
          fileDialog.root,
          fileDialog.path,
          fileDialog.value
        );
        upsertWorkspace(info);
        setFileDialog(null);
        migrateOpenPath(fileDialog.path, path);
        return;
      }
      const info = await api.deleteHttpFile(fileDialog.root, fileDialog.path);
      upsertWorkspace(info);
      setFileDialog(null);
      closeOpenPath(fileDialog.path);
    } catch (err) {
      setNotice({ kind: "error", message: errText(err) });
    }
  };

  useEffect(() => {
    if (!activePath) return;
    const content = contents[activePath];
    if (content === undefined) return;
    const timer = setTimeout(() => {
      api
        .parseHttpText(content)
        .then((fileInfo) =>
          setParsed((prev) => ({ ...prev, [activePath]: fileInfo }))
        )
        .catch(() => undefined);
    }, 250);
    return () => clearTimeout(timer);
  }, [contents, activePath]);

  const saveFile = useCallback(
    async (path?: string) => {
      const target = path ?? activePath;
      if (!target) return;
      const content = contents[target];
      if (content === undefined) return;
      try {
        await api.saveHttpFile(target, content);
        setDirty((prev) => ({ ...prev, [target]: false }));
      } catch (err) {
        setNotice({ kind: "error", message: errText(err) });
      }
    },
    [activePath, contents]
  );

  const saveAllDirty = useCallback(async () => {
    for (const [path, isDirty] of Object.entries(dirty)) {
      if (isDirty) {
        await saveFile(path);
      }
    }
  }, [dirty, saveFile]);

  const activeParsed = activePath ? parsed[activePath] : null;

  const currentIndex = useMemo(() => {
    const requests = activeParsed?.requests ?? [];
    if (requests.length === 0) return null;
    let candidate = requests[0].index;
    for (const request of requests) {
      if (request.line <= cursorLine) candidate = request.index;
    }
    return candidate;
  }, [activeParsed, cursorLine]);

  const rootOf = useCallback(
    (path: string | null): string | null => {
      if (!path) return workspace?.root ?? null;
      const owner = workspaces.find(
        (item) =>
          path === item.root ||
          path.startsWith(`${item.root}\\`) ||
          path.startsWith(`${item.root}/`)
      );
      return owner?.root ?? null;
    },
    [workspaces, workspace]
  );

  const runSingle = useCallback(
    async (index: number) => {
      const root = rootOf(activePath);
      if (!root || !activePath) return;
      try {
        if (dirty[activePath]) await saveFile(activePath);
        setRunning(true);
        setNotice(null);
        const result = await api.runRequest(root, activePath, index, env);
        setResults([result]);
        setResultIndex(0);
        setRunLabel(result.name);
        void refreshGlobals();
        void refreshHistory();
      } catch (err) {
        setNotice({ kind: "error", message: errText(err) });
      } finally {
        setRunning(false);
      }
    },
    [rootOf, activePath, env, dirty, saveFile, refreshGlobals, refreshHistory]
  );

  const runWholeFile = useCallback(async () => {
    const root = rootOf(activePath);
    if (!root || !activePath) return;
    try {
      if (dirty[activePath]) await saveFile(activePath);
      setRunning(true);
      setNotice(null);
      const list = await api.runFile(root, activePath, env);
      setResults(list);
      setResultIndex(0);
      setRunLabel(`${basename(activePath)} · 全部请求`);
      if (list.length === 0) {
        setNotice({ kind: "info", message: "该文件没有可运行的请求" });
      }
      void refreshGlobals();
      void refreshHistory();
    } catch (err) {
      setNotice({ kind: "error", message: errText(err) });
    } finally {
      setRunning(false);
    }
  }, [rootOf, activePath, env, dirty, saveFile, refreshGlobals, refreshHistory]);

  const runFolder = useCallback(
    async (root: string, node: FileNode) => {
      const target = workspaces.find((item) => item.root === root);
      if (!target) return;
      try {
        await saveAllDirty();
        setRunning(true);
        setNotice(null);
        const groups = await api.runFolder(root, node.path, target.selectedEnv);
        const flat = groups.flatMap((group) => group.results);
        setResults(flat);
        setResultIndex(0);
        setRunLabel(`目录 ${node.name}`);
        const errors = groups.filter((group) => group.error);
        if (errors.length > 0) {
          setNotice({
            kind: "error",
            message: errors.map((group) => `${group.name}: ${group.error}`).join("\n"),
          });
        } else if (flat.length === 0) {
          setNotice({ kind: "info", message: "该目录下没有可运行的请求" });
        }
        if (root === activeRoot) {
          void refreshGlobals();
          void refreshHistory();
        }
      } catch (err) {
        setNotice({ kind: "error", message: errText(err) });
      } finally {
        setRunning(false);
      }
    },
    [workspaces, activeRoot, saveAllDirty, refreshGlobals, refreshHistory]
  );

  const togglePin = useCallback(
    async (root: string, node: FileNode, pinned: boolean) => {
      try {
        upsertWorkspace(await api.setPinned(root, node.path, pinned));
      } catch (err) {
        setNotice({ kind: "error", message: errText(err) });
      }
    },
    [upsertWorkspace]
  );

  const changeEnv = useCallback(
    async (value: string) => {
      if (!workspace) return;
      try {
        upsertWorkspace(await api.setEnv(workspace.root, value || null));
      } catch (err) {
        setNotice({ kind: "error", message: errText(err) });
      }
    },
    [workspace, upsertWorkspace]
  );

  const closeTab = useCallback(
    (path: string) => {
      const index = openFiles.indexOf(path);
      const next = openFiles.filter((item) => item !== path);
      setOpenFiles(next);
      if (activePath === path) {
        setActivePath(next.length > 0 ? next[Math.max(0, index - 1)] : null);
      }
    },
    [openFiles, activePath]
  );

  const scrollToRequest = useCallback((line: number) => {
    setScrollTarget((prev) => ({ line, nonce: (prev?.nonce ?? 0) + 1 }));
  }, []);

  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      const mod = event.ctrlKey || event.metaKey;
      if (mod && event.key.toLowerCase() === "s") {
        event.preventDefault();
        void saveFile();
      }
      if (mod && event.key === "Enter" && currentIndex !== null) {
        event.preventDefault();
        void runSingle(currentIndex);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [saveFile, runSingle, currentIndex]);

  const openGlobals = useCallback(async () => {
    setGlobalsOpen(true);
    await refreshGlobals();
  }, [refreshGlobals]);

  // 历史记录：打开面板、清空、按条目重发
  const openHistory = useCallback(async () => {
    setHistoryOpen(true);
    setHistoryQuery("");
    await refreshHistory();
  }, [refreshHistory]);

  const clearHistory = useCallback(async () => {
    if (!workspace) return;
    try {
      await api.clearHistory(workspace.root);
      setHistory([]);
    } catch (err) {
      setNotice({ kind: "error", message: errText(err) });
    }
  }, [workspace]);

  const rerunHistory = useCallback(
    async (entry: HistoryEntry) => {
      const root = rootOf(entry.file);
      if (!root) {
        setNotice({ kind: "error", message: `文件不在当前工作区中: ${entry.file}` });
        return;
      }
      const owner = workspaces.find((item) => item.root === root);
      setHistoryOpen(false);
      // 同时打开来源文件，便于查看上下文
      void openFile({
        name: basename(entry.file),
        path: entry.file,
        relPath: "",
        isDir: false,
        children: [],
      });
      try {
        setRunning(true);
        setNotice(null);
        const result = await api.runRequest(
          root,
          entry.file,
          entry.requestIndex,
          owner?.selectedEnv ?? null
        );
        setResults([result]);
        setResultIndex(0);
        setRunLabel(`重发 · ${entry.name}`);
        if (root === activeRoot) {
          void refreshGlobals();
          void refreshHistory();
        }
      } catch (err) {
        setNotice({ kind: "error", message: errText(err) });
      } finally {
        setRunning(false);
      }
    },
    [rootOf, workspaces, activeRoot, openFile, refreshGlobals, refreshHistory]
  );

  const filteredHistory = useMemo(() => {
    const query = historyQuery.trim().toLowerCase();
    if (!query) return history;
    return history.filter((entry) =>
      [entry.name, entry.method, entry.url, entry.file]
        .join("\n")
        .toLowerCase()
        .includes(query)
    );
  }, [history, historyQuery]);

  // Cookie 查看器
  const openCookies = useCallback(async () => {
    if (!workspace) return;
    setCookiesOpen(true);
    try {
      setCookies(await api.listCookies(workspace.root));
    } catch {
      setCookies([]);
    }
  }, [workspace]);

  const clearCookies = useCallback(async () => {
    if (!workspace) return;
    try {
      await api.clearCookies(workspace.root);
      setCookies([]);
    } catch (err) {
      setNotice({ kind: "error", message: errText(err) });
    }
  }, [workspace]);

  // 代理设置：工作区级，空为不使用代理
  const openProxy = useCallback(() => {
    setProxyInput(workspace?.proxy ?? "");
    setProxyOpen(true);
  }, [workspace]);

  const saveProxy = useCallback(async () => {
    if (!workspace) return;
    try {
      upsertWorkspace(await api.setProxy(workspace.root, proxyInput.trim() || null));
      setProxyOpen(false);
      setNotice({
        kind: "info",
        message: proxyInput.trim() ? `已设置代理 ${proxyInput.trim()}` : "已关闭代理",
      });
    } catch (err) {
      setNotice({ kind: "error", message: errText(err) });
    }
  }, [workspace, proxyInput, upsertWorkspace]);

  const clearGlobals = useCallback(async () => {
    if (!workspace) return;
    try {
      await api.clearGlobals(workspace.root);
      await refreshGlobals();
    } catch (err) {
      setNotice({ kind: "error", message: errText(err) });
    }
  }, [workspace, refreshGlobals]);

  const activeContent = activePath ? contents[activePath] ?? "" : "";
  const completionVariables = useMemo(() => {
    const map = new Map<string, string>();
    for (const item of envVars) map.set(item.name, item.value);
    for (const item of globals) map.set(item.name, item.value);
    return [...map.entries()].map(([name, value]) => ({ name, value }));
  }, [envVars, globals]);
  const issues = activeParsed?.issues ?? [];
  const runLines = useMemo(
    () => (activeParsed?.requests ?? []).map((request) => request.line),
    [activeParsed]
  );
  const runByLine = useCallback(
    (line: number) => {
      const request = activeParsed?.requests.find((item) => item.line === line);
      if (request) void runSingle(request.index);
    },
    [activeParsed, runSingle]
  );

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          <span className="brand-mark">H</span>
          <div className="brand-text">
            <span className="brand-name">HttpDeck</span>
            <span className="brand-sub">IntelliJ HTTP Client 兼容</span>
          </div>
        </div>

        <div className="topbar-center">
          {workspace && (
            <span className="workspace-chip" title={workspace.root}>
              <span className="dot" />
              {workspace.name}
              {workspaces.length > 1 && (
                <span className="chip-count">{workspaces.length} 个目录</span>
              )}
            </span>
          )}
        </div>

        <div className="topbar-actions">
          <div className="env-select">
            <label>环境</label>
            <select
              value={env ?? ""}
              onChange={(event) => void changeEnv(event.target.value)}
              disabled={!workspace || workspace.envs.length === 0}
            >
              {workspace && workspace.envs.length > 0 ? (
                workspace.envs.map((name) => (
                  <option key={name} value={name}>
                    {name}
                  </option>
                ))
              ) : (
                <option value="">无环境</option>
              )}
            </select>
          </div>
          <button
            className="ghost-button"
            onClick={() => void openGlobals()}
            disabled={!workspace}
            title="查看 client.global 变量"
          >
            变量 <span className="count">{globals.length}</span>
          </button>
          <button
            className="ghost-button"
            onClick={() => void openHistory()}
            disabled={!workspace}
            title="请求历史记录"
          >
            历史 <span className="count">{history.length}</span>
          </button>
          <button
            className="ghost-button"
            onClick={() => void openCookies()}
            disabled={!workspace}
            title="查看当前工作区的 Cookie"
          >
            Cookie
          </button>
          <button
            className="ghost-button"
            onClick={openProxy}
            disabled={!workspace}
            title="代理设置（HTTP / SOCKS5）"
          >
            代理{workspace?.proxy ? " ✓" : ""}
          </button>
          <button
            className="ghost-button"
            onClick={() => workspace && api.refreshWorkspace(workspace.root).then(upsertWorkspace)}
            disabled={!workspace}
          >
            刷新
          </button>
          <button className="ghost-button" onClick={() => void pickWorkspace()}>
            添加目录
          </button>
        </div>
      </header>

      {notice && (
        <div className={`notice ${notice.kind}`}>
          <span className="notice-text">{notice.message}</span>
          <button className="notice-close" onClick={() => setNotice(null)}>
            ×
          </button>
        </div>
      )}

      <div className="app-body">
        <Sidebar
          workspaces={workspaces}
          activeRoot={activeRoot}
          activePath={activePath}
          dirty={dirty}
          running={running}
          onAddWorkspace={() => void pickWorkspace()}
          onSelectWorkspace={setActiveRoot}
          onRemoveWorkspace={(root) => void removeWorkspace(root)}
          onOpenFile={openFileFromWorkspace}
          onOpenRequest={openRequestFromTree}
          onTogglePin={(root, node, pinned) => void togglePin(root, node, pinned)}
          onRunFolder={(root, node) => void runFolder(root, node)}
          onCreateFile={(root, parent) =>
            setFileDialog({ kind: "create", root, parent, value: "" })
          }
          onRenameFile={(root, node) =>
            setFileDialog({ kind: "rename", root, path: node.path, value: node.name })
          }
          onDeleteFile={(root, node) =>
            setFileDialog({ kind: "delete", root, path: node.path })
          }
        />

        <main className="main">
          {workspaces.length === 0 ? (
            <div className="welcome">
              <div className="welcome-card">
                <div className="welcome-logo">H</div>
                <h1>HttpDeck</h1>
                <p className="welcome-sub">
                  替代 IntelliJ IDEA HTTP Client 的桌面工具，直接运行你的{" "}
                  <code>.http</code> 文件。
                </p>
                <button className="primary-button" onClick={() => void pickWorkspace()}>
                  添加目录
                </button>
                <div className="welcome-features">
                  <div>完整 HTTP 请求与响应查看</div>
                  <div>IntelliJ 风格 .http 语法与环境变量</div>
                  <div>JavaScript 前置 / 后置脚本与断言</div>
                  <div>多目录并列、历史目录自动恢复、置顶常用文件</div>
                </div>
              </div>
            </div>
          ) : !activePath ? (
            <div className="blank-main">
              <div className="blank-mark">HTTP</div>
              <div>从左侧选择一个 .http 文件开始</div>
              <div className="blank-hint">支持 .http 与 .rest 文件，置顶后可在顶部快速打开</div>
            </div>
          ) : (
            <>
              <div className="tabbar">
                {openFiles.map((path) => (
                  <div
                    key={path}
                    className={`tab ${path === activePath ? "active" : ""}`}
                    onClick={() => setActivePath(path)}
                    title={path}
                  >
                    <span className="tab-name">{basename(path)}</span>
                    {dirty[path] && <span className="dirty-dot" />}
                    <button
                      className="tab-close"
                      onClick={(event) => {
                        event.stopPropagation();
                        closeTab(path);
                      }}
                    >
                      ×
                    </button>
                  </div>
                ))}
              </div>

              <div className="main-toolbar">
                <div className="file-crumb">
                  {workspace ? relOf(workspace.root, activePath) : activePath}
                </div>
                {issues.length > 0 && (
                  <span className="issue-badge" title={issues.join("\n")}>
                    解析提示 {issues.length}
                  </span>
                )}
                <span className="spacer" />
                <button
                  className="secondary-button"
                  onClick={() => void saveFile()}
                  disabled={!dirty[activePath]}
                >
                  保存
                </button>
                <button
                  className="primary-button compact"
                  onClick={() => void runWholeFile()}
                  disabled={running}
                >
                  ▶ 运行全部
                </button>
              </div>

              <div className="outline">
                <div className="outline-label">请求</div>
                <div className="outline-list">
                  {activeParsed && activeParsed.requests.length > 0 ? (
                    activeParsed.requests.map((request) => (
                      <div
                        key={request.index}
                        className={`outline-item ${
                          currentIndex === request.index ? "current" : ""
                        }`}
                        onClick={() => scrollToRequest(request.line)}
                        title={`第 ${request.line} 行 · ${request.url}`}
                      >
                        <span className={`method-badge ${request.method.toLowerCase()}`}>
                          {request.method}
                        </span>
                        <span className="outline-name">{request.name}</span>
                        {request.hasPreScript && (
                          <span className="script-tag" title="包含前置脚本">
                            pre
                          </span>
                        )}
                        {request.hasPostScript && (
                          <span className="script-tag" title="包含后置脚本">
                            post
                          </span>
                        )}
                        <button
                          className="outline-run"
                          disabled={running}
                          title="运行此请求"
                          onClick={(event) => {
                            event.stopPropagation();
                            void runSingle(request.index);
                          }}
                        >
                          ▶
                        </button>
                      </div>
                    ))
                  ) : (
                    <div className="outline-empty">没有解析到请求</div>
                  )}
                </div>
              </div>

              <div className="editor-wrap">
                <CodeEditor
                  value={activeContent}
                  onChange={(value) => {
                    setContents((prev) => ({ ...prev, [activePath]: value }));
                    setDirty((prev) => ({ ...prev, [activePath]: true }));
                  }}
                  scrollTarget={scrollTarget}
                  onCursorLine={setCursorLine}
                  runLines={runLines}
                  onRunLine={runByLine}
                  variables={completionVariables}
                  onVariableHint={(message) => setNotice({ kind: "info", message })}
                />
              </div>

              <div className="panel-drag" onMouseDown={startPanelDrag} title="拖拽调整响应面板高度" />
              <ResponsePanel
                results={results}
                selectedIndex={resultIndex}
                label={runLabel}
                running={running}
                onSelect={setResultIndex}
                onCancel={() => void api.cancelRun()}
                height={panelHeight}
              />
            </>
          )}
        </main>
      </div>

      {globalsOpen && (
        <div className="modal-backdrop" onClick={() => setGlobalsOpen(false)}>
          <div className="modal" onClick={(event) => event.stopPropagation()}>
            <div className="modal-head">
              <h2>client.global 变量</h2>
              <button className="icon-button" onClick={() => setGlobalsOpen(false)}>
                ×
              </button>
            </div>
            <div className="modal-body">
              {globals.length === 0 ? (
                <div className="kv-empty">
                  暂无全局变量。可在脚本中使用 <code>client.global.set(name, value)</code>{" "}
                  写入。
                </div>
              ) : (
                <table className="kv-table">
                  <tbody>
                    {globals.map((item) => (
                      <tr key={item.name}>
                        <td className="kv-name">{item.name}</td>
                        <td className="kv-value">{item.value}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </div>
            <div className="modal-foot">
              <button className="secondary-button" onClick={() => void clearGlobals()}>
                清空变量
              </button>
              <button className="primary-button compact" onClick={() => setGlobalsOpen(false)}>
                关闭
              </button>
            </div>
          </div>
        </div>
      )}

      {historyOpen && (
        <div className="modal-backdrop" onClick={() => setHistoryOpen(false)}>
          <div className="modal" onClick={(event) => event.stopPropagation()}>
            <div className="modal-head">
              <h2>请求历史</h2>
              <button className="icon-button" onClick={() => setHistoryOpen(false)}>
                ×
              </button>
            </div>
            <div className="history-search">
              <input
                autoFocus
                placeholder="搜索名称 / 方法 / URL / 文件…"
                value={historyQuery}
                onChange={(event) => setHistoryQuery(event.target.value)}
              />
            </div>
            <div className="modal-body history-body">
              {filteredHistory.length === 0 ? (
                <div className="kv-empty">
                  {history.length === 0 ? "暂无历史记录" : "没有匹配的请求"}
                </div>
              ) : (
                filteredHistory.map((entry, index) => (
                  <div
                    className="history-row"
                    key={`${entry.time}-${index}`}
                    onClick={() => void rerunHistory(entry)}
                    title={`${entry.file}\n点击重发此请求`}
                  >
                    <span className={`mini-method ${entry.method.toLowerCase()}`}>
                      {entry.method}
                    </span>
                    <span className="history-name">{entry.name}</span>
                    <span className="history-url">{entry.url}</span>
                    <span
                      className={`mini-status ${
                        entry.error ? "err" : entry.status >= 200 && entry.status < 300 ? "ok" : entry.status >= 300 && entry.status < 400 ? "warn" : "bad"
                      }`}
                    >
                      {entry.error ? "ERR" : entry.status}
                    </span>
                    <span className="history-time">
                      {new Date(entry.time).toLocaleString()}
                    </span>
                  </div>
                ))
              )}
            </div>
            <div className="modal-foot">
              <button
                className="secondary-button"
                onClick={() => void clearHistory()}
                disabled={history.length === 0}
              >
                清空历史
              </button>
              <button className="primary-button compact" onClick={() => setHistoryOpen(false)}>
                关闭
              </button>
            </div>
          </div>
        </div>
      )}
      {cookiesOpen && (
        <div className="modal-backdrop" onClick={() => setCookiesOpen(false)}>
          <div className="modal" onClick={(event) => event.stopPropagation()}>
            <div className="modal-head">
              <h2>Cookie（当前工作区）</h2>
              <button className="icon-button" onClick={() => setCookiesOpen(false)}>
                ×
              </button>
            </div>
            <div className="modal-body">
              {cookies.length === 0 ? (
                <div className="kv-empty">暂无 Cookie</div>
              ) : (
                <table className="kv-table">
                  <thead>
                    <tr>
                      <th>名称</th>
                      <th>值</th>
                      <th>域</th>
                      <th>路径</th>
                      <th>过期时间</th>
                    </tr>
                  </thead>
                  <tbody>
                    {cookies.map((cookie) => (
                      <tr key={`${cookie.domain}-${cookie.name}-${cookie.path}`}>
                        <td>{cookie.name}</td>
                        <td className="kv-mono">{cookie.value}</td>
                        <td>{cookie.domain}</td>
                        <td>{cookie.path}</td>
                        <td>{cookie.expires ?? "会话"}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </div>
            <div className="modal-foot">
              <button
                className="secondary-button"
                onClick={() => void clearCookies()}
                disabled={cookies.length === 0}
              >
                清空 Cookie
              </button>
              <button className="primary-button compact" onClick={() => setCookiesOpen(false)}>
                关闭
              </button>
            </div>
          </div>
        </div>
      )}
      {proxyOpen && (
        <div className="modal-backdrop" onClick={() => setProxyOpen(false)}>
          <div className="modal modal-narrow" onClick={(event) => event.stopPropagation()}>
            <div className="modal-head">
              <h2>代理设置</h2>
              <button className="icon-button" onClick={() => setProxyOpen(false)}>
                ×
              </button>
            </div>
            <div className="modal-body">
              <p className="modal-hint">
                留空表示不使用代理。支持 http:// 、 https:// 与 socks5:// ，
                例如 <code>http://127.0.0.1:7890</code> 或 <code>socks5://127.0.0.1:1080</code>。
                设置保存在当前工作区的 <code>.http-client/workspace.json</code>。
              </p>
              <input
                className="modal-input"
                autoFocus
                placeholder="http://127.0.0.1:7890"
                value={proxyInput}
                onChange={(event) => setProxyInput(event.target.value)}
                onKeyDown={(event) => event.key === "Enter" && void saveProxy()}
              />
            </div>
            <div className="modal-foot">
              <button className="secondary-button" onClick={() => setProxyOpen(false)}>
                取消
              </button>
              <button className="primary-button compact" onClick={() => void saveProxy()}>
                保存
              </button>
            </div>
          </div>
        </div>
      )}

      {fileDialog && (
        <div className="modal-backdrop" onClick={() => setFileDialog(null)}>
          <div className="modal modal-narrow" onClick={(event) => event.stopPropagation()}>
            <div className="modal-head">
              <h2>
                {fileDialog.kind === "create"
                  ? "新建 .http 文件"
                  : fileDialog.kind === "rename"
                    ? "重命名文件"
                    : "删除文件"}
              </h2>
              <button className="icon-button" onClick={() => setFileDialog(null)}>
                ×
              </button>
            </div>
            <div className="modal-body">
              {fileDialog.kind === "delete" ? (
                <p className="modal-hint">
                  确定删除「{basename(fileDialog.path)}」？该操作会直接从磁盘删除文件。
                </p>
              ) : (
                <>
                  <p className="modal-hint">
                    {fileDialog.kind === "create"
                      ? "未写扩展名时自动补 .http。"
                      : "未写扩展名时保留原扩展名。"}
                  </p>
                  <input
                    className="modal-input"
                    autoFocus
                    placeholder="demo.http"
                    value={fileDialog.value}
                    onChange={(event) =>
                      setFileDialog({ ...fileDialog, value: event.target.value })
                    }
                    onKeyDown={(event) => {
                      if (event.key === "Enter") void submitFileDialog();
                    }}
                  />
                </>
              )}
            </div>
            <div className="modal-foot">
              <button className="secondary-button" onClick={() => setFileDialog(null)}>
                取消
              </button>
              <button
                className={
                  fileDialog.kind === "delete"
                    ? "primary-button compact danger"
                    : "primary-button compact"
                }
                onClick={() => void submitFileDialog()}
              >
                {fileDialog.kind === "delete" ? "删除" : "确定"}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
