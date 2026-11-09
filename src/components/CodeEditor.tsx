import { useEffect, useRef } from "react";
import { basicSetup } from "codemirror";
import { EditorState, StateEffect, StateField } from "@codemirror/state";
import { EditorView, GutterMarker, gutter, keymap } from "@codemirror/view";
import { indentWithTab } from "@codemirror/commands";
import { httpLanguage } from "../lib/httpLanguage";

interface ScrollTarget {
  line: number;
  nonce: number;
}

interface Props {
  value: string;
  onChange: (value: string) => void;
  scrollTarget: ScrollTarget | null;
  onCursorLine?: (line: number) => void;
  runLines?: number[];
  onRunLine?: (line: number) => void;
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
}: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const onChangeRef = useRef(onChange);
  const onCursorRef = useRef(onCursorLine);
  const onRunLineRef = useRef(onRunLine);
  onChangeRef.current = onChange;
  onCursorRef.current = onCursorLine;
  onRunLineRef.current = onRunLine;

  useEffect(() => {
    if (!containerRef.current) return;
    const view = new EditorView({
      state: EditorState.create({
        doc: value,
        extensions: [
          basicSetup,
          keymap.of([indentWithTab]),
          httpLanguage,
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

  return <div className="editor-host" ref={containerRef} />;
}
