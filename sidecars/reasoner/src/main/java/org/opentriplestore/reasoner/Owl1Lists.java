package org.opentriplestore.reasoner;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * OWL 2 Mapping to RDF Graphs, Tables 14, 15 and 18 ("... for Compatibility
 * with OWL 1 DL"), applied to the
 * N-Triples before the OWL API reads them (it does not apply them):
 *
 * <ul>
 *   <li>{@code _:x owl:intersectionOf ( y )} and {@code _:x owl:unionOf ( y )} are {@code y}
 *       wherever {@code _:x} is used, also as the subject of an axiom (Table 15);
 *   <li>{@code _:x owl:intersectionOf ()} is {@code owl:Thing}, or {@code rdfs:Literal} for a
 *       data range;
 *   <li>{@code _:x owl:unionOf ()} and {@code _:x owl:oneOf ()} are {@code owl:Nothing}, or
 *       the complement of {@code rdfs:Literal} for a data range;
 *   <li>a named class with such a list is equivalent to what it reads as (Table 18).
 * </ul>
 *
 */
final class Owl1Lists {
    private static final String RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
    private static final String OWL = "http://www.w3.org/2002/07/owl#";
    private static final String TYPE = "<" + RDF + "type>";
    private static final String FIRST = "<" + RDF + "first>";
    private static final String REST = "<" + RDF + "rest>";
    private static final String NIL = "<" + RDF + "nil>";
    private static final String RDF_LIST = "<" + RDF + "List>";
    private static final String INTERSECTION = "<" + OWL + "intersectionOf>";
    private static final String UNION = "<" + OWL + "unionOf>";
    private static final String ONE_OF = "<" + OWL + "oneOf>";
    private static final String DATATYPE = "<http://www.w3.org/2000/01/rdf-schema#Datatype>";
    private static final String LITERAL = "<http://www.w3.org/2000/01/rdf-schema#Literal>";
    private static final String THING = "<" + OWL + "Thing>";
    private static final String NOTHING = "<" + OWL + "Nothing>";
    private static final String COMPLEMENT = "<" + OWL + "datatypeComplementOf>";
    private static final String DATA_RANGE = "<" + OWL + "DataRange>";
    private static final String EQUIVALENT_CLASS = "<" + OWL + "equivalentClass>";
    private static final java.util.Set<String> CLASS_TYPES = java.util.Set.of(
            "<" + OWL + "Class>", "<http://www.w3.org/2000/01/rdf-schema#Class>", DATATYPE, DATA_RANGE);
    /** Predicates that make a node a class expression or data range. */
    private static final java.util.Set<String> DEFINING = java.util.Set.of(
            INTERSECTION, UNION, ONE_OF, "<" + OWL + "complementOf>", COMPLEMENT,
            "<" + OWL + "onProperty>", "<" + OWL + "onProperties>", "<" + OWL + "onDatatype>");

    private Owl1Lists() {}

    static String rewrite(String nt) {
        if (!nt.contains(INTERSECTION) && !nt.contains(UNION) && !nt.contains(ONE_OF)) {
            return nt;
        }
        StringBuilder other = new StringBuilder();
        List<String[]> ts = NTriples.split(nt, other);
        boolean changed = false;
        while (step(ts)) {
            changed = true;
        }
        return changed ? NTriples.join(ts, other) : nt;
    }

    /** Apply one rewrite; false when none applies. */
    private static boolean step(List<String[]> ts) {
        Map<String, List<String[]>> bySubject = new HashMap<>();
        for (String[] t : ts) {
            bySubject.computeIfAbsent(t[0], k -> new ArrayList<>()).add(t);
        }
        for (String[] t : ts) {
            String x = t[0];
            String p = t[1];
            if (!(p.equals(INTERSECTION) || p.equals(UNION) || p.equals(ONE_OF))) {
                continue;
            }
            List<String[]> about = bySubject.get(x);
            // Another expression on the same node: leave it to the parser.
            if (about.stream().anyMatch(u -> u != t && DEFINING.contains(u[1]))) {
                continue;
            }
            boolean data = about.stream().anyMatch(u -> u[1].equals(TYPE)
                    && (u[2].equals(DATATYPE) || u[2].equals(DATA_RANGE)));
            String list = t[2];
            String member = null;
            List<String[]> listTriples = List.of();
            if (!list.equals(NIL)) {
                List<String[]> cell = bySubject.getOrDefault(list, List.of());
                String first = value(cell, FIRST);
                String rest = value(cell, REST);
                long described = cell.stream().filter(u -> !(u[1].equals(TYPE) && u[2].equals(RDF_LIST))).count();
                if (!list.startsWith("_:") || described != 2 || first == null || !NIL.equals(rest)) {
                    continue; // two or more members, or not a well-formed list
                }
                if (p.equals(ONE_OF)) {
                    continue; // a one-member oneOf is fine as it is
                }
                member = first;
                listTriples = cell;
            }
            String replacement;
            if (member != null) {
                replacement = member;
            } else if (p.equals(INTERSECTION)) {
                replacement = data ? LITERAL : THING;
            } else if (data) {
                // DataComplementOf(rdfs:Literal): keep x, swap its list for a complement.
                t[1] = COMPLEMENT;
                t[2] = LITERAL;
                return true;
            } else {
                replacement = NOTHING;
            }
            ts.removeAll(listTriples);
            if (!x.startsWith("_:")) {
                // Table 18: a named class with such a list is equivalent to it.
                t[1] = EQUIVALENT_CLASS;
                t[2] = replacement;
                return true;
            }
            // Table 15: CE(_:x) is the replacement wherever _:x is used.
            ts.remove(t);
            ts.removeIf(u -> u[0].equals(x) && u[1].equals(TYPE) && CLASS_TYPES.contains(u[2]));
            for (String[] u : ts) {
                if (u[0].equals(x)) {
                    u[0] = replacement;
                }
                if (u[2].equals(x)) {
                    u[2] = replacement;
                }
            }
            return true;
        }
        return false;
    }

    private static String value(List<String[]> about, String p) {
        String v = null;
        for (String[] u : about) {
            if (u[1].equals(p)) {
                if (v != null) {
                    return null;
                }
                v = u[2];
            }
        }
        return v;
    }
}
