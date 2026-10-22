package org.opentriplestore.reasoner;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.lang.reflect.Proxy;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

import org.junit.jupiter.api.Test;
import org.semanticweb.owlapi.reasoner.OWLReasoner;
import org.semanticweb.owlapi.reasoner.ReasonerInterruptedException;

class RestartTest {
    /** A stand-in for HermiT whose consistency test answers at once or runs until interrupted. */
    static final class Fake {
        final boolean learning;
        final boolean answers;
        final CountDownLatch interrupted = new CountDownLatch(1);
        volatile boolean ran;
        volatile boolean disposed;
        final OWLReasoner reasoner;

        Fake(boolean learning, boolean answers) {
            this.learning = learning;
            this.answers = answers;
            this.reasoner = (OWLReasoner) Proxy.newProxyInstance(
                    OWLReasoner.class.getClassLoader(), new Class<?>[] {OWLReasoner.class}, (proxy, m, args) ->
                            switch (m.getName()) {
                                case "isConsistent" -> {
                                    ran = true;
                                    if (!answers && !interrupted.await(30, TimeUnit.SECONDS)) {
                                        throw new AssertionError("never interrupted");
                                    }
                                    if (!answers) {
                                        throw new ReasonerInterruptedException();
                                    }
                                    yield true;
                                }
                                case "interrupt" -> {
                                    interrupted.countDown();
                                    yield null;
                                }
                                case "dispose" -> {
                                    disposed = true;
                                    yield null;
                                }
                                case "hashCode" -> System.identityHashCode(proxy);
                                case "equals" -> proxy == args[0];
                                case "toString" -> "fake(learning=" + learning + ")";
                                default -> throw new UnsupportedOperationException(m.getName());
                            });
        }
    }

    /** Fakes in creation order; {@code answers[i]} says whether the i-th one answers. */
    static Restart.Hermits hermits(Job job, List<Fake> made, boolean... answers) {
        return learning -> {
            Fake f = new Fake(learning, made.size() < answers.length && answers[made.size()]);
            made.add(f);
            job.attach(f.reasoner);
            return f.reasoner;
        };
    }

    @Test
    void anAnswerWithinTheFirstTryKeepsHermitsDefaults() throws Exception {
        Job job = new Job(60_000);
        List<Fake> made = new ArrayList<>();
        OWLReasoner r = Restart.consistencyDecided(hermits(job, made, true), job, 200);
        assertEquals(1, made.size());
        assertSame(made.get(0).reasoner, r);
        assertTrue(made.get(0).learning);
        Thread.sleep(400);
        assertEquals(1, made.get(0).interrupted.getCount(), "no interrupt after the test answered");
        assertFalse(made.get(0).disposed);
    }

    @Test
    void anUndecidedFirstTryIsRetriedWithoutDisjunctionLearning() throws Exception {
        Job job = new Job(60_000);
        List<Fake> made = new ArrayList<>();
        long t0 = System.nanoTime();
        OWLReasoner r = Restart.consistencyDecided(hermits(job, made, false, true), job, 200);
        long ms = (System.nanoTime() - t0) / 1_000_000;
        assertEquals(2, made.size());
        assertTrue(made.get(0).learning);
        assertTrue(made.get(0).disposed, "the first try's reasoner is disposed");
        assertFalse(made.get(1).learning);
        assertSame(made.get(1).reasoner, r);
        assertTrue(ms >= 200 && ms < 5_000, ms + " ms");
    }

    @Test
    void twoUndecidedTriesLeaveTheRestToHermitsDefaults() throws Exception {
        Job job = new Job(60_000);
        List<Fake> made = new ArrayList<>();
        OWLReasoner r = Restart.consistencyDecided(hermits(job, made, false, false), job, 100);
        assertEquals(3, made.size());
        assertTrue(made.get(0).disposed);
        assertTrue(made.get(1).disposed);
        assertTrue(made.get(2).learning);
        assertFalse(made.get(2).ran, "the last reasoner is left to the caller, under the job's time limit");
        assertSame(made.get(2).reasoner, r);
    }

    @Test
    void theRequestsOwnTimeLimitStillStopsTheFirstTry() throws Exception {
        Job job = new Job(60_000);
        List<Fake> made = new ArrayList<>();
        Thread canceller = new Thread(() -> {
            try {
                Thread.sleep(100);
            } catch (InterruptedException e) {
                return;
            }
            job.cancel();
        });
        canceller.start();
        assertThrows(Job.Expired.class, () -> Restart.consistencyDecided(hermits(job, made), job, 10_000));
        canceller.join();
        assertEquals(1, made.size(), "no retry after the request ended");
        assertTrue(made.get(0).disposed);
    }

    @Test
    void aShortTimeLimitShortensTheFirstTry() throws Exception {
        Job job = new Job(800);
        List<Fake> made = new ArrayList<>();
        long t0 = System.nanoTime();
        Restart.consistencyDecided(hermits(job, made, false, false), job, 10_000);
        long ms = (System.nanoTime() - t0) / 1_000_000;
        assertEquals(3, made.size());
        assertTrue(ms < 700, "two tries of at most a quarter of 800 ms each took " + ms + " ms");
    }
}
