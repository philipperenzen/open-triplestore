package org.opentriplestore.reasoner;

import java.util.ArrayList;
import java.util.List;
import java.util.Set;
import java.util.stream.Collectors;

import org.semanticweb.HermiT.Configuration;
import org.semanticweb.HermiT.Reasoner;
import org.semanticweb.owlapi.model.OWLAxiom;
import org.semanticweb.owlapi.model.OWLOntology;
import org.semanticweb.owlapi.model.OWLOntologyCreationException;
import org.semanticweb.owlapi.model.OWLOntologyManager;
import org.semanticweb.owlapi.reasoner.OWLReasoner;

/**
 * A minimal inconsistent subset of the logical axioms, by QuickXplain
 * (Junker, AAAI 2004): O(k log n) consistency tests for a core of k axioms.
 * Declarations are kept as background, so every subset keeps its typing.
 * Bounded by the job's deadline and {@link #MAX_TESTS}.
 */
final class Explain {
    static final int MAX_TESTS = 400;

    private final OWLOntologyManager manager;
    private final Set<OWLAxiom> background;
    private final Job job;
    private int tests;

    private Explain(Dl.Input in, Job job) {
        this.manager = Dl.manager();
        this.background = in.ontology.axioms()
                .filter(a -> !a.isLogicalAxiom())
                .collect(Collectors.toSet());
        this.job = job;
    }

    /** A minimal inconsistent subset of {@code axioms}, or null when out of budget. */
    static List<OWLAxiom> minimalInconsistentSubset(Dl.Input in, List<OWLAxiom> axioms, Job job) {
        Explain e = new Explain(in, job);
        try {
            if (e.consistent(axioms)) {
                return null;
            }
            return e.qx(new ArrayList<>(), false, new ArrayList<>(axioms));
        } catch (Budget b) {
            return null;
        }
    }

    private static final class Budget extends RuntimeException {
        Budget() {
            super(null, null, false, false);
        }
    }

    private List<OWLAxiom> qx(List<OWLAxiom> b, boolean delta, List<OWLAxiom> c) {
        if (delta && !consistent(b)) {
            return new ArrayList<>();
        }
        if (c.size() == 1) {
            return new ArrayList<>(c);
        }
        int k = c.size() / 2;
        List<OWLAxiom> c1 = new ArrayList<>(c.subList(0, k));
        List<OWLAxiom> c2 = new ArrayList<>(c.subList(k, c.size()));
        List<OWLAxiom> b1 = new ArrayList<>(b);
        b1.addAll(c1);
        List<OWLAxiom> d2 = qx(b1, !c1.isEmpty(), c2);
        List<OWLAxiom> b2 = new ArrayList<>(b);
        b2.addAll(d2);
        List<OWLAxiom> d1 = qx(b2, !d2.isEmpty(), c1);
        d1.addAll(d2);
        return d1;
    }

    private boolean consistent(List<OWLAxiom> axioms) {
        job.check();
        if (++tests > MAX_TESTS) {
            throw new Budget();
        }
        OWLOntology o;
        try {
            o = manager.createOntology();
        } catch (OWLOntologyCreationException e) {
            throw new Budget();
        }
        try {
            manager.addAxioms(o, background);
            manager.addAxioms(o, axioms);
            Configuration c = new Configuration();
            c.throwInconsistentOntologyException = false;
            OWLReasoner r = new Reasoner.ReasonerFactory().createReasoner(o, c);
            job.attach(r);
            try {
                return r.isConsistent();
            } finally {
                job.detach(r);
                r.dispose();
            }
        } finally {
            manager.removeOntology(o);
        }
    }
}
