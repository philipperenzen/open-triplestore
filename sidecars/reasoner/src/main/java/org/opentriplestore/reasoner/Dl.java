package org.opentriplestore.reasoner;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.TreeSet;
import java.util.stream.Collectors;

import org.semanticweb.HermiT.Configuration;
import org.semanticweb.HermiT.Reasoner;
import org.semanticweb.owlapi.apibinding.OWLManager;
import org.semanticweb.owlapi.formats.NTriplesDocumentFormat;
import org.semanticweb.owlapi.io.StringDocumentSource;
import org.semanticweb.owlapi.model.AxiomType;
import org.semanticweb.owlapi.model.IRI;
import org.semanticweb.owlapi.model.MissingImportHandlingStrategy;
import org.semanticweb.owlapi.model.OWLAxiom;
import org.semanticweb.owlapi.model.OWLClass;
import org.semanticweb.owlapi.model.OWLDataFactory;
import org.semanticweb.owlapi.model.OWLDataProperty;
import org.semanticweb.owlapi.model.OWLDeclarationAxiom;
import org.semanticweb.owlapi.model.OWLEntity;
import org.semanticweb.owlapi.model.OWLLiteral;
import org.semanticweb.owlapi.model.OWLNamedIndividual;
import org.semanticweb.owlapi.model.OWLObjectProperty;
import org.semanticweb.owlapi.model.OWLObjectPropertyExpression;
import org.semanticweb.owlapi.model.OWLOntology;
import org.semanticweb.owlapi.model.OWLOntologyCreationException;
import org.semanticweb.owlapi.model.OWLOntologyLoaderConfiguration;
import org.semanticweb.owlapi.model.OWLOntologyManager;
import org.semanticweb.owlapi.model.parameters.Imports;
import org.semanticweb.owlapi.profiles.OWL2DLProfile;
import org.semanticweb.owlapi.profiles.OWLProfileViolation;
import org.semanticweb.owlapi.reasoner.InferenceType;
import org.semanticweb.owlapi.reasoner.OWLReasoner;
import org.semanticweb.owlapi.reasoner.ReasonerInterruptedException;
import org.semanticweb.owlapi.reasoner.UnsupportedEntailmentTypeException;
import org.semanticweb.owlapi.vocab.OWL2Datatype;

/**
 * The reasoning behind protocol v1: N-Triples in, a structured outcome out.
 *
 * <p>Every request parses its data with the OWL API (the reference reading of
 * the OWL 2 RDF mapping), never follows {@code owl:imports}, types undeclared
 * entities by use (as the server's own mapping does, with a warning), checks
 * the OWL 2 DL profile and only then reasons with HermiT. Inferences are
 * reported about named entities only.
 */
final class Dl {
    static final String RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
    static final String SUBCLASS = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
    static final String SUBPROPERTY = "http://www.w3.org/2000/01/rdf-schema#subPropertyOf";
    static final String OWL = "http://www.w3.org/2002/07/owl#";
    static final String IMPORTS = "<" + OWL + "imports>";
    static final int MAX_VIOLATIONS = 50;

    private Dl() {}

    // ── results ──────────────────────────────────────────────────────────

    record Violation(String rule, String detail) {}

    record ReasonResult(
            Boolean consistent,
            String inconsistency,
            List<String> unsatisfiable,
            String inferred,
            List<String> warnings) {}

    record CheckResult(String result, String detail, List<String> warnings) {}

    /** The input is not in OWL 2 DL (HTTP 422). */
    static final class NotInProfile extends Exception {
        final List<Violation> violations;
        final List<String> warnings;

        NotInProfile(List<Violation> violations, List<String> warnings) {
            super(violations.size() + " OWL 2 DL profile violation(s)");
            this.violations = violations;
            this.warnings = warnings;
        }
    }

    /** The request itself is unusable (HTTP 400). */
    static final class BadInput extends Exception {
        BadInput(String message) {
            super(message);
        }
    }

    // ── loading ──────────────────────────────────────────────────────────

    /** A parsed, profile-checked input and the manager that owns it. */
    static final class Input {
        final OWLOntologyManager manager;
        final OWLOntology ontology;
        final List<String> warnings;

        Input(OWLOntologyManager manager, OWLOntology ontology, List<String> warnings) {
            this.manager = manager;
            this.ontology = ontology;
            this.warnings = warnings;
        }
    }

    private static final Object MANAGER_LOCK = new Object();

    static OWLOntologyManager manager() {
        OWLOntologyManager m;
        // OWL API 5.1.9's OWLManager shares one injector that is not
        // thread-safe; concurrent requests must not create managers at once.
        synchronized (MANAGER_LOCK) {
            m = OWLManager.createOWLOntologyManager();
        }
        // Nothing is ever fetched: no document IRI is resolved over the network.
        m.getIRIMappers().clear();
        m.setOntologyLoaderConfiguration(loaderConfiguration());
        return m;
    }

    static OWLOntologyLoaderConfiguration loaderConfiguration() {
        return new OWLOntologyLoaderConfiguration()
                .setMissingImportHandlingStrategy(MissingImportHandlingStrategy.SILENT)
                .setFollowRedirects(false)
                .setStrict(false)
                .setReportStackTraces(false);
    }

    /**
     * Drop {@code owl:imports} triples: a run reasons over exactly the data it
     * was given. The number dropped goes to {@code dropped[0]}.
     */
    static String withoutImports(String ntriples, int[] dropped) {
        StringBuilder out = new StringBuilder(ntriples.length());
        int n = 0;
        for (String line : ntriples.split("\n", -1)) {
            String t = line.strip();
            int sp = t.indexOf(' ');
            if (sp > 0 && t.substring(sp).stripLeading().startsWith(IMPORTS)) {
                n++;
                continue;
            }
            out.append(line).append('\n');
        }
        dropped[0] = n;
        return out.toString();
    }

    /** Parse N-Triples into a fresh ontology (no profile check). */
    static OWLOntology parse(OWLOntologyManager m, String ntriples, List<String> warnings)
            throws BadInput {
        int[] dropped = {0};
        String data = Owl1Lists.rewrite(withoutImports(ntriples == null ? "" : ntriples, dropped));
        if (dropped[0] > 0) {
            warnings.add(dropped[0] + " owl:imports triple(s) ignored: imports are never followed");
        }
        List<String> typed = TypingByUse.declarations(NTriples.split(data, new StringBuilder()));
        if (!typed.isEmpty()) {
            data = data + "\n" + String.join("\n", typed) + "\n";
            warnings.add(typed.size() + " undeclared propert" + (typed.size() == 1 ? "y" : "ies")
                    + " typed by use (a literal object makes a data property, any other an object property)");
        }
        try {
            if (data.isBlank()) {
                return m.createOntology();
            }
            return m.loadOntologyFromOntologyDocument(
                    new StringDocumentSource(
                            data,
                            IRI.create("urn:ots:reasoner:document"),
                            new NTriplesDocumentFormat(),
                            "application/n-triples"),
                    loaderConfiguration());
        } catch (OWLOntologyCreationException | RuntimeException e) {
            throw new BadInput("the data is not readable N-Triples / OWL 2: " + parserCause(e));
        }
    }

    /**
     * Parse, type undeclared entities by use, and check the OWL 2 DL profile.
     *
     * @throws NotInProfile when the input is outside OWL 2 DL
     */
    static Input load(String ntriples) throws BadInput, NotInProfile {
        OWLOntologyManager m = manager();
        List<String> warnings = new ArrayList<>();
        OWLOntology o = parse(m, ntriples, warnings);
        List<Violation> violations = new ArrayList<>();

        unparsed(m, o, violations, warnings);

        OWLDataFactory df = m.getOWLDataFactory();
        List<OWLDeclarationAxiom> declarations =
                o.signature(Imports.EXCLUDED)
                        .filter(e -> !e.isBuiltIn() && !o.isDeclared(e))
                        .map(df::getOWLDeclarationAxiom)
                        .collect(Collectors.toList());
        if (!declarations.isEmpty()) {
            m.addAxioms(o, declarations);
            warnings.add(
                    declarations.size()
                            + " undeclared entit"
                            + (declarations.size() == 1 ? "y" : "ies")
                            + " typed by use (OWL 2 DL requires declarations)");
        }

        punning(o, violations);
        for (OWLProfileViolation v : new OWL2DLProfile().checkOntology(o).getViolations()) {
            if (violations.size() >= MAX_VIOLATIONS) {
                break;
            }
            violations.add(new Violation(v.getClass().getSimpleName(), firstLine(v.toString())));
        }
        if (!violations.isEmpty()) {
            throw new NotInProfile(violations, warnings);
        }
        return new Input(m, o, warnings);
    }

    /**
     * OWL 2 DL typing constraints (Structural Specification §5.8.1) the OWL
     * API's profile check leaves out: one IRI may not name two kinds of
     * property, nor both a class and a datatype.
     */
    private static void punning(OWLOntology o, List<Violation> out) {
        Set<IRI> iris = new TreeSet<>();
        o.signature(Imports.EXCLUDED).forEach(e -> iris.add(e.getIRI()));
        for (IRI iri : iris) {
            int kinds = (o.containsObjectPropertyInSignature(iri) ? 1 : 0)
                    + (o.containsDataPropertyInSignature(iri) ? 1 : 0)
                    + (o.containsAnnotationPropertyInSignature(iri) ? 1 : 0);
            if (kinds > 1) {
                out.add(new Violation("property-punning", iri + " is used as more than one kind of property"));
            }
            if (o.containsClassInSignature(iri) && o.containsDatatypeInSignature(iri)) {
                out.add(new Violation("class-datatype-punning", iri + " is used as a class and as a datatype"));
            }
        }
    }

    /** The parser's own explanation, from the OWL API's multi-parser report. */
    static String parserCause(Throwable e) {
        String msg = String.valueOf(e.getMessage());
        int i = msg.indexOf("1) ");
        String tail = i >= 0 ? msg.substring(i) : msg;
        tail = tail.replaceAll("\\s+", " ").trim();
        return tail.length() > 600 ? tail.substring(0, 600) + "…" : tail;
    }

    private static void unparsed(OWLOntologyManager m, OWLOntology o, List<Violation> out, List<String> warnings) {
        var format = m.getOntologyFormat(o);
        if (format == null) {
            return;
        }
        List<org.semanticweb.owlapi.io.RDFTriple> triples = format.getOntologyLoaderMetaData()
                .map(md -> md.getUnparsedTriples().collect(Collectors.toList()))
                .orElse(List.of());
        // OWL API 5.1.9 lists the triple of a class assertion whose class is a
        // blank-node expression as unparsed although it made the axiom: such a
        // triple counts as read while its subject has that many assertions.
        Map<String, Long> anonymousTypes = o.axioms(AxiomType.CLASS_ASSERTION)
                .filter(ax -> ax.getClassExpression().isAnonymous())
                .collect(Collectors.groupingBy(
                        ax -> ax.getIndividual().isNamed() ? ax.getIndividual().toStringID() : "_:",
                        Collectors.counting()));
        // Nor does it read annotations on annotations (owl:Annotation
        // reifications): they carry no meaning for reasoning, so they are
        // dropped with a warning.
        Set<String> reifications = new java.util.HashSet<>();
        for (var t : triples) {
            if (t.getPredicate().getIRI().toString().equals(OWL + "annotatedSource")) {
                reifications.add(t.getSubject().toString());
            }
        }
        OWLDataFactory df = m.getOWLDataFactory();
        Map<String, Long> seen = new HashMap<>();
        int ignored = 0;
        for (var t : triples) {
            if (reifications.contains(t.getSubject().toString())) {
                ignored++;
                continue;
            }
            if (t.getPredicate().getIRI().toString().equals(RDF_TYPE)) {
                String object = t.getObject().getIRI().toString();
                // It does not read rdf:type owl:Thing / owl:Nothing on a blank
                // node either: the first says nothing, the second has no model.
                if (object.equals(OWL + "Thing")) {
                    continue;
                }
                if (object.equals(OWL + "Nothing")) {
                    m.addAxiom(o, df.getOWLClassAssertionAxiom(df.getOWLNothing(), df.getOWLAnonymousIndividual()));
                    continue;
                }
                if (t.getObject().isAnonymous()) {
                    String key = t.getSubject().isAnonymous() ? "_:" : t.getSubject().getIRI().toString();
                    if (seen.merge(key, 1L, Long::sum) <= anonymousTypes.getOrDefault(key, 0L)) {
                        continue;
                    }
                }
            }
            if (out.size() < MAX_VIOLATIONS) {
                out.add(new Violation("unmapped-triple", t + " has no OWL 2 reading"));
            }
        }
        if (ignored > 0) {
            warnings.add(reifications.size() + " annotation(s) of annotations ignored: they do not affect reasoning");
        }
    }

    // ── reasoning ────────────────────────────────────────────────────────

    /** HermiT over {@code o}, registered with {@code job} so a timeout can interrupt it. */
    static OWLReasoner hermit(OWLOntology o, Job job) throws NotInProfile {
        Configuration c = new Configuration();
        c.throwInconsistentOntologyException = false;
        c.ignoreUnsupportedDatatypes = false;
        try {
            OWLReasoner r = new Reasoner.ReasonerFactory().createReasoner(o, c);
            job.attach(r);
            return r;
        } catch (RuntimeException e) {
            if (e.getClass().getSimpleName().contains("UnsupportedDatatype")) {
                throw new NotInProfile(
                        List.of(new Violation("unsupported-datatype", firstLine(e))), List.of());
            }
            throw e;
        }
    }

    /** Classify and realise; report the named-entity consequences. */
    static ReasonResult reason(Input in, Job job, int explainMaxAxioms) throws NotInProfile {
        OWLOntology o = in.ontology;
        List<String> warnings = new ArrayList<>(in.warnings);
        OWLReasoner r = hermit(o, job);
        try {
            job.check();
            if (!r.isConsistent()) {
                return new ReasonResult(
                        false,
                        inconsistency(in, job, explainMaxAxioms),
                        List.of(),
                        "",
                        warnings);
            }
            r.precomputeInferences(
                    InferenceType.CLASS_HIERARCHY,
                    InferenceType.OBJECT_PROPERTY_HIERARCHY,
                    InferenceType.DATA_PROPERTY_HIERARCHY,
                    InferenceType.CLASS_ASSERTIONS,
                    InferenceType.OBJECT_PROPERTY_ASSERTIONS,
                    InferenceType.SAME_INDIVIDUAL);
            job.check();

            NTriples nt = new NTriples();
            Set<String> unsatisfiable = new TreeSet<>();
            r.getUnsatisfiableClasses().entities()
                    .filter(c -> !c.isOWLNothing())
                    .forEach(c -> unsatisfiable.add(c.getIRI().toString()));

            List<OWLClass> classes = sorted(o.classesInSignature(Imports.INCLUDED)
                    .filter(c -> !c.isOWLThing() && !c.isOWLNothing()));
            for (OWLClass c : classes) {
                job.check();
                String ci = c.getIRI().toString();
                if (unsatisfiable.contains(ci)) {
                    nt.iri(ci, SUBCLASS, OWL + "Nothing");
                    continue;
                }
                r.getSuperClasses(c, false).entities()
                        .filter(d -> !d.isOWLThing())
                        .forEach(d -> nt.iri(ci, SUBCLASS, d.getIRI().toString()));
                r.getEquivalentClasses(c).entities()
                        .filter(d -> !d.equals(c) && !d.isOWLThing() && !d.isOWLNothing())
                        .forEach(d -> nt.iri(ci, OWL + "equivalentClass", d.getIRI().toString()));
            }

            List<OWLObjectProperty> objectProperties = sorted(o.objectPropertiesInSignature(Imports.INCLUDED)
                    .filter(p -> !p.isOWLTopObjectProperty() && !p.isOWLBottomObjectProperty()));
            for (OWLObjectProperty p : objectProperties) {
                job.check();
                String pi = p.getIRI().toString();
                r.getSuperObjectProperties(p, false).entities()
                        .filter(q -> q.isNamed() && !q.isOWLTopObjectProperty())
                        .forEach(q -> nt.iri(pi, SUBPROPERTY, q.asOWLObjectProperty().getIRI().toString()));
                r.getEquivalentObjectProperties(p).entities()
                        .filter(q -> q.isNamed() && !q.equals(p) && !q.isOWLBottomObjectProperty())
                        .forEach(q -> nt.iri(pi, OWL + "equivalentProperty", q.asOWLObjectProperty().getIRI().toString()));
            }
            List<OWLDataProperty> dataProperties = sorted(o.dataPropertiesInSignature(Imports.INCLUDED)
                    .filter(p -> !p.isOWLTopDataProperty() && !p.isOWLBottomDataProperty()));
            for (OWLDataProperty p : dataProperties) {
                job.check();
                String pi = p.getIRI().toString();
                r.getSuperDataProperties(p, false).entities()
                        .filter(q -> !q.isOWLTopDataProperty())
                        .forEach(q -> nt.iri(pi, SUBPROPERTY, q.getIRI().toString()));
                r.getEquivalentDataProperties(p).entities()
                        .filter(q -> !q.equals(p) && !q.isOWLBottomDataProperty())
                        .forEach(q -> nt.iri(pi, OWL + "equivalentProperty", q.getIRI().toString()));
            }

            for (OWLNamedIndividual i : sorted(o.individualsInSignature(Imports.INCLUDED))) {
                job.check();
                String ii = i.getIRI().toString();
                r.getTypes(i, false).entities()
                        .filter(c -> !c.isOWLThing())
                        .forEach(c -> nt.iri(ii, RDF_TYPE, c.getIRI().toString()));
                r.getSameIndividuals(i).entities()
                        .filter(j -> !j.equals(i))
                        .forEach(j -> nt.iri(ii, OWL + "sameAs", j.getIRI().toString()));
                for (OWLObjectPropertyExpression p : objectProperties) {
                    String pi = p.asOWLObjectProperty().getIRI().toString();
                    r.getObjectPropertyValues(i, p).entities()
                            .forEach(j -> nt.iri(ii, pi, j.getIRI().toString()));
                }
                for (OWLDataProperty p : dataProperties) {
                    String pi = p.getIRI().toString();
                    for (OWLLiteral l : r.getDataPropertyValues(i, p)) {
                        nt.literal(ii, pi, l);
                    }
                }
            }
            if (!dataProperties.isEmpty()) {
                warnings.add(
                        "data property values: HermiT reports the asserted values and those of "
                                + "sub-properties and equivalent individuals, not values entailed "
                                + "by owl:hasValue restrictions");
            }
            return new ReasonResult(true, null, new ArrayList<>(unsatisfiable), nt.toString(), warnings);
        } finally {
            job.detach(r);
            r.dispose();
        }
    }

    /** Answer a check task. */
    static CheckResult check(Input in, String task, String conclusion, String clazz, Job job, int explainMaxAxioms)
            throws BadInput, NotInProfile {
        List<String> warnings = new ArrayList<>(in.warnings);
        OWLReasoner r = hermit(in.ontology, job);
        try {
            job.check();
            boolean consistent = r.isConsistent();
            switch (task) {
                case "consistency":
                    return new CheckResult(
                            consistent ? "true" : "false",
                            consistent ? null : inconsistency(in, job, explainMaxAxioms),
                            warnings);
                case "satisfiability": {
                    if (clazz == null || clazz.isBlank()) {
                        throw new BadInput("a satisfiability check needs `class`");
                    }
                    if (!consistent) {
                        return new CheckResult("false", "the premise is inconsistent", warnings);
                    }
                    OWLClass c = in.manager.getOWLDataFactory().getOWLClass(IRI.create(clazz));
                    return new CheckResult(r.isSatisfiable(c) ? "true" : "false", null, warnings);
                }
                case "entailment": {
                    if (conclusion == null) {
                        throw new BadInput("an entailment check needs `conclusion` (N-Triples)");
                    }
                    Set<OWLAxiom> axioms = conclusionAxioms(in, conclusion, warnings);
                    if (!consistent) {
                        return new CheckResult("true", "the premise is inconsistent, so it entails everything", warnings);
                    }
                    if (axioms.isEmpty()) {
                        return new CheckResult("true", "the conclusion has no logical axioms", warnings);
                    }
                    try {
                        OWLDataFactory df = in.manager.getOWLDataFactory();
                        for (OWLAxiom ax : axioms) {
                            if (!Entailment.entailed(r, df, ax, job)) {
                                return new CheckResult("false", "not entailed: " + firstLine(ax), warnings);
                            }
                        }
                        return new CheckResult("true", null, warnings);
                    } catch (UnsupportedEntailmentTypeException e) {
                        return new CheckResult("unknown", "HermiT cannot check this entailment: " + firstLine(e), warnings);
                    }
                }
                default:
                    throw new BadInput("unknown task `" + task + "`; one of consistency, entailment, satisfiability");
            }
        } finally {
            job.detach(r);
            r.dispose();
        }
    }

    /**
     * The conclusion's logical axioms. It is parsed next to the premise's
     * declarations, so an entity keeps the kind the premise gives it.
     */
    static Set<OWLAxiom> conclusionAxioms(Input in, String conclusion, List<String> warnings) throws BadInput {
        StringBuilder text = new StringBuilder(conclusion);
        text.append('\n');
        in.ontology.axioms(AxiomType.DECLARATION).forEach(d -> {
            String kind = declarationClass(d.getEntity());
            if (kind != null) {
                text.append(NTriples.iriTerm(d.getEntity().getIRI().toString()))
                        .append(' ').append(NTriples.iriTerm(RDF_TYPE))
                        .append(' ').append(NTriples.iriTerm(kind)).append(" .\n");
            }
        });
        OWLOntologyManager m = manager();
        OWLOntology c = parse(m, text.toString(), warnings);
        return c.logicalAxioms().collect(Collectors.toCollection(LinkedHashSet::new));
    }

    private static String declarationClass(OWLEntity e) {
        if (e.isOWLClass()) return OWL + "Class";
        if (e.isOWLObjectProperty()) return OWL + "ObjectProperty";
        if (e.isOWLDataProperty()) return OWL + "DatatypeProperty";
        if (e.isOWLAnnotationProperty()) return OWL + "AnnotationProperty";
        if (e.isOWLNamedIndividual()) return OWL + "NamedIndividual";
        if (e.isOWLDatatype() && !OWL2Datatype.isBuiltIn(e.getIRI())) {
            return "http://www.w3.org/2000/01/rdf-schema#Datatype";
        }
        return null;
    }

    /** Why the input is inconsistent: a minimal inconsistent subset when affordable. */
    static String inconsistency(Input in, Job job, int maxAxioms) {
        String base = "HermiT found the ontology inconsistent";
        List<OWLAxiom> logical = in.ontology.logicalAxioms().collect(Collectors.toList());
        if (maxAxioms <= 0 || logical.size() > maxAxioms) {
            return base;
        }
        try {
            List<OWLAxiom> core = Explain.minimalInconsistentSubset(in, logical, job);
            if (core == null || core.isEmpty()) {
                return base;
            }
            List<String> shown = core.stream().map(a -> firstLine(a.toString())).sorted().limit(20).collect(Collectors.toList());
            return base + "; a minimal inconsistent subset (" + core.size() + " axiom"
                    + (core.size() == 1 ? "" : "s") + "): " + String.join("; ", shown)
                    + (core.size() > shown.size() ? "; …" : "");
        } catch (Job.Expired | ReasonerInterruptedException e) {
            return base;
        }
    }

    private static <T extends OWLEntity> List<T> sorted(java.util.stream.Stream<T> s) {
        return s.sorted().collect(Collectors.toList());
    }

    static String firstLine(Object o) {
        String s = o instanceof Throwable t ? String.valueOf(t.getMessage()) : String.valueOf(o);
        int nl = s.indexOf('\n');
        s = nl >= 0 ? s.substring(0, nl) : s;
        return s.length() > 500 ? s.substring(0, 500) + "…" : s;
    }
}
