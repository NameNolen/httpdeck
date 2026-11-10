import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { RequestResult } from "../api";
import { JsonTree } from "./JsonTree";

interface Props {
  results: RequestResult[];
  selectedIndex: number;
  label: string;
  running: boolean;
  onSelect: (index: number) => void;
  onCancel: () => void;
  height?: number | null;
}

type Tab = "body" | "headers" | "tests" | "console" | "request";

/// 响应体超过该长度时关闭自动美化和 JSON 树解析，避免卡顿
const PRETTY_LIMIT = 1_000_000;
/// 响应体超过该长度时使用虚拟滚动渲染
const VIRTUAL_LIMIT = 1_000_000;
/// 虚拟滚动行高，需与 styles.css 中 .virtual-line 保持一致
const LINE_HEIGHT = 20;
const OVERSCAN = 20;

const tabLabels: Record<Tab, string> = {
  body: "响应体",
  headers: "响应头",
  tests: "测试",
  console: "控制台",
  request: "实际请求",
};

function statusClass(status: number): string {
  if (status === 0) return "err";
  if (status >= 200 && status < 300) return "ok";
  if (status >= 300 && status < 400) return "warn";
  return "bad";
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(2)} MB`;
}

function formatTimeout(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  return ms % 1000 === 0 ? `${ms / 1000} s` : `${(ms / 1000).toFixed(1)} s`;
}

function prettyBody(result: RequestResult): string {
  if (result.bodyIsBinary) return result.body;
  if (result.body.length > PRETTY_LIMIT) return result.body;
  const parsed = parseJsonBody(result);
  if (parsed !== undefined) {
    return JSON.stringify(parsed, null, 2);
  }
  return result.body;
}

function parseJsonBody(result: RequestResult): unknown {
  if (result.bodyIsBinary) return undefined;
  const mime = (result.mimeType ?? "").toLowerCase();
  const trimmed = result.body.trim();
  const looksJson =
    mime.includes("json") || trimmed.startsWith("{") || trimmed.startsWith("[");
  if (!looksJson) return undefined;
  try {
    return JSON.parse(result.body);
  } catch {
    return undefined;
  }
}

function escapeShell(text: string): string {
  return `'${text.replace(/'/g, `'\\''`)}'`;
}

export function toCurlCommand(result: RequestResult): string {
  const lines = [`curl -X ${result.method} ${escapeShell(result.url)}`];
  for (const header of result.requestHeaders) {
    lines.push(`-H ${escapeShell(`${header.name}: ${header.value}`)}`);
  }
  if (result.requestBody) {
    lines.push(`--data ${escapeShell(result.requestBody)}`);
  }
  return lines.join(" \\\n  ");
}

function highlightText(text: string, query: string): ReactNode {
  if (!query) return text;
  const lower = text.toLowerCase();
  const needle = query.toLowerCase();
  const parts: ReactNode[] = [];
  let cursor = 0;
  let key = 0;
  for (;;) {
    const found = lower.indexOf(needle, cursor);
    if (found < 0) {
      parts.push(text.slice(cursor));
      break;
    }
    if (found > cursor) parts.push(text.slice(cursor, found));
    parts.push(<mark key={key++}>{text.slice(found, found + needle.length)}</mark>);
    cursor = found + needle.length;
  }
  return parts;
}

function tabText(result: RequestResult, tab: Tab, raw: boolean): string {
  switch (tab) {
    case "body":
      return raw ? result.body : prettyBody(result);
    case "headers":
      return result.responseHeaders
        .map((header) => `${header.name}: ${header.value}`)
        .join("\n");
    case "tests":
      return result.tests
        .map(
          (test) =>
            `${test.passed ? "PASS" : "FAIL"} ${test.name}${
              test.passed ? "" : ` - ${test.message}`
            }`
        )
        .join("\n");
    case "console":
      return result.logs.join("\n");
    case "request":
      return [
        `${result.method} ${result.url}`,
        ...result.requestHeaders.map((header) => `${header.name}: ${header.value}`),
        result.requestBody !== null ? `\n${result.requestBody}` : "",
      ]
        .filter(Boolean)
        .join("\n");
    default:
      return "";
  }
}

/// 超大响应按行虚拟滚动，只渲染可视区域
function VirtualCode({ text, query }: { text: string; query: string }) {
  const lines = useMemo(() => text.split("\n"), [text]);
  const scrollRef = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewport, setViewport] = useState(0);

  useEffect(() => {
    const element = scrollRef.current;
    if (!element) return;
    const update = () => setViewport(element.clientHeight);
    update();
    const observer = new ResizeObserver(update);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    if (scrollRef.current) scrollRef.current.scrollTop = 0;
    setScrollTop(0);
  }, [text]);

  const start = Math.max(0, Math.floor(scrollTop / LINE_HEIGHT) - OVERSCAN);
  const end = Math.min(
    lines.length,
    Math.ceil((scrollTop + viewport) / LINE_HEIGHT) + OVERSCAN
  );

  return (
    <div
      className="virtual-code"
      ref={scrollRef}
      onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
    >
      <div className="virtual-canvas" style={{ height: lines.length * LINE_HEIGHT }}>
        <div
          className="virtual-lines"
          style={{ transform: `translateY(${start * LINE_HEIGHT}px)` }}
        >
          {lines.slice(start, end).map((line, index) => (
            <div className="virtual-line" key={start + index}>
              {highlightText(line, query)}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

async function writeClipboard(text: string) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const area = document.createElement("textarea");
    area.value = text;
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    document.execCommand("copy");
    area.remove();
  }
}

export function ResponsePanel({
  results,
  selectedIndex,
  label,
  running,
  onSelect,
  onCancel,
  height,
}: Props) {
  const [tab, setTab] = useState<Tab>("body");
  const [view, setView] = useState<"pretty" | "tree" | "raw">("pretty");
  const [query, setQuery] = useState("");
  const [copied, setCopied] = useState(false);
  const result = results[selectedIndex];
  const panelStyle = height
    ? { flexGrow: 0, flexShrink: 0, flexBasis: `${height}px` }
    : undefined;
  const jsonData = useMemo(
    () =>
      result && result.body.length <= PRETTY_LIMIT
        ? parseJsonBody(result)
        : undefined,
    [result]
  );

  useEffect(() => {
    setTab("body");
    setView("pretty");
    setQuery("");
  }, [selectedIndex, results.length]);

  useEffect(() => {
    setCopied(false);
  }, [selectedIndex, tab]);

  const copyCurrent = async () => {
    if (!result) return;
    await writeClipboard(
      tabText(result, tab, view === "raw" || result.body.length > PRETTY_LIMIT)
    );
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1500);
  };

  const copyCurl = async () => {
    if (!result) return;
    await writeClipboard(toCurlCommand(result));
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1500);
  };

  if (running) {
    return (
      <section className="response-panel" style={panelStyle}>
        <div className="response-empty">
          <div className="spinner" />
          <div>正在发送请求…</div>
          <button className="secondary-button" onClick={onCancel}>
            取消请求
          </button>
        </div>
      </section>
    );
  }

  if (!result) {
    return (
      <section className="response-panel" style={panelStyle}>
        <div className="response-empty">
          <div className="empty-mark">HTTP</div>
          <div>发送请求后在此查看响应</div>
          <div className="empty-hint">点击请求列表中的 ▶，或按 Ctrl+Enter 运行光标所在请求</div>
        </div>
      </section>
    );
  }

  const passed = result.tests.filter((test) => test.passed).length;
  const oversized = result.body.length > PRETTY_LIMIT;
  const displayText =
    view === "raw" || oversized ? result.body : prettyBody(result);
  const virtualMode =
    tab === "body" &&
    view !== "tree" &&
    displayText.length > VIRTUAL_LIMIT;

  return (
    <section className="response-panel" style={panelStyle}>
      <div className="response-top">
        <div className="response-status">
          <span className={`status-badge ${statusClass(result.status)}`}>
            {result.status === 0 ? "ERR" : result.status}
          </span>
          <span className="status-text">
            {result.error ? "请求失败" : result.statusText || "OK"}
          </span>
          <span className="metric">{result.durationMs} ms</span>
          <span className="metric">{formatSize(result.size)}</span>
          {result.mimeType && <span className="metric mime">{result.mimeType}</span>}
          {result.timeoutMs != null && (
            <span className="metric flag" title="请求级超时（// @timeout）">
              超时 {formatTimeout(result.timeoutMs)}
            </span>
          )}
          {result.noRedirect ? (
            <span className="metric flag warn" title="// @no-redirect 已关闭重定向跟随">
              不跟随重定向
            </span>
          ) : (
            result.redirected && (
              <span className="metric flag" title="响应来自重定向后的最终地址">
                已跟随重定向
              </span>
            )
          )}
          {result.savedTo && (
            <span className="metric saved" title={result.savedTo}>
              已保存响应
            </span>
          )}
        </div>
        <div className="response-label">{label}</div>
      </div>

      {results.length > 1 && (
        <div className="result-strip">
          {results.map((item, index) => (
            <button
              key={`${item.requestIndex}-${index}`}
              className={`result-chip ${index === selectedIndex ? "active" : ""}`}
              onClick={() => onSelect(index)}
              title={item.url}
            >
              <span className={`mini-method ${item.method.toLowerCase()}`}>
                {item.method}
              </span>
              <span className="result-name">{item.name}</span>
              <span className={`mini-status ${statusClass(item.status)}`}>
                {item.error ? "ERR" : item.status}
              </span>
            </button>
          ))}
        </div>
      )}

      <div className="response-tabs">
        <button className={tab === "body" ? "active" : ""} onClick={() => setTab("body")}>
          响应体
        </button>
        <button className={tab === "headers" ? "active" : ""} onClick={() => setTab("headers")}>
          响应头 <span className="count">{result.responseHeaders.length}</span>
        </button>
        <button className={tab === "tests" ? "active" : ""} onClick={() => setTab("tests")}>
          测试 <span className="count">{result.tests.length}</span>
        </button>
        <button className={tab === "console" ? "active" : ""} onClick={() => setTab("console")}>
          控制台 <span className="count">{result.logs.length}</span>
        </button>
        <button className={tab === "request" ? "active" : ""} onClick={() => setTab("request")}>
          实际请求
        </button>
        <div className="tabs-actions">
          {tab === "body" && !result.bodyIsBinary && (
            <>
              <div className="view-toggle">
                <button
                  className={view === "pretty" ? "active" : ""}
                  onClick={() => setView("pretty")}
                >
                  美化
                </button>
                {jsonData !== undefined && (
                  <button
                    className={view === "tree" ? "active" : ""}
                    onClick={() => setView("tree")}
                  >
                    树形
                  </button>
                )}
                <button
                  className={view === "raw" ? "active" : ""}
                  onClick={() => setView("raw")}
                >
                  原始
                </button>
              </div>
              {view !== "tree" && (
                <input
                  className="body-search"
                  placeholder="搜索响应…"
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                />
              )}
            </>
          )}
          <button
            className="copy-button"
            onClick={() => void copyCurl()}
            title="复制为 cURL 命令"
          >
            cURL
          </button>
          <button
            className={`copy-button ${copied ? "copied" : ""}`}
            onClick={() => void copyCurrent()}
            title="复制当前标签页的内容"
          >
            {copied ? "已复制" : "⧉ 复制"}
          </button>
        </div>
      </div>

      <div className={`response-content ${virtualMode ? "virtual" : ""}`}>
        <div className="content-copy-row">
          <button
            className={`content-copy ${copied ? "copied" : ""}`}
            onClick={() => void copyCurrent()}
            title={`复制${tabLabels[tab]}`}
          >
            {copied ? "✓" : "⧉"}
          </button>
        </div>
        {result.error && <div className="banner error">{result.error}</div>}
        {result.scriptError && <div className="banner warn">脚本错误：{result.scriptError}</div>}

        {tab === "body" && (
          <>
            {result.bodyIsBinary && (
              <div className="banner info">二进制响应，以下内容为 Base64 编码</div>
            )}
            {oversized && (
              <div className="banner info">
                响应较大（{formatSize(result.size)}），已关闭自动美化与 JSON
                树；使用虚拟滚动渲染。
              </div>
            )}
            {view === "tree" && jsonData !== undefined ? (
              <JsonTree data={jsonData} />
            ) : virtualMode ? (
              <VirtualCode text={displayText} query={query} />
            ) : (
              <pre className="code-block">
                {highlightText(displayText, query)}
              </pre>
            )}
          </>
        )}

        {tab === "headers" && (
          <table className="kv-table">
            <tbody>
              {result.responseHeaders.map((header, index) => (
                <tr key={`${header.name}-${index}`}>
                  <td className="kv-name">{header.name}</td>
                  <td className="kv-value">{header.value}</td>
                </tr>
              ))}
              {result.responseHeaders.length === 0 && (
                <tr>
                  <td className="kv-empty" colSpan={2}>
                    没有响应头
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        )}

        {tab === "tests" && (
          <div className="tests">
            <div className="tests-summary">
              {result.tests.length === 0
                ? "该请求没有测试断言"
                : `${passed} / ${result.tests.length} 通过`}
            </div>
            {result.tests.map((test, index) => (
              <div className={`test-row ${test.passed ? "pass" : "fail"}`} key={index}>
                <span className="test-dot" />
                <span className="test-name">{test.name}</span>
                {!test.passed && <span className="test-message">{test.message}</span>}
              </div>
            ))}
          </div>
        )}

        {tab === "console" && (
          <div className="console">
            {result.logs.length === 0 ? (
              <div className="kv-empty">没有脚本日志（可使用 client.log 输出）</div>
            ) : (
              result.logs.map((line, index) => (
                <div className="console-line" key={index}>
                  <span className="console-index">{index + 1}</span>
                  <span>{line}</span>
                </div>
              ))
            )}
          </div>
        )}

        {tab === "request" && (
          <div className="request-detail">
            <div className="request-line">
              <span className={`mini-method ${result.method.toLowerCase()}`}>{result.method}</span>
              <span className="request-url">{result.url}</span>
            </div>
            <table className="kv-table">
              <tbody>
                {result.requestHeaders.map((header, index) => (
                  <tr key={`${header.name}-${index}`}>
                    <td className="kv-name">{header.name}</td>
                    <td className="kv-value">{header.value}</td>
                  </tr>
                ))}
              </tbody>
            </table>
            {result.requestBody !== null && (
              <pre className="code-block request-body">{result.requestBody}</pre>
            )}
          </div>
        )}
      </div>
    </section>
  );
}
