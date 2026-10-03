package org.opentriplestore.reasoner;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * Declarations for undeclared properties, typed by use the way the server's
 * own RDF → OWL 2 mapping types them (src/reasoning/owl_mapping.rs), so both
 * read the same input the same way. Without them the OWL API reads an
 * undeclared predicate between two IRIs as an annotation.
 *
 * <ul>
 *   <li>a predicate of an assertion is a data property when its object is a
 *       literal, an object property otherwise (both: punning, refused later);
 *   <li>a property used only in schema position ({@code owl:onProperty},
 *       {@code rdfs:subPropertyOf}, {@code owl:equivalentProperty},
 *       {@code owl:propertyDisjointWith}, {@code rdfs:domain}, {@code rdfs:range},
 *       {@code owl:inverseOf}, {@code owl:propertyChainAxiom}) is a data
 *       property when a restriction on it has a data filler or its range is a
 *       datatype, an object property otherwise.
 * </ul>
 */
final class TypingByUse {
    private static final String RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
    private static final String RDFS = "http://www.w3.org/2000/01/rdf-schema#";
    private static final String OWL = "http://www.w3.org/2002/07/owl#";
    private static final String XSD = "http://www.w3.org/2001/XMLSchema#";
    private static final String TYPE = "<" + RDF + "type>";
    static final String OBJECT = "<" + OWL + "ObjectProperty>";
    static final String DATA = "<" + OWL + "DatatypeProperty>";
    private static final String ANNOTATION = "<" + OWL + "AnnotationProperty>";
    private static final String DATATYPE = "<" + RDFS + "Datatype>";
    private static final Set<String> OBJECT_ONLY = Set.of(
            "<" + OWL + "TransitiveProperty>", "<" + OWL + "SymmetricProperty>",
            "<" + OWL + "AsymmetricProperty>", "<" + OWL + "ReflexiveProperty>",
            "<" + OWL + "IrreflexiveProperty>", "<" + OWL + "InverseFunctionalProperty>");
    private static final Set<String> SCHEMA = Set.of(
            "<" + RDFS + "subPropertyOf>", "<" + OWL + "equivalentProperty>",
            "<" + OWL + "propertyDisjointWith>", "<" + RDFS + "domain>", "<" + RDFS + "range>",
            "<" + OWL + "inverseOf>", "<" + OWL + "propertyChainAxiom>");
    private static final List<String> FILLERS = List.of(
            "<" + OWL + "onDataRange>", "<" + OWL + "someValuesFrom>",
            "<" + OWL + "allValuesFrom>", "<" + OWL + "hasValue>");

    private TypingByUse() {}

    static boolean reserved(String term) {
        return term.startsWith("<" + RDF) || term.startsWith("<" + RDFS)
                || term.startsWith("<" + OWL) || term.startsWith("<" + XSD);
    }

    /** N-Triples declaration lines for the undeclared properties of {@code ts}. */
    static List<String> declarations(List<String[]> ts) {
        Map<String, Set<String>> kinds = new HashMap<>();
        Set<String> datatypes = new HashSet<>();
        Map<String, List<String[]>> bySubject = new HashMap<>();
        for (String[] t : ts) {
            bySubject.computeIfAbsent(t[0], k -> new ArrayList<>()).add(t);
            if (t[1].equals(TYPE)) {
                String o = t[2];
                if (o.equals(OBJECT) || OBJECT_ONLY.contains(o)) {
                    kinds.computeIfAbsent(t[0], k -> new HashSet<>()).add(OBJECT);
                } else if (o.equals(DATA)) {
                    kinds.computeIfAbsent(t[0], k -> new HashSet<>()).add(DATA);
                } else if (o.equals(ANNOTATION)) {
                    kinds.computeIfAbsent(t[0], k -> new HashSet<>()).add(ANNOTATION);
                } else if (o.equals(DATATYPE)) {
                    datatypes.add(t[0]);
                }
            }
        }
        Map<String, Set<String>> add = new LinkedHashMap<>();
        // Assertions.
        for (String[] t : ts) {
            String p = t[1];
            if (reserved(p) || kinds.containsKey(p)) {
                continue;
            }
            add.computeIfAbsent(p, k -> new HashSet<>()).add(t[2].startsWith("\"") ? DATA : OBJECT);
        }
        // Schema-only properties, settled after the assertions.
        Map<String, String> schema = new LinkedHashMap<>();
        for (String[] t : ts) {
            if (t[1].equals("<" + OWL + "onProperty>") && t[2].startsWith("<")) {
                boolean data = false;
                for (String[] f : bySubject.getOrDefault(t[0], List.of())) {
                    if (FILLERS.contains(f[1]) && isDataFiller(f[2], datatypes, bySubject)) {
                        data = true;
                    }
                }
                schema.merge(t[2], data ? DATA : OBJECT, (a, b) -> a.equals(DATA) ? a : b);
            } else if (SCHEMA.contains(t[1])) {
                String kind = null;
                if (t[1].equals("<" + RDFS + "range>")) {
                    kind = isDatatype(t[2], datatypes) ? DATA : OBJECT;
                } else if (t[1].equals("<" + OWL + "inverseOf>") || t[1].equals("<" + OWL + "propertyChainAxiom>")) {
                    kind = OBJECT;
                }
                if (t[0].startsWith("<")) {
                    schema.merge(t[0], kind == null ? "" : kind, (a, b) -> a.isEmpty() ? b : a);
                }
                boolean objectIsProperty = !t[1].equals("<" + RDFS + "domain>") && !t[1].equals("<" + RDFS + "range>")
                        && !t[1].equals("<" + OWL + "propertyChainAxiom>");
                if (objectIsProperty && t[2].startsWith("<")) {
                    schema.merge(t[2], kind == null ? "" : kind, (a, b) -> a.isEmpty() ? b : a);
                }
            }
        }
        for (Map.Entry<String, String> e : schema.entrySet()) {
            String p = e.getKey();
            if (reserved(p) || kinds.containsKey(p) || add.containsKey(p)) {
                continue;
            }
            add.computeIfAbsent(p, k -> new HashSet<>()).add(e.getValue().isEmpty() ? OBJECT : e.getValue());
        }
        List<String> out = new ArrayList<>();
        for (Map.Entry<String, Set<String>> e : add.entrySet()) {
            for (String kind : e.getValue()) {
                out.add(e.getKey() + " " + TYPE + " " + kind + " .");
            }
        }
        return out;
    }

    private static boolean isDatatype(String term, Set<String> datatypes) {
        return term.startsWith("<" + XSD) || term.equals("<" + RDFS + "Literal>")
                || term.equals("<" + RDF + "PlainLiteral>") || term.equals("<" + RDF + "langString>")
                || term.equals("<" + RDF + "XMLLiteral>") || datatypes.contains(term);
    }

    private static boolean isDataFiller(String f, Set<String> datatypes, Map<String, List<String[]>> bySubject) {
        if (f.startsWith("\"")) {
            return true;
        }
        if (f.startsWith("<")) {
            return isDatatype(f, datatypes);
        }
        return bySubject.getOrDefault(f, List.of()).stream()
                .anyMatch(u -> u[1].equals(TYPE) && u[2].equals(DATATYPE));
    }
}
