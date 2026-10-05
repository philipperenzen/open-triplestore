package org.opentriplestore.reasoner;

import java.util.LinkedHashSet;
import java.util.Set;

import org.semanticweb.owlapi.model.OWLLiteral;

/** A de-duplicating N-Triples writer for inferred triples. */
final class NTriples {
    static final String XSD_STRING = "http://www.w3.org/2001/XMLSchema#string";
    static final String RDF_PLAIN_LITERAL = "http://www.w3.org/1999/02/22-rdf-syntax-ns#PlainLiteral";
    static final String RDF_LANG_STRING = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString";

    private final Set<String> lines = new LinkedHashSet<>();

    void iri(String s, String p, String o) {
        lines.add(iriTerm(s) + " " + iriTerm(p) + " " + iriTerm(o) + " .");
    }

    void literal(String s, String p, OWLLiteral l) {
        lines.add(iriTerm(s) + " " + iriTerm(p) + " " + literalTerm(l) + " .");
    }

    int size() {
        return lines.size();
    }

    @Override
    public String toString() {
        StringBuilder b = new StringBuilder();
        for (String l : lines) {
            b.append(l).append('\n');
        }
        return b.toString();
    }

    /**
     * Split N-Triples into {subject, predicate, object} (object without the
     * final " ."). N-Triples puts no space inside a subject or predicate.
     * Lines that are not triples (blank, comments) go to {@code other}.
     */
    static java.util.List<String[]> split(String nt, StringBuilder other) {
        java.util.List<String[]> ts = new java.util.ArrayList<>();
        for (String line : nt.split("\n")) {
            String l = line.strip();
            int a = l.indexOf(' ');
            int b = a < 0 ? -1 : l.indexOf(' ', a + 1);
            if (l.isEmpty() || l.startsWith("#") || b < 0 || !l.endsWith(".")) {
                other.append(line).append('\n');
                continue;
            }
            ts.add(new String[] {l.substring(0, a), l.substring(a + 1, b), l.substring(b + 1, l.length() - 1).strip()});
        }
        return ts;
    }

    static String join(java.util.List<String[]> ts, CharSequence prefix) {
        StringBuilder out = new StringBuilder(prefix);
        for (String[] t : ts) {
            out.append(t[0]).append(' ').append(t[1]).append(' ').append(t[2]).append(" .\n");
        }
        return out.toString();
    }

    static String iriTerm(String iri) {
        StringBuilder b = new StringBuilder(iri.length() + 2).append('<');
        iri.codePoints().forEach(c -> {
            if (c <= 0x20 || c == '<' || c == '>' || c == '"' || c == '{' || c == '}'
                    || c == '|' || c == '^' || c == '`' || c == '\\') {
                b.append(String.format("\\u%04X", c));
            } else {
                b.appendCodePoint(c);
            }
        });
        return b.append('>').toString();
    }

    static String literalTerm(OWLLiteral l) {
        String lex = quote(l.getLiteral());
        if (l.hasLang()) {
            return lex + "@" + l.getLang();
        }
        String dt = l.getDatatype().getIRI().toString();
        if (dt.equals(XSD_STRING) || dt.equals(RDF_PLAIN_LITERAL) || dt.equals(RDF_LANG_STRING)) {
            return lex;
        }
        return lex + "^^" + iriTerm(dt);
    }

    static String quote(String s) {
        StringBuilder b = new StringBuilder(s.length() + 2).append('"');
        s.codePoints().forEach(c -> {
            switch (c) {
                case '"' -> b.append("\\\"");
                case '\\' -> b.append("\\\\");
                case '\n' -> b.append("\\n");
                case '\r' -> b.append("\\r");
                default -> {
                    if (c < 0x20 || c == 0x7F) {
                        b.append(String.format("\\u%04X", c));
                    } else {
                        b.appendCodePoint(c);
                    }
                }
            }
        });
        return b.append('"').toString();
    }
}
