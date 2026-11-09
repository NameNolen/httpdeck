import { useEffect, useState } from "react";
import type { RequestResult } from "../api";

interface Props {
  results: RequestResult[];
  selectedIndex: number;
  label: string;
  running: boolean;
  onSelect: (index: number) => void;
}

type Tab = "body" | "headers" | "tests" | "console" | "request";

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

function prettyBody(result: RequestResult): string {
  if (result.bodyIsBinary) return result.body;
  const mime = (result.mimeType ?? "").toLowerCase();
  const trimmed = result.body.trim();
  const looksJson =
    mime.includes("json") || trimmed.startsWith("{") || trimmed.startsWith("[");
  if (looksJson) {
    try {
      return JSON.stringify(JSON.parse(result.body), null, 2);
    } catch {
      return result.body;
    }
  }
  return result.body;
}

export function ResponsePanel({
  results,
  selectedIndex,
  label,
  running,
  onSelect,
}: Props) {
  const [tab, setTab] = useState<Tab>("body");
  const [raw, setRaw] = useState(false);
  const result = results[selectedIndex];

  useEffect(() => {
    setTab("body");
    setRaw(false);
  }, [selectedIndex, results.length]);

  if (running) {
    return (
      <section className="response-panel">
        <div className="response-empty">
          <div className="spinner" />
          <div>正在发送请求…</div>
        </div>
      </section>
    );
  }

  if (!result) {
    return (
      <section className="response-panel">
        <div className="response-empty">
          <div className="empty-mark">HTTP</div>
          <div>发送请求后在此查看响应</div>
          <div className="empty-hint">点击请求列表中的 ▶，或按 Ctrl+Enter 运行光标所在请求</div>
        </div>
      </section>
    );
  }

  const passed = result.tests.filter((test) => test.passed).length;

  return (
    <section className="response-panel">
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
        {tab === "body" && !result.bodyIsBinary && (
          <label className="raw-toggle">
            <input type="checkbox" checked={raw} onChange={(e) => setRaw(e.target.checked)} />
            原始
          </label>
        )}
      </div>

      <div className="response-content">
        {result.error && <div className="banner error">{result.error}</div>}
        {result.scriptError && <div className="banner warn">脚本错误：{result.scriptError}</div>}

        {tab === "body" && (
          <>
            {result.bodyIsBinary && (
              <div className="banner info">二进制响应，以下内容为 Base64 编码</div>
            )}
            <pre className="code-block">{raw ? result.body : prettyBody(result)}</pre>
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
