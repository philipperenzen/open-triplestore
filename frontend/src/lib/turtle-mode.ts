import { StreamLanguage, HighlightStyle, syntaxHighlighting } from '@codemirror/language';
import { tags } from '@lezer/highlight';

// Turtle/N3 StreamLanguage tokenizer
export const turtleLanguage = StreamLanguage.define({
  name: 'turtle',

  startState() {
    return { inString: false, stringChar: null, tripleQuoted: false, inIri: false };
  },

  token(stream, state) {
    if (state.inIri) {
      if (stream.match(/[^>]*/)) {
        if (stream.eat('>')) { state.inIri = false; }
        return 'string';
      }
      stream.next();
      return 'string';
    }

    if (state.inString) {
      if (state.tripleQuoted) {
        const end = state.stringChar.repeat(3);
        if (stream.match(end)) {
          state.inString = false;
          state.tripleQuoted = false;
          return 'string2';
        }
        stream.next();
        return 'string2';
      }
      if (stream.match(state.stringChar)) {
        state.inString = false;
        return 'string2';
      }
      if (stream.eat('\\')) stream.next();
      else stream.next();
      return 'string2';
    }

    if (stream.eatSpace()) return null;

    // Comment
    if (stream.eat('#')) {
      stream.skipToEnd();
      return 'comment';
    }

    // Triple-quoted strings
    if (stream.match('"""')) {
      state.inString = true; state.stringChar = '"'; state.tripleQuoted = true;
      return 'string2';
    }
    if (stream.match("'''")) {
      state.inString = true; state.stringChar = "'"; state.tripleQuoted = true;
      return 'string2';
    }

    // String literals
    if (stream.eat('"')) {
      state.inString = true; state.stringChar = '"'; state.tripleQuoted = false;
      return 'string2';
    }
    if (stream.eat("'")) {
      state.inString = true; state.stringChar = "'"; state.tripleQuoted = false;
      return 'string2';
    }

    // IRI
    if (stream.eat('<')) {
      if (stream.peek() && stream.peek() !== ' ' && stream.peek() !== '=') {
        state.inIri = true;
        return 'string';
      }
      return 'operator';
    }

    // @prefix / @base directives
    if (stream.match(/@(prefix|base|forSome|forAll)/)) return 'keyword';

    // Numbers
    if (stream.match(/[+-]?[0-9]+(\.[0-9]*)?([eE][+-]?[0-9]+)?/)) return 'number';

    // Language tag
    if (stream.match(/@[a-zA-Z]+(-[a-zA-Z0-9]+)*/)) return 'meta';

    // ^^datatype
    if (stream.match('^^')) return 'operator';

    // Blank nodes
    if (stream.match(/_:[a-zA-Z0-9_-]+/)) return 'variableName';

    // Keywords: a, true, false
    if (stream.match(/\b(true|false)\b/)) return 'bool';
    if (stream.match(/\ba\b/)) return 'keyword';

    // Prefixed names and prefix declarations
    if (stream.match(/[a-zA-Z_][a-zA-Z0-9_-]*/)) {
      if (stream.peek() === ':') {
        stream.eat(':');
        stream.match(/[a-zA-Z0-9_\-.]*/);
        return 'namespace';
      }
      return 'variableName';
    }

    // Bare colon
    if (stream.eat(':')) {
      stream.match(/[a-zA-Z0-9_\-.]*/);
      return 'namespace';
    }

    if (stream.match(/[{}[\]().,;!^]/)) return 'operator';

    stream.next();
    return null;
  },

  languageData: {
    commentTokens: { line: '#' },
  },

  tokenTable: {
    string2: tags.regexp,
    namespace: tags.namespace,
  },
});

export const turtleHighlightStyle = HighlightStyle.define([
  { tag: tags.keyword, color: '#6f42c1', fontWeight: 'bold' },
  { tag: tags.variableName, color: '#24292e' },
  { tag: tags.string, color: '#22863a' },
  { tag: tags.regexp,  color: '#032f62' },
  { tag: tags.namespace, color: '#6f42c1' },
  { tag: tags.number, color: '#005cc5' },
  { tag: tags.comment, color: '#6a737d', fontStyle: 'italic' },
  { tag: tags.operator, color: '#444d56' },
  { tag: tags.meta, color: '#e36209' },
]);

export const turtleHighlighting = syntaxHighlighting(turtleHighlightStyle);
