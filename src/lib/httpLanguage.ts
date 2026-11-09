import { StreamLanguage } from "@codemirror/language";

interface HttpState {
  inBody: boolean;
}

const METHOD_RE = /^(GET|POST|PUT|DELETE|PATCH|HEAD|OPTIONS|TRACE|CONNECT)\b/i;

export const httpLanguage = StreamLanguage.define<HttpState>({
  startState: () => ({ inBody: false }),
  token(stream, state) {
    if (stream.sol()) {
      state.inBody = false;
      if (stream.match(/^###.*$/)) return "heading";
      if (stream.match(/^\s*(>>!?|<>)\s.*$/)) return "meta";
      if (stream.match(/^\s*>.*$/)) return "meta";
      if (stream.match(/^\s*(\/\/|#).*$/)) return "comment";
      if (stream.match(/^@[\w.$-]+\s*=/)) return "variableName";
      if (stream.match(/^\s*@[\w.$-]+\s*=/)) return "variableName";
      if (stream.match(METHOD_RE)) return "keyword";
      if (stream.match(/^[A-Za-z0-9_-]+(?=\s*:)/)) return "propertyName";
      if (stream.match(/^(\{\{.*?\}\}|https?:\/\/[^\s]*|\/[^\s]*)$/)) return "url";
      if (stream.match(/^\s*$/)) {
        state.inBody = true;
        return null;
      }
    }

    if (stream.match(/\{\{[^{}]*\}\}/)) return "variableName";
    if (stream.match(/^\s*\/\/.*$/)) return "comment";
    if (stream.match(/^\s*"[^"]*"(?=\s*:)/)) return "propertyName";
    if (stream.match(/^\s*"[^"]*"/)) return "string";
    if (stream.match(/^\s*(true|false|null)\b/)) return "atom";
    if (stream.match(/^\s*-?\d+(\.\d+)?\b/)) return "number";
    if (stream.match(/^\s*[{}[\],:]/)) return "punctuation";
    stream.next();
    return null;
  },
});
