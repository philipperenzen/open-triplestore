import { StreamLanguage, HighlightStyle, syntaxHighlighting } from '@codemirror/language';
import { tags } from '@lezer/highlight';
import { autocompletion } from '@codemirror/autocomplete';

// All SPARQL 1.1/1.2 keywords (case-insensitive)
const KEYWORDS = new Set([
  'SELECT', 'DISTINCT', 'REDUCED', 'WHERE', 'FILTER', 'OPTIONAL', 'UNION',
  'GRAPH', 'NAMED', 'FROM', 'ASK', 'CONSTRUCT', 'DESCRIBE', 'INSERT', 'DELETE',
  'WITH', 'UPDATE', 'LOAD', 'CLEAR', 'DROP', 'CREATE', 'COPY', 'MOVE', 'ADD',
  'PREFIX', 'BASE', 'LIMIT', 'OFFSET', 'ORDER', 'BY', 'ASC', 'DESC',
  'GROUP', 'HAVING', 'VALUES', 'BIND', 'SERVICE', 'MINUS', 'NOT', 'IN', 'EXISTS',
  'UNDEF', 'TRUE', 'FALSE', 'AS', 'DATA', 'ALL', 'SILENT', 'DEFAULT', 'INTO',
  // Aggregates
  'COUNT', 'SUM', 'MIN', 'MAX', 'AVG', 'SAMPLE', 'GROUP_CONCAT', 'SEPARATOR',
  // Built-in functions
  'STR', 'LANG', 'LANGMATCHES', 'DATATYPE', 'BOUND', 'IRI', 'URI', 'BNODE',
  'RAND', 'ABS', 'CEIL', 'FLOOR', 'ROUND', 'CONCAT', 'STRLEN', 'SUBSTR',
  'UCASE', 'LCASE', 'ENCODE_FOR_URI', 'CONTAINS', 'STRSTARTS', 'STRENDS',
  'STRBEFORE', 'STRAFTER', 'YEAR', 'MONTH', 'DAY', 'HOURS', 'MINUTES',
  'SECONDS', 'TIMEZONE', 'TZ', 'NOW', 'UUID', 'STRUUID', 'MD5', 'SHA1',
  'SHA256', 'SHA384', 'SHA512', 'COALESCE', 'IF', 'STRLANG', 'STRDT',
  'SAMETERM', 'ISIRI', 'ISURI', 'ISBLANK', 'ISLITERAL', 'ISNUMERIC',
  'REGEX', 'REPLACE',
]);

// SPARQL StreamLanguage tokenizer
export const sparqlLanguage = StreamLanguage.define({
  name: 'sparql',

  startState() {
    return { inString: false, stringChar: null, inIri: false, tripleQuoted: false };
  },

  token(stream, state) {
    // Inside an IRI <...>
    if (state.inIri) {
      if (stream.match(/[^>]*/)) {
        if (stream.eat('>')) {
          state.inIri = false;
          return 'string';
        }
        return 'string';
      }
      stream.next();
      return 'string';
    }

    // Inside a string literal
    if (state.inString) {
      // Triple-quoted end
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
      // Single-quoted end
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
      state.inString = true;
      state.stringChar = '"';
      state.tripleQuoted = true;
      return 'string2';
    }
    if (stream.match("'''")) {
      state.inString = true;
      state.stringChar = "'";
      state.tripleQuoted = true;
      return 'string2';
    }

    // Single-quoted strings
    if (stream.eat('"')) {
      state.inString = true;
      state.stringChar = '"';
      state.tripleQuoted = false;
      return 'string2';
    }
    if (stream.eat("'")) {
      state.inString = true;
      state.stringChar = "'";
      state.tripleQuoted = false;
      return 'string2';
    }

    // IRI
    if (stream.eat('<')) {
      // Could be < (less than) or start of IRI
      if (stream.peek() && stream.peek() !== ' ' && stream.peek() !== '=') {
        state.inIri = true;
        return 'string';
      }
      return 'operator';
    }

    // Variables
    if (stream.match(/[\?$][a-zA-Z_][a-zA-Z0-9_]*/)) return 'variableName';

    // Numbers
    if (stream.match(/[+-]?[0-9]+(\.[0-9]*)?([eE][+-]?[0-9]+)?/)) return 'number';

    // Language tag after literal @en
    if (stream.match(/@[a-zA-Z]+(-[a-zA-Z0-9]+)*/)) return 'meta';

    // Datatype annotation ^^
    if (stream.match('^^')) return 'operator';

    // Prefixed names and keywords
    if (stream.match(/[a-zA-Z_][a-zA-Z0-9_\-]*/)) {
      const word = stream.current().toUpperCase();
      if (KEYWORDS.has(word)) return 'keyword';
      // Check if followed by colon → it's a prefix
      if (stream.peek() === ':') {
        stream.eat(':');
        stream.match(/[a-zA-Z0-9_\-\.]*/);
        return 'namespace';
      }
      return 'variableName';
    }

    // Colon for bare prefix (e.g. :localName)
    if (stream.eat(':')) {
      stream.match(/[a-zA-Z0-9_\-\.]*/);
      return 'namespace';
    }

    // Punctuation
    if (stream.match(/[{}\[\]().,;*+|^]/)) return 'operator';

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

// Autocomplete options
const AUTOCOMPLETE_KEYWORDS = [
  'SELECT', 'DISTINCT', 'REDUCED', 'WHERE', 'FILTER', 'OPTIONAL', 'UNION',
  'GRAPH', 'NAMED', 'FROM', 'ASK', 'CONSTRUCT', 'DESCRIBE', 'INSERT', 'DELETE',
  'WITH', 'PREFIX', 'BASE', 'LIMIT', 'OFFSET', 'ORDER BY', 'GROUP BY', 'HAVING',
  'VALUES', 'BIND', 'SERVICE', 'MINUS', 'NOT IN', 'EXISTS', 'NOT EXISTS',
  'COUNT(*)', 'COUNT(DISTINCT', 'SUM(', 'MIN(', 'MAX(', 'AVG(', 'SAMPLE(',
  'GROUP_CONCAT(', 'SEPARATOR', 'AS',
  'STR(', 'LANG(', 'LANGMATCHES(', 'DATATYPE(', 'BOUND(', 'IRI(', 'URI(', 'BNODE(',
  'RAND()', 'ABS(', 'CEIL(', 'FLOOR(', 'ROUND(', 'CONCAT(', 'STRLEN(', 'SUBSTR(',
  'UCASE(', 'LCASE(', 'ENCODE_FOR_URI(', 'CONTAINS(', 'STRSTARTS(', 'STRENDS(',
  'STRBEFORE(', 'STRAFTER(', 'YEAR(', 'MONTH(', 'DAY(', 'HOURS(', 'MINUTES(',
  'SECONDS(', 'TIMEZONE(', 'TZ(', 'NOW()', 'UUID()', 'STRUUID()',
  'COALESCE(', 'IF(', 'SAMETERM(', 'ISIRI(', 'ISURI(', 'ISBLANK(', 'ISLITERAL(',
  'REGEX(', 'REPLACE(', 'STRDT(', 'STRLANG(',
];

const COMMON_PREFIXES_DECL = [
  'PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>',
  'PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>',
  'PREFIX owl: <http://www.w3.org/2002/07/owl#>',
  'PREFIX sh: <http://www.w3.org/ns/shacl#>',
  'PREFIX foaf: <http://xmlns.com/foaf/0.1/>',
  'PREFIX schema: <http://schema.org/>',
  'PREFIX skos: <http://www.w3.org/2004/02/skos/core#>',
  'PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>',
  'PREFIX dct: <http://purl.org/dc/terms/>',
  'PREFIX geo: <http://www.opengis.net/ont/geosparql#>',
  'PREFIX prov: <http://www.w3.org/ns/prov#>',
];

function sparqlCompletions(context) {
  const word = context.matchBefore(/\w*/);
  if (!word || (word.from === word.to && !context.explicit)) return null;

  const options = [
    ...AUTOCOMPLETE_KEYWORDS.map(label => ({ label, type: 'keyword' })),
    ...COMMON_PREFIXES_DECL.map(label => ({ label, type: 'text', detail: 'prefix' })),
  ];

  return {
    from: word.from,
    options: options.filter(o => o.label.toLowerCase().startsWith(word.text.toLowerCase())),
  };
}

// Syntax highlight style — colours chosen for contrast on white and under blue selection
export const sparqlHighlightStyle = HighlightStyle.define([
  { tag: tags.keyword,      color: '#7c3aed', fontWeight: '600' },  // keywords: SELECT WHERE PREFIX …
  { tag: tags.variableName, color: '#c05000' },                      // ?var  (dark orange)
  { tag: tags.string,       color: '#166534' },                      // <IRIs>  (dark green)
  { tag: tags.regexp,       color: '#1e3a5f' },                      // "string literals"  (navy)
  { tag: tags.namespace,    color: '#7c3aed' },                      // prefix:local
  { tag: tags.number,       color: '#0550ae', fontWeight: '500' },   // numbers / booleans
  { tag: tags.comment,      color: '#6b7280', fontStyle: 'italic' }, // # comments
  { tag: tags.operator,     color: '#374151' },                      // . ; , operators
  { tag: tags.meta,         color: '#b45309' },                      // @lang tags
]);

export const sparqlHighlighting = syntaxHighlighting(sparqlHighlightStyle);

export { sparqlCompletions };
export const sparqlAutocomplete = autocompletion({ override: [sparqlCompletions] });
