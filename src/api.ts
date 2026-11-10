import { invoke } from "@tauri-apps/api/core";

export interface KeyValue {
  name: string;
  value: string;
}

export interface RequestSummary {
  index: number;
  name: string;
  method: string;
  url: string;
  line: number;
  hasPreScript: boolean;
  hasPostScript: boolean;
  bodyKind: "none" | "inline" | "file";
}

export interface ParsedFile {
  requests: RequestSummary[];
  variables: KeyValue[];
  issues: string[];
}

export interface TestResult {
  name: string;
  passed: boolean;
  message: string;
}

export interface RequestResult {
  requestIndex: number;
  name: string;
  method: string;
  url: string;
  requestHeaders: KeyValue[];
  requestBody: string | null;
  status: number;
  statusText: string;
  responseHeaders: KeyValue[];
  body: string;
  bodyIsBinary: boolean;
  mimeType: string | null;
  size: number;
  durationMs: number;
  tests: TestResult[];
  logs: string[];
  scriptError: string | null;
  savedTo: string | null;
  error: string | null;
  timeoutMs: number | null;
  noRedirect: boolean;
  redirected: boolean;
}

export interface FileNode {
  name: string;
  path: string;
  relPath: string;
  isDir: boolean;
  children: FileNode[];
}

export interface WorkspaceInfo {
  root: string;
  name: string;
  tree: FileNode;
  pinned: string[];
  envs: string[];
  selectedEnv: string | null;
  globals: KeyValue[];
  proxy: string | null;
}

export interface FileRunResult {
  file: string;
  name: string;
  results: RequestResult[];
  error: string | null;
}

export interface HistoryEntry {
  time: string;
  file: string;
  requestIndex: number;
  name: string;
  method: string;
  url: string;
  status: number;
  durationMs: number;
  error: string | null;
}

export interface CookieInfo {
  name: string;
  value: string;
  domain: string;
  path: string;
  expires: string | null;
}

export const api = {
  initialWorkspace: () => invoke<string | null>("initial_workspace"),
  recentWorkspaces: () => invoke<string[]>("recent_workspaces"),
  openWorkspace: (path: string) => invoke<WorkspaceInfo>("open_workspace", { path }),
  closeWorkspace: (workspace: string) => invoke<void>("close_workspace", { workspace }),
  refreshWorkspace: (workspace: string) =>
    invoke<WorkspaceInfo>("refresh_workspace", { workspace }),
  readHttpFile: (path: string) => invoke<string>("read_http_file", { path }),
  saveHttpFile: (path: string, content: string) =>
    invoke<void>("save_http_file", { path, content }),
  parseHttpFile: (path: string) => invoke<ParsedFile>("parse_http_file", { path }),
  parseHttpText: (content: string) => invoke<ParsedFile>("parse_http_text", { content }),
  setPinned: (workspace: string, file: string, pinned: boolean) =>
    invoke<WorkspaceInfo>("set_pinned", { workspace, file, pinned }),
  setEnv: (workspace: string, env: string | null) =>
    invoke<WorkspaceInfo>("set_env", { workspace, env }),
  setProxy: (workspace: string, proxy: string | null) =>
    invoke<WorkspaceInfo>("set_proxy", { workspace, proxy }),
  getGlobals: (workspace: string) => invoke<KeyValue[]>("get_globals", { workspace }),
  clearGlobals: (workspace: string) => invoke<void>("clear_globals", { workspace }),
  listVariables: (workspace: string, file: string, env: string | null) =>
    invoke<KeyValue[]>("list_variables", { workspace, file, env }),
  runRequest: (workspace: string, file: string, index: number, env: string | null) =>
    invoke<RequestResult>("run_request", { workspace, file, index, env }),
  runFile: (workspace: string, file: string, env: string | null) =>
    invoke<RequestResult[]>("run_file", { workspace, file, env }),
  runFolder: (workspace: string, folder: string, env: string | null) =>
    invoke<FileRunResult[]>("run_folder", { workspace, folder, env }),
  cancelRun: () => invoke<void>("cancel_run"),
  listHistory: (workspace: string) => invoke<HistoryEntry[]>("list_history", { workspace }),
  clearHistory: (workspace: string) => invoke<void>("clear_history", { workspace }),
  listCookies: (workspace: string) => invoke<CookieInfo[]>("list_cookies", { workspace }),
  clearCookies: (workspace: string) => invoke<void>("clear_cookies", { workspace }),
  revealInExplorer: (path: string) => invoke<void>("reveal_in_explorer", { path }),
};
