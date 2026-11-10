import { useState } from "react";

interface Props {
  data: unknown;
}

interface NodeProps {
  name?: string;
  value: unknown;
  depth: number;
}

const INDENT = 14;

function preview(value: unknown): string {
  if (Array.isArray(value)) return `Array(${value.length})`;
  if (value !== null && typeof value === "object") {
    return `Object(${Object.keys(value as object).length})`;
  }
  return "";
}

function Primitive({ value }: { value: unknown }) {
  if (value === null) return <span className="json-null">null</span>;
  switch (typeof value) {
    case "string":
      return <span className="json-string">&quot;{value}&quot;</span>;
    case "number":
      return <span className="json-number">{String(value)}</span>;
    case "boolean":
      return <span className="json-bool">{String(value)}</span>;
    default:
      return <span className="json-null">{String(value)}</span>;
  }
}

function JsonNode({ name, value, depth }: NodeProps) {
  const [collapsed, setCollapsed] = useState(depth >= 2);
  const isObject = value !== null && typeof value === "object";
  const entries: [string, unknown][] = isObject
    ? Array.isArray(value)
      ? value.map((item, index) => [String(index), item] as [string, unknown])
      : Object.entries(value as Record<string, unknown>)
    : [];

  if (!isObject) {
    return (
      <div className="json-row" style={{ paddingLeft: depth * INDENT }}>
        {name !== undefined && <span className="json-key">{name}: </span>}
        <Primitive value={value} />
      </div>
    );
  }

  const open = Array.isArray(value) ? "[" : "{";
  const close = Array.isArray(value) ? "]" : "}";

  return (
    <div>
      <div
        className="json-row json-toggle"
        style={{ paddingLeft: depth * INDENT }}
        onClick={() => setCollapsed((prev) => !prev)}
      >
        <span className="json-arrow">{collapsed ? "▸" : "▾"}</span>
        {name !== undefined && <span className="json-key">{name}: </span>}
        <span className="json-bracket">{open}</span>
        {collapsed && (
          <span className="json-preview">
            {" "}
            {preview(value)} {close}
          </span>
        )}
      </div>
      {!collapsed && (
        <>
          {entries.map(([key, item]) => (
            <JsonNode key={key} name={key} value={item} depth={depth + 1} />
          ))}
          <div className="json-row" style={{ paddingLeft: depth * INDENT }}>
            <span className="json-bracket">{close}</span>
          </div>
        </>
      )}
    </div>
  );
}

export function JsonTree({ data }: Props) {
  return (
    <div className="json-tree">
      <JsonNode value={data} depth={0} />
    </div>
  );
}
