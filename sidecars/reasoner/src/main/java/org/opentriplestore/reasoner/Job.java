package org.opentriplestore.reasoner;

import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;

import org.semanticweb.owlapi.reasoner.OWLReasoner;

/**
 * One request's deadline. The reasoners it starts register here, so the
 * request thread can interrupt them when the time is up.
 */
final class Job {
    /** The deadline passed or the job was cancelled. */
    static final class Expired extends RuntimeException {
        Expired() {
            super("the time limit was reached", null, false, false);
        }
    }

    private final long deadlineNanos;
    private final Set<OWLReasoner> running = ConcurrentHashMap.newKeySet();
    private volatile boolean cancelled;

    Job(long timeoutMillis) {
        this.deadlineNanos = System.nanoTime() + timeoutMillis * 1_000_000L;
    }

    long remainingMillis() {
        return Math.max(0, (deadlineNanos - System.nanoTime()) / 1_000_000L);
    }

    /** Throw {@link Expired} once the deadline passed or the job was cancelled. */
    void check() {
        if (cancelled || System.nanoTime() - deadlineNanos > 0 || Thread.currentThread().isInterrupted()) {
            throw new Expired();
        }
    }

    void attach(OWLReasoner r) {
        running.add(r);
        if (cancelled) {
            r.interrupt();
        }
    }

    void detach(OWLReasoner r) {
        running.remove(r);
    }

    /** Stop: interrupt every reasoner this job is running. */
    void cancel() {
        cancelled = true;
        for (OWLReasoner r : running) {
            try {
                r.interrupt();
            } catch (RuntimeException ignored) {
                // best effort
            }
        }
    }
}
