import { useEffect, useRef } from "react";
import { basicSetup } from "codemirror";
import { EditorState, StateEffect, StateField } from "@codemirror/state";
import { EditorView, GutterMarker, gutter, keymap } from "@codemirror/view";
import { indentWithTab } from "@codemirror/commands";
import type {
  Completion,
  CompletionContext,
  CompletionResult,
} from "@codemirror/autocomplete";
import { httpLanguage } from "../lib/httpLanguage";

interface ScrollTarget {
  line: number;
  nonce: number;
}

interface VariableHint {
  name: string;
  value: string;
}

interface Props {
  value: string;
  onChange: (value: string) => void;
  scrollTarget: ScrollTarget | null;
  onCursorLine?: (line: number) => void;
  runLines?: number[];
  onRunLine?: (line: number) => void;
  variables?: VariableHint[];
  onVariableHint?: (message: string) => void;
}

/// 动态变量补全列表，与后端 variables.rs 支持的语法保持一致
const DYNAMIC_VARIABLES = [
  "$uuid",
  "$timestamp",
  "$isoTimestamp",
  "$randomInt",
  "$random.uuid",
  "$random.alphabetic(8)",
  "$random.alphanumeric(12)",
  "$random.hexadecimal(16)",
  "$random.email",
  "$processEnv(NAME)",
];

const FILE_VARIABLE_RE = /^[ \t]*@([\w.$-]+)[ \t]*=/gm;
const VARIABLE_TOKEN_RE = /\{\{[ \t]*([\w.$-]+(?:\([^)]*\))?)[ \t]*\}\}/g;

/// 收集当前文档中的文件变量、环境/全局变量与动态变量，按优先级去重
function collectCompletions(
  doc: string,
  variables: VariableHint[]
): Completion[] {
  const items: Completion[] = [];
  const seen = new Set<string>();
  const add = (name: string, detail: string, info?: string) => {
    if (!name || seen.has(name)) return;
    seen.add(name);
    items.push({ label: name, detail, info, type: "variable" });
  };
  for (const match of doc.matchAll(FILE_VARIABLE_RE)) {
    const lineEnd = doc.indexOf("\n", match.index);
    const value = doc
      .slice(match.index + match[0].length, lineEnd < 0 ? doc.length : lineEnd)
      .trim();
    add(match[1], "文件变量", value);
  }
  for (const item of variables) add(item.name, "环境 / 全局", item.value);
  for (const name of DYNAMIC_VARIABLES) add(name, "动态变量");
  return items;
}

const setRunLines = StateEffect.define<ReadonlySet<number>>();

const runLinesField = StateField.define<ReadonlySet<number>>({
  create: () => new Set<number>(),
  update(value, tr) {
    for (const effect of tr.effects) {
      if (effect.is(setRunLines)) return effect.value;
    }
    return value;
  },
});

class RunMarker extends GutterMarker {
  constructor(
    private readonly lineNumber: number,
    private readonly onRun: (line: number) => void
  ) {
    super();
  }

  eq(other: GutterMarker) {
    return other instanceof RunMarker && other.lineNumber === this.lineNumber;
  }

  toDOM() {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "cm-run-btn";
    button.textContent = "▶";
    button.title = `运行第 ${this.lineNumber} 行的请求`;
    button.addEventListener("click", (event) => {
      event.preventDefault();
      event.stopPropagation();
      this.onRun(this.lineNumber);
    });
    return button;
  }
}

const runGutter = (onRun: (line: number) => void) =>
  gutter({
    class: "cm-run-gutter",
    lineMarker(view, line) {
      const lineNumber = view.state.doc.lineAt(line.from).number;
      if (!view.state.field(runLinesField).has(lineNumber)) return null;
      return new RunMarker(lineNumber, onRun);
    },
  });

const darkTheme = EditorView.theme(
  {
    "&": {
      color: "#d7dce7",
      backgroundColor: "#0f1115",
      height: "100%",
      fontSize: "13px",
    },
    ".cm-scroller": {
      fontFamily:
        '"Cascadia Code", "JetBrains Mono", Consolas, "Courier New", monospace',
      lineHeight: "1.65",
    },
    ".cm-content": { padding: "12px 0 80px", caretColor: "#7aa2ff" },
    ".cm-gutters": {
      backgroundColor: "#0f1115",
      color: "#4a5263",
      border: "none",
      paddingRight: "6px",
    },
    ".cm-activeLine": { backgroundColor: "rgba(122, 162, 255, 0.06)" },
    ".cm-activeLineGutter": { backgroundColor: "transparent", color: "#8b95a8" },
    ".cm-selectionBackground, ::selection": {
      backgroundColor: "rgba(76, 141, 255, 0.28) !important",
    },
    ".cm-cursor": { borderLeftColor: "#7aa2ff" },
    ".cm-matchingBracket": { backgroundColor: "rgba(63, 185, 80, 0.2)" },
    ".cm-tooltip": {
      backgroundColor: "#1b1f2a",
      border: "1px solid #2b3242",
      color: "#d7dce7",
    },
    ".cm-panels": { backgroundColor: "#151821", color: "#d7dce7" },
  },
  { dark: true }
);

export function CodeEditor({
  value,
  onChange,
  scrollTarget,
  onCursorLine,
  runLines,
  onRunLine,
  variables,
  onVariableHint,
}: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const onChangeRef = useRef(onChange);
  const onCursorRef = useRef(onCursorLine);
  const onRunLineRef = useRef(onRunLine);
  const variablesRef = useRef(variables);
  const onVariableHintRef = useRef(onVariableHint);
  onChangeRef.current = onChange;
  onCursorRef.current = onCursorLine;
  onRunLineRef.current = onRunLine;
  variablesRef.current = variables;
  onVariableHintRef.current = onVariableHint;

  // Ctrl+点击 `{{变量}}`：跳转到当前文件内的 @定义；否则提示来源
  const jumpToDefinition = (view: EditorView, name: string) => {
    if (name.startsWith("$")) {
      onVariableHintRef.current?.(`动态变量 ${name} 由客户端在运行时生成，无需定义`);
      return;
    }
    const doc = view.state.doc;
    const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    const pattern = new RegExp(`^[ \\t]*@${escaped}[ \\t]*=`, "m");
    const match = pattern.exec(doc.toString());
    if (match) {
      const line = doc.lineAt(match.index);
      view.dispatch({
        selection: { anchor: line.from },
        effects: EditorView.scrollIntoView(line.from, { y: "center" }),
      });
      view.focus();
      return;
    }
    const known = variablesRef.current?.find((item) => item.name === name);
    if (known) {
      onVariableHintRef.current?.(
        `变量 ${name} 来自环境 / 全局变量，当前值：${known.value}`
      );
    } else {
      onVariableHintRef.current?.(`未找到变量 ${name} 的定义`);
    }
  };

  useEffect(() => {
    if (!containerRef.current) return;
    // 输入 `{{` 后触发补全，数据来自当前文档与外部环境/全局变量
    const variableSource = (
      context: CompletionContext
    ): CompletionResult | null => {
      const before = context.matchBefore(/\{\{[ \t]*[\w.$()-]*$/);
      if (!before) return null;
      const open = /\{\{[ \t]*/.exec(before.text);
      const from = before.from + (open ? open[0].length : 2);
      return {
        from,
        options: collectCompletions(
          context.state.doc.toString(),
          variablesRef.current ?? []
        ),
        validFor: /^[\w.$()-]*$/,
      };
    };

    const view = new EditorView({
      state: EditorState.create({
        doc: value,
        extensions: [
          basicSetup,
          keymap.of([indentWithTab]),
          httpLanguage,
          EditorState.languageData.of(() => [{ autocomplete: variableSource }]),
          EditorView.domEventHandlers({
            mousedown(event, view) {
              if (!event.ctrlKey || event.button !== 0) return false;
              const pos = view.posAtCoords({
                x: event.clientX,
                y: event.clientY,
              });
              if (pos == null) return false;
              const line = view.state.doc.lineAt(pos);
              VARIABLE_TOKEN_RE.lastIndex = 0;
              let match: RegExpExecArray | null;
              while ((match = VARIABLE_TOKEN_RE.exec(line.text))) {
                const from = line.from + match.index;
                const to = from + match[0].length;
                if (pos < from || pos > to) continue;
                event.preventDefault();
                jumpToDefinition(view, match[1]);
                return true;
              }
              return false;
            },
          }),
          darkTheme,
          EditorView.lineWrapping,
          runLinesField,
          runGutter((line) => onRunLineRef.current?.(line)),
          EditorView.updateListener.of((update) => {
            if (update.docChanged) {
              onChangeRef.current(update.state.doc.toString());
            }
            if (update.selectionSet || update.docChanged) {
              const head = update.state.selection.main.head;
              onCursorRef.current?.(update.state.doc.lineAt(head).number);
            }
          }),
        ],
      }),
      parent: containerRef.current,
    });
    viewRef.current = view;
    return () => {
      view.destroy();
      viewRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    const current = view.state.doc.toString();
    if (current !== value) {
      view.dispatch({
        changes: { from: 0, to: current.length, insert: value },
      });
    }
  }, [value]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    const next = new Set(runLines ?? []);
    const current = view.state.field(runLinesField);
    if (
      current.size === next.size &&
      [...next].every((line) => current.has(line))
    ) {
      return;
    }
    view.dispatch({ effects: setRunLines.of(next) });
  }, [runLines]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view || !scrollTarget) return;
    const lineNumber = Math.min(
      Math.max(scrollTarget.line, 1),
      view.state.doc.lines
    );
    const line = view.state.doc.line(lineNumber);
    view.dispatch({
      selection: { anchor: line.from },
      effects: EditorView.scrollIntoView(line.from, { y: "center" }),
    });
    view.focus();
  }, [scrollTarget]);

  return (
    <div
      className="editor-host"
      ref={containerRef}
      title="输入 {{ 自动补全变量；Ctrl+点击变量可跳转到定义"
    />
  );
}
