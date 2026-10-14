package org.opentriplestore.reasoner;

import java.util.List;
import java.util.stream.Collectors;

import org.semanticweb.owlapi.model.*;
import org.semanticweb.owlapi.reasoner.InferenceType;
import org.semanticweb.owlapi.reasoner.OWLReasoner;

/**
 * Entailment by reduction to class satisfiability (OWL 2 Direct Semantics):
 * O ⊨ α iff a class expression that says "α fails here" is unsatisfiable in
 * O. Each answer is one HermiT tableau test, so a {@code false} is as sound
 * as HermiT's consistency check. HermiT's own {@code isEntailed} answers a
 * class assertion {@code false} until the ABox has been realised (seen with
 * HermiT 1.4.5.519), so it is used only for axiom types with no reduction
 * here, and only after every inference has been precomputed.
 */
final class Entailment {
    private Entailment() {}

    static boolean entailed(OWLReasoner r, OWLDataFactory df, OWLAxiom ax, Job job) {
        job.check();
        Boolean b = byReduction(r, df, ax, job);
        if (b != null) {
            return b;
        }
        r.precomputeInferences(
                InferenceType.CLASS_HIERARCHY,
                InferenceType.OBJECT_PROPERTY_HIERARCHY,
                InferenceType.DATA_PROPERTY_HIERARCHY,
                InferenceType.CLASS_ASSERTIONS,
                InferenceType.OBJECT_PROPERTY_ASSERTIONS,
                InferenceType.DATA_PROPERTY_ASSERTIONS,
                InferenceType.SAME_INDIVIDUAL);
        return r.isEntailed(ax);
    }

    /** Is {@code ce} empty in every model? */
    private static boolean empty(OWLReasoner r, OWLClassExpression ce, Job job) {
        job.check();
        return !r.isSatisfiable(ce);
    }

    private static OWLClassExpression and(OWLDataFactory df, OWLClassExpression a, OWLClassExpression b) {
        return df.getOWLObjectIntersectionOf(a, b);
    }

    private static OWLClassExpression not(OWLDataFactory df, OWLClassExpression a) {
        return df.getOWLObjectComplementOf(a);
    }

    private static Boolean byReduction(OWLReasoner r, OWLDataFactory df, OWLAxiom ax, Job job) {
        if (ax.anonymousIndividuals().findAny().isPresent()) {
            return null; // existentials: HermiT's own checker rolls them up
        }
        OWLClassExpression top = df.getOWLThing();
        if (ax instanceof OWLSubClassOfAxiom a) {
            return empty(r, and(df, a.getSubClass(), not(df, a.getSuperClass())), job);
        }
        if (ax instanceof OWLEquivalentClassesAxiom a) {
            List<OWLClassExpression> cs = a.classExpressions().collect(Collectors.toList());
            for (OWLClassExpression x : cs) {
                for (OWLClassExpression y : cs) {
                    if (x != y && !empty(r, and(df, x, not(df, y)), job)) {
                        return false;
                    }
                }
            }
            return true;
        }
        if (ax instanceof OWLDisjointClassesAxiom a) {
            List<OWLClassExpression> cs = a.classExpressions().collect(Collectors.toList());
            for (int i = 0; i < cs.size(); i++) {
                for (int j = i + 1; j < cs.size(); j++) {
                    if (!empty(r, and(df, cs.get(i), cs.get(j)), job)) {
                        return false;
                    }
                }
            }
            return true;
        }
        if (ax instanceof OWLClassAssertionAxiom a) {
            return empty(r, and(df, df.getOWLObjectOneOf(a.getIndividual()), not(df, a.getClassExpression())), job);
        }
        if (ax instanceof OWLObjectPropertyAssertionAxiom a) {
            return empty(r, and(df, df.getOWLObjectOneOf(a.getSubject()),
                    df.getOWLObjectAllValuesFrom(a.getProperty(), not(df, df.getOWLObjectOneOf(a.getObject())))), job);
        }
        if (ax instanceof OWLNegativeObjectPropertyAssertionAxiom a) {
            return empty(r, and(df, df.getOWLObjectOneOf(a.getSubject()),
                    df.getOWLObjectHasValue(a.getProperty(), a.getObject())), job);
        }
        if (ax instanceof OWLDataPropertyAssertionAxiom a) {
            return empty(r, and(df, df.getOWLObjectOneOf(a.getSubject()),
                    df.getOWLDataAllValuesFrom(a.getProperty(),
                            df.getOWLDataComplementOf(df.getOWLDataOneOf(a.getObject())))), job);
        }
        if (ax instanceof OWLNegativeDataPropertyAssertionAxiom a) {
            return empty(r, and(df, df.getOWLObjectOneOf(a.getSubject()),
                    df.getOWLDataHasValue(a.getProperty(), a.getObject())), job);
        }
        if (ax instanceof OWLSameIndividualAxiom a) {
            List<OWLIndividual> is = a.individuals().collect(Collectors.toList());
            for (int i = 1; i < is.size(); i++) {
                if (!empty(r, and(df, df.getOWLObjectOneOf(is.get(0)), not(df, df.getOWLObjectOneOf(is.get(i)))), job)) {
                    return false;
                }
            }
            return true;
        }
        if (ax instanceof OWLDifferentIndividualsAxiom a) {
            List<OWLIndividual> is = a.individuals().collect(Collectors.toList());
            for (int i = 0; i < is.size(); i++) {
                for (int j = i + 1; j < is.size(); j++) {
                    if (!empty(r, and(df, df.getOWLObjectOneOf(is.get(i)), df.getOWLObjectOneOf(is.get(j))), job)) {
                        return false;
                    }
                }
            }
            return true;
        }
        if (ax instanceof OWLObjectPropertyDomainAxiom a) {
            return empty(r, and(df, df.getOWLObjectSomeValuesFrom(a.getProperty(), top), not(df, a.getDomain())), job);
        }
        if (ax instanceof OWLObjectPropertyRangeAxiom a) {
            return empty(r, and(df, df.getOWLObjectSomeValuesFrom(a.getProperty().getInverseProperty(), top),
                    not(df, a.getRange())), job);
        }
        if (ax instanceof OWLDataPropertyDomainAxiom a) {
            return empty(r, and(df, df.getOWLDataSomeValuesFrom(a.getProperty(), df.getTopDatatype()),
                    not(df, a.getDomain())), job);
        }
        if (ax instanceof OWLDataPropertyRangeAxiom a) {
            return empty(r, df.getOWLDataSomeValuesFrom(a.getProperty(), df.getOWLDataComplementOf(a.getRange())), job);
        }
        if (ax instanceof OWLFunctionalObjectPropertyAxiom a) {
            return empty(r, df.getOWLObjectMinCardinality(2, a.getProperty(), top), job);
        }
        if (ax instanceof OWLInverseFunctionalObjectPropertyAxiom a) {
            return empty(r, df.getOWLObjectMinCardinality(2, a.getProperty().getInverseProperty(), top), job);
        }
        if (ax instanceof OWLFunctionalDataPropertyAxiom a) {
            return empty(r, df.getOWLDataMinCardinality(2, a.getProperty(), df.getTopDatatype()), job);
        }
        if (ax instanceof OWLIrreflexiveObjectPropertyAxiom a) {
            return empty(r, df.getOWLObjectHasSelf(a.getProperty()), job);
        }
        if (ax instanceof OWLReflexiveObjectPropertyAxiom a) {
            return empty(r, not(df, df.getOWLObjectHasSelf(a.getProperty())), job);
        }
        return null;
    }
}
