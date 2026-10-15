package org.opentriplestore.reasoner;

import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.ScheduledThreadPoolExecutor;
import java.util.concurrent.TimeUnit;

import org.semanticweb.owlapi.reasoner.OWLReasoner;
import org.semanticweb.owlapi.reasoner.ReasonerInterruptedException;

/**
 * HermiT's consistency test, restarted once without disjunction learning.
 *
 * <p>Disjunction learning makes HermiT try first the disjuncts of a
 * disjunction that led to fewer clashes so far. It only changes the order of
 * the search, so HermiT is sound and complete with it and without it, but
 * each setting has inputs on which it runs for a very long time while the
 * other answers at once. The premises of the W3C OWL 2 test cases
 * WebOnt-description-logic-208 and -209 (k_poly, from the DL98 systems
 * comparison) were not decided in 15 minutes with it and are decided in under
 * a second without it (#506); the classification of the premise of
 * WebOnt-description-logic-905 takes half a second with it and did not finish
 * in 20 seconds without it.
 *
 * <p>So HermiT first runs with its defaults. When it has not decided
 * consistency after a first try ({@link #FIRST_TRY_MS}, at most a quarter of
 * the time left), a second reasoner tries as long without disjunction
 * learning, and when that has not decided it either, a third one with the
 * defaults gets the rest of the time limit. The reasoner that decided
 * consistency does the rest of the request's work, so a request whose
 * consistency test answers within the first try runs exactly as before.
 */
final class Restart {
    /** How long the first try with HermiT's defaults may take. */
    static final long FIRST_TRY_MS = 5_000;

    private static final ScheduledThreadPoolExecutor TIMER = timer();

    private static ScheduledThreadPoolExecutor timer() {
        ScheduledThreadPoolExecutor t = new ScheduledThreadPoolExecutor(1, r -> {
            Thread th = new Thread(r, "hermit-first-try");
            th.setDaemon(true);
            return th;
        });
        // A cancelled interrupter leaves the queue at once, and with it the
        // reasoner it refers to.
        t.setRemoveOnCancelPolicy(true);
        return t;
    }

    private Restart() {}

    /** Creates a HermiT reasoner, registered with the request's job. */
    interface Hermits {
        OWLReasoner create(boolean disjunctionLearning) throws Dl.NotInProfile;
    }

    /**
     * A reasoner that has decided consistency within a try, or else a fresh
     * one with HermiT's defaults that has the rest of the time limit.
     *
     * @throws Job.Expired when the request's own time limit passes or it is cancelled
     */
    static OWLReasoner consistencyDecided(Hermits hermits, Job job, long firstTryMs) throws Dl.NotInProfile {
        long tryMs = Math.min(firstTryMs, job.remainingMillis() / 4);
        if (tryMs > 0) {
            for (boolean learning : new boolean[] {true, false}) {
                OWLReasoner r = hermits.create(learning);
                boolean decided = false;
                try {
                    decided = decidedWithin(r, tryMs, job);
                } finally {
                    if (!decided) {
                        job.detach(r);
                        r.dispose();
                    }
                }
                if (decided) {
                    return r;
                }
            }
        }
        return hermits.create(true);
    }

    /**
     * Whether {@code r} decided consistency within {@code ms}. HermiT clears a
     * pending interrupt when a task starts, so the interrupt is repeated until
     * the test has stopped; none is sent after it has.
     */
    static boolean decidedWithin(OWLReasoner r, long ms, Job job) {
        Object lock = new Object();
        boolean[] stopped = {false};
        ScheduledFuture<?> interrupter = TIMER.scheduleAtFixedRate(() -> {
            synchronized (lock) {
                if (!stopped[0]) {
                    r.interrupt();
                }
            }
        }, ms, 20, TimeUnit.MILLISECONDS);
        try {
            r.isConsistent();
            return true;
        } catch (ReasonerInterruptedException e) {
            job.check(); // the request's own time limit or cancellation: Job.Expired
            return false;
        } finally {
            synchronized (lock) {
                stopped[0] = true;
            }
            interrupter.cancel(false);
        }
    }
}
