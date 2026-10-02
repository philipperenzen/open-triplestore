package org.opentriplestore.reasoner;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.List;

import org.junit.jupiter.api.Test;

class DlTest {
    static final String EX = "http://example.org/";

    /** Compact triples: "s p o" per entry, with ex: rdf: rdfs: owl: xsd: prefixes and _:b nodes. */
    static String nt(String... triples) {
        StringBuilder b = new StringBuilder();
        for (String t : triples) {
            String[] parts = t.split(" ", 3);
            b.append(term(parts[0])).append(' ').append(term(parts[1])).append(' ').append(term(parts[2])).append(" .\n");
        }
        return b.toString();
    }

    static String term(String t) {
        if (t.startsWith("_:") || t.startsWith("\"")) return t;
        String[][] prefixes = {
            {"ex:", EX},
            {"rdf:", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"},
            {"rdfs:", "http://www.w3.org/2000/01/rdf-schema#"},
            {"owl:", "http://www.w3.org/2002/07/owl#"},
            {"xsd:", "http://www.w3.org/2001/XMLSchema#"},
        };
        for (String[] p : prefixes) {
            if (t.startsWith(p[0])) return "<" + p[1] + t.substring(p[0].length()) + ">";
        }
        throw new IllegalArgumentException(t);
    }

    static Dl.ReasonResult reason(String data) throws Exception {
        return Dl.reason(Dl.load(data), new Job(60_000), 2_000);
    }

    static boolean has(Dl.ReasonResult r, String s, String p, String o) {
        return r.inferred().contains(term(s) + " " + term(p) + " " + term(o) + " .");
    }

    @Test
    void existentialWitnessIsUsed() throws Exception {
        // A ⊑ ∃r.B, ∃r.B ⊑ C, a : A  ⊨  a : C  (no forward-chaining rule derives it)
        Dl.ReasonResult r = reason(nt(
                "ex:A rdfs:subClassOf _:x",
                "_:x rdf:type owl:Restriction", "_:x owl:onProperty ex:r", "_:x owl:someValuesFrom ex:B",
                "_:y rdf:type owl:Restriction", "_:y owl:onProperty ex:r", "_:y owl:someValuesFrom ex:B",
                "_:y rdfs:subClassOf ex:C",
                "ex:a rdf:type ex:A"));
        assertEquals(Boolean.TRUE, r.consistent());
        assertTrue(has(r, "ex:a", "rdf:type", "ex:C"), r.inferred());
        assertTrue(has(r, "ex:A", "rdfs:subClassOf", "ex:C"), r.inferred());
        assertFalse(r.inferred().contains("_:"), "named entities only");
        assertFalse(r.inferred().contains("owl#Thing"), "no owl:Thing noise");
    }

    @Test
    void caseSplitOverAUnion() throws Exception {
        Dl.ReasonResult r = reason(nt(
                "ex:A rdfs:subClassOf _:u", "_:u owl:unionOf _:l1",
                "_:l1 rdf:first ex:B", "_:l1 rdf:rest _:l2", "_:l2 rdf:first ex:C", "_:l2 rdf:rest rdf:nil",
                "ex:B rdfs:subClassOf ex:D", "ex:C rdfs:subClassOf ex:D",
                "ex:a rdf:type ex:A"));
        assertTrue(has(r, "ex:a", "rdf:type", "ex:D"), r.inferred());
    }

    @Test
    void nominalsGiveSameAs() throws Exception {
        Dl.ReasonResult r = reason(nt(
                "ex:C owl:equivalentClass _:o", "_:o owl:oneOf _:l", "_:l rdf:first ex:a", "_:l rdf:rest rdf:nil",
                "ex:b rdf:type ex:C", "ex:a rdf:type owl:NamedIndividual"));
        assertTrue(has(r, "ex:b", "owl:sameAs", "ex:a"), r.inferred());
    }

    @Test
    void propertyAssertionsThroughChainsAndInverses() throws Exception {
        Dl.ReasonResult r = reason(nt(
                "ex:hasParent owl:inverseOf ex:hasChild",
                "ex:hasGrandparent owl:propertyChainAxiom _:l", "_:l rdf:first ex:hasParent", "_:l rdf:rest _:m",
                "_:m rdf:first ex:hasParent", "_:m rdf:rest rdf:nil",
                "ex:c ex:hasParent ex:p", "ex:p ex:hasParent ex:g"));
        assertTrue(has(r, "ex:p", "ex:hasChild", "ex:c"), r.inferred());
        assertTrue(has(r, "ex:c", "ex:hasGrandparent", "ex:g"), r.inferred());
    }

    @Test
    void unsatisfiableClassesAreListed() throws Exception {
        Dl.ReasonResult r = reason(nt(
                "ex:A owl:disjointWith ex:B", "ex:C rdfs:subClassOf ex:A", "ex:C rdfs:subClassOf ex:B"));
        assertEquals(List.of(EX + "C"), r.unsatisfiable());
        assertTrue(has(r, "ex:C", "rdfs:subClassOf", "owl:Nothing"), r.inferred());
    }

    @Test
    void inconsistencyIsExplained() throws Exception {
        Dl.ReasonResult r = reason(nt(
                "ex:A owl:disjointWith ex:B", "ex:a rdf:type ex:A", "ex:a rdf:type ex:B",
                "ex:Unrelated rdfs:subClassOf ex:Other"));
        assertEquals(Boolean.FALSE, r.consistent());
        assertTrue(r.inconsistency().contains("3 axioms"), r.inconsistency());
        assertFalse(r.inconsistency().contains("Unrelated"), r.inconsistency());
        assertEquals("", r.inferred());
    }

    @Test
    void classAssertionOfARestrictionDefinedFirstIsRead() throws Exception {
        // OWL API 5.1.9 reported "a rdf:type _:r" as unparsed when _:r's
        // triples came first, although it read the assertion.
        Dl.ReasonResult r = reason(nt(
                "ex:p rdf:type owl:ObjectProperty",
                "_:r rdf:type owl:Restriction", "_:r owl:onProperty ex:p", "_:r owl:someValuesFrom owl:Thing",
                "ex:a rdf:type _:r", "ex:B owl:equivalentClass _:s",
                "_:s rdf:type owl:Restriction", "_:s owl:onProperty ex:p", "_:s owl:someValuesFrom owl:Thing"));
        assertTrue(has(r, "ex:a", "rdf:type", "ex:B"), r.inferred());
    }

    @Test
    void blankNodeInNothingIsInconsistent() throws Exception {
        Dl.ReasonResult r = reason(nt("_:o rdf:type owl:Ontology", "_:d rdf:type owl:Nothing"));
        assertEquals(Boolean.FALSE, r.consistent());
    }

    @Test
    void annotationsOfOntologyAnnotationsAreIgnored() throws Exception {
        Dl.ReasonResult r = reason(nt(
                "ex:onto rdf:type owl:Ontology", "ex:onto rdfs:label \"x\"",
                "_:a rdf:type owl:Annotation", "_:a owl:annotatedSource ex:onto",
                "_:a owl:annotatedProperty rdfs:label", "_:a owl:annotatedTarget \"x\"",
                "_:a ex:author \"Mike\"", "ex:author rdf:type owl:AnnotationProperty"));
        assertEquals(Boolean.TRUE, r.consistent());
        assertTrue(r.warnings().stream().anyMatch(w -> w.contains("of annotations ignored")), r.warnings().toString());
    }

    @Test
    void topEquivalentToBottomIsInconsistent() throws Exception {
        // HermiT crashed on this with OWL API 5.1.20 (empty union); 5.1.9 is pinned.
        Dl.ReasonResult r = reason(nt("owl:Thing rdf:type owl:Class", "owl:Thing owl:equivalentClass owl:Nothing"));
        assertEquals(Boolean.FALSE, r.consistent());
    }

    @Test
    void datatypeFacetConflictIsInconsistent() throws Exception {
        // age ≤ 10 (facet) and an asserted age of 20
        Dl.ReasonResult r = reason(nt(
                "ex:age rdf:type owl:DatatypeProperty", "ex:age rdf:type owl:FunctionalProperty",
                "ex:Child owl:equivalentClass _:r", "_:r rdf:type owl:Restriction", "_:r owl:onProperty ex:age",
                "_:r owl:allValuesFrom _:d", "_:d rdf:type rdfs:Datatype", "_:d owl:onDatatype xsd:integer",
                "_:d owl:withRestrictions _:l", "_:l rdf:first _:f", "_:l rdf:rest rdf:nil",
                "_:f xsd:maxInclusive \"10\"^^<http://www.w3.org/2001/XMLSchema#integer>",
                "ex:kim rdf:type ex:Child",
                "ex:kim ex:age \"20\"^^<http://www.w3.org/2001/XMLSchema#integer>"));
        assertEquals(Boolean.FALSE, r.consistent());
    }

    @Test
    void anonymousOntologyHeaderLoads() throws Exception {
        // The OWL API's Rio parser needs javax.xml.bind for an ontology header
        // that is a blank node, which Java 11+ no longer bundles.
        Dl.ReasonResult r = reason(nt(
                "_:N4aff0a7ea76e47629709a80a74e5fbc6 rdf:type owl:Ontology",
                "ex:A owl:disjointWith ex:B", "ex:a rdf:type ex:A"));
        assertEquals(Boolean.TRUE, r.consistent());
    }

    @Test
    void oneMemberListsReadAsTheirMember() throws Exception {
        // OWL 2 Mapping to RDF, Table 15: intersectionOf ( y ) is y, even nested.
        Dl.ReasonResult r = reason(nt(
                "ex:A rdfs:subClassOf _:i", "_:i rdf:type owl:Class", "_:i owl:intersectionOf _:l",
                "_:l rdf:first _:r", "_:l rdf:rest rdf:nil",
                "_:r rdf:type owl:Restriction", "_:r owl:onProperty ex:p", "_:r owl:someValuesFrom _:u",
                "_:u rdf:type owl:Class", "_:u owl:unionOf _:m", "_:m rdf:first ex:B", "_:m rdf:rest rdf:nil",
                "_:s rdf:type owl:Restriction", "_:s owl:onProperty ex:p", "_:s owl:someValuesFrom ex:B",
                "_:s rdfs:subClassOf ex:C",
                "ex:a rdf:type ex:A"));
        assertTrue(has(r, "ex:a", "rdf:type", "ex:C"), r.inferred());
    }

    @Test
    void oneMemberListsAsAxiomSubjectsAndOnNamedClasses() throws Exception {
        // Table 15: [ intersectionOf (A) ] disjointWith B is DisjointClasses(A B);
        // Table 18: C intersectionOf (A) is EquivalentClasses(C A).
        Dl.ReasonResult r = reason(nt(
                "_:x rdf:type owl:Class", "_:x owl:intersectionOf _:l", "_:l rdf:first ex:A", "_:l rdf:rest rdf:nil",
                "_:x owl:disjointWith ex:B",
                "ex:C rdf:type owl:Class", "ex:C owl:intersectionOf _:m", "_:m rdf:first ex:A", "_:m rdf:rest rdf:nil",
                "ex:a rdf:type ex:A"));
        assertTrue(has(r, "ex:a", "rdf:type", "ex:C"), r.inferred());
        assertEquals("false", Dl.check(Dl.load(nt(
                "_:x rdf:type owl:Class", "_:x owl:intersectionOf _:l", "_:l rdf:first ex:A", "_:l rdf:rest rdf:nil",
                "_:x owl:disjointWith ex:B", "ex:a rdf:type ex:A", "ex:a rdf:type ex:B")),
                "consistency", null, null, new Job(60_000), 0).result());
    }

    @Test
    void nonSimplePropertyInCardinalityIsNotInProfile() {
        Dl.NotInProfile e = assertThrows(Dl.NotInProfile.class, () -> Dl.load(nt(
                "ex:p rdf:type owl:ObjectProperty", "ex:p rdf:type owl:TransitiveProperty",
                "ex:A rdfs:subClassOf _:r", "_:r rdf:type owl:Restriction", "_:r owl:onProperty ex:p",
                "_:r owl:maxCardinality \"1\"^^<http://www.w3.org/2001/XMLSchema#nonNegativeInteger>")));
        assertTrue(e.violations.stream().anyMatch(v -> v.rule().contains("NonSimple")), e.violations.toString());
    }

    @Test
    void objectAndDataPropertyPunningIsNotInProfile() {
        Dl.NotInProfile e = assertThrows(Dl.NotInProfile.class, () -> Dl.load(nt(
                "ex:p rdf:type owl:ObjectProperty", "ex:p rdf:type owl:DatatypeProperty")));
        assertFalse(e.violations.isEmpty());
    }

    @Test
    void importsAreNeverFollowed() throws Exception {
        Dl.Input in = Dl.load(nt("_:o rdf:type owl:Ontology", "_:o owl:imports ex:elsewhere", "ex:a rdf:type ex:A"));
        assertTrue(in.warnings.stream().anyMatch(w -> w.contains("owl:imports")), in.warnings.toString());
    }

    @Test
    void undeclaredPredicatesAreTypedByUseLikeTheServer() throws Exception {
        // The OWL API alone reads ex:hasBrother between two IRIs as an annotation
        // and then finds it punned with the chain's object property.
        Dl.ReasonResult r = reason(nt(
                "ex:hasUncle owl:propertyChainAxiom _:l", "_:l rdf:first ex:hasParent", "_:l rdf:rest _:m",
                "_:m rdf:first ex:hasBrother", "_:m rdf:rest rdf:nil",
                "ex:hasBirthMother rdf:type owl:FunctionalProperty",
                "ex:John ex:hasParent ex:Mary", "ex:Mary ex:hasBrother ex:Bob",
                "ex:Ann ex:hasBirthMother ex:Mary", "ex:Ann ex:hasBirthMother ex:Maria",
                "ex:Ann ex:age \"3\"^^<http://www.w3.org/2001/XMLSchema#integer>"));
        assertTrue(has(r, "ex:John", "ex:hasUncle", "ex:Bob"), r.inferred());
        assertTrue(has(r, "ex:Mary", "owl:sameAs", "ex:Maria"), r.inferred());
        assertTrue(r.warnings().stream().anyMatch(w -> w.contains("typed by use")), r.warnings().toString());
    }

    @Test
    void undeclaredEntitiesAreTypedByUseWithAWarning() throws Exception {
        Dl.Input in = Dl.load(nt("ex:a ex:knows ex:b", "ex:a rdf:type ex:Person"));
        assertTrue(in.warnings.stream().anyMatch(w -> w.contains("typed by use")), in.warnings.toString());
    }

    @Test
    void checks() throws Exception {
        String premise = nt(
                "ex:A rdfs:subClassOf ex:B", "ex:B rdfs:subClassOf ex:C", "ex:a rdf:type ex:A",
                "ex:A owl:disjointWith ex:D", "ex:E rdfs:subClassOf ex:A", "ex:E rdfs:subClassOf ex:D");
        Job job = new Job(60_000);
        assertEquals("true", Dl.check(Dl.load(premise), "consistency", null, null, job, 0).result());
        assertEquals("true", Dl.check(Dl.load(premise), "entailment", nt("ex:a rdf:type ex:C"), null, job, 0).result());
        assertEquals("false", Dl.check(Dl.load(premise), "entailment", nt("ex:C rdfs:subClassOf ex:A"), null, job, 0).result());
        assertEquals("false", Dl.check(Dl.load(premise), "satisfiability", null, EX + "E", job, 0).result());
        assertEquals("true", Dl.check(Dl.load(premise), "satisfiability", null, EX + "A", job, 0).result());
        // Needs a case split; HermiT's own isEntailed said false before realisation.
        String split = nt(
                "ex:A rdfs:subClassOf _:u", "_:u rdf:type owl:Class", "_:u owl:unionOf _:l1",
                "_:l1 rdf:first ex:B", "_:l1 rdf:rest _:l2", "_:l2 rdf:first ex:C", "_:l2 rdf:rest rdf:nil",
                "ex:B rdfs:subClassOf ex:D", "ex:C rdfs:subClassOf ex:D", "ex:a rdf:type ex:A");
        assertEquals("true", Dl.check(Dl.load(split), "entailment", nt("ex:a rdf:type ex:D"), null, job, 0).result());
        assertEquals("false", Dl.check(Dl.load(split), "entailment", nt("ex:a rdf:type ex:B"), null, job, 0).result());
        // An anonymous individual in the conclusion is an existential.
        assertEquals("true", Dl.check(Dl.load(nt("ex:a ex:r ex:b", "ex:b rdf:type ex:B")), "entailment",
                nt("ex:a ex:r _:z", "_:z rdf:type ex:B"), null, job, 0).result());
        assertThrows(Dl.BadInput.class, () -> Dl.check(Dl.load(premise), "nonsense", null, null, job, 0));
    }

    @Test
    void anExpiredJobStops() {
        Job job = new Job(0);
        assertThrows(Job.Expired.class, job::check);
    }

    @Test
    void literalsAndIrisAreEscaped() {
        assertEquals("<http://e/a\\u0020b>", NTriples.iriTerm("http://e/a b"));
        assertEquals("\"a\\\"b\\nc\"", NTriples.quote("a\"b\nc"));
    }
}
