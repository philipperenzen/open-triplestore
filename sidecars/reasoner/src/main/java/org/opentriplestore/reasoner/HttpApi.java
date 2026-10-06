package org.opentriplestore.reasoner;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.Callable;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.Semaphore;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpServer;

import org.semanticweb.owlapi.reasoner.ReasonerInterruptedException;

/**
 * Protocol v1 over HTTP:
 *
 * <ul>
 *   <li>{@code POST /v1/reason} — {@code {data, timeout_ms}} → the reason outcome;
 *   <li>{@code POST /v1/check} — {@code {task, data, conclusion?, class?, timeout_ms}} →
 *       {@code {result: "true"|"false"|"unknown", ...}};
 *   <li>{@code GET /health}, {@code GET /version} (no token needed).
 * </ul>
 *
 * Statuses: 400 unreadable request, 401 bad token, 413 body too large, 422
 * not in OWL 2 DL ({@code in_profile: false, violations}), 503 busy, 504 past
 * the time limit ({@code result: "unknown"}).
 */
final class HttpApi {
    static final int PROTOCOL = 1;
    private static final ObjectMapper JSON = new ObjectMapper();

    private final Config cfg;
    private final Map<String, String> backend;
    private final Semaphore slots;
    private final ExecutorService workers = Executors.newCachedThreadPool(r -> {
        Thread t = new Thread(r, "reasoner");
        t.setDaemon(true);
        return t;
    });
    private HttpServer server;

    HttpApi(Config cfg) {
        this.cfg = cfg;
        this.backend = Map.of("name", "hermit", "version", Versions.hermit());
        this.slots = new Semaphore(cfg.concurrency(), true);
    }

    int start() throws IOException {
        server = HttpServer.create(new InetSocketAddress(cfg.bind(), cfg.port()), 64);
        server.setExecutor(Executors.newFixedThreadPool(cfg.concurrency() + 4));
        server.createContext("/health", ex -> get(ex, () -> Map.of("status", "ok")));
        server.createContext("/version", ex -> get(ex, () -> {
            Map<String, Object> v = new LinkedHashMap<>();
            v.put("protocol", PROTOCOL);
            v.put("sidecar", Versions.sidecar());
            v.put("backend", backend);
            v.put("owlapi", Versions.owlapi());
            return v;
        }));
        server.createContext("/v1/reason", ex -> post(ex, this::reason));
        server.createContext("/v1/check", ex -> post(ex, this::check));
        server.start();
        return server.getAddress().getPort();
    }

    void stop() {
        if (server != null) {
            server.stop(0);
        }
        workers.shutdownNow();
    }

    // ── handlers ─────────────────────────────────────────────────────────

    private interface Body {
        Reply handle(JsonNode body) throws Exception;
    }

    record Reply(int status, Object body) {}

    private Reply reason(JsonNode body) throws Exception {
        String data = text(body, "data", true);
        long timeout = timeout(body);
        return run(timeout, job -> {
            Dl.Input in = Dl.load(data);
            job.check();
            Dl.ReasonResult r = Dl.reason(in, job, cfg.explainMaxAxioms());
            Map<String, Object> out = new LinkedHashMap<>();
            out.put("consistent", r.consistent());
            out.put("inconsistency", r.inconsistency());
            out.put("in_profile", true);
            out.put("violations", List.of());
            out.put("unsatisfiable", r.unsatisfiable());
            out.put("inferred", r.inferred());
            out.put("complete", true);
            out.put("backend", backend);
            out.put("warnings", r.warnings());
            return new Reply(200, out);
        });
    }

    private Reply check(JsonNode body) throws Exception {
        String task = text(body, "task", true).trim().toLowerCase();
        String data = text(body, "data", true);
        String conclusion = text(body, "conclusion", false);
        String clazz = text(body, "class", false);
        long timeout = timeout(body);
        return run(timeout, job -> {
            Dl.Input in = Dl.load(data);
            job.check();
            Dl.CheckResult r = Dl.check(in, task, conclusion, clazz, job, cfg.explainMaxAxioms());
            Map<String, Object> out = new LinkedHashMap<>();
            out.put("result", r.result());
            if (r.detail() != null) {
                out.put("detail", r.detail());
            }
            out.put("in_profile", true);
            out.put("violations", List.of());
            out.put("backend", backend);
            out.put("warnings", r.warnings());
            return new Reply(200, out);
        });
    }

    private interface Work {
        Reply run(Job job) throws Exception;
    }

    /** Run {@code work} in a reasoning slot under the request's time limit. */
    private Reply run(long timeoutMs, Work work) throws Exception {
        Job job = new Job(timeoutMs);
        if (!slots.tryAcquire(Math.min(timeoutMs, cfg.queueWaitMs()), TimeUnit.MILLISECONDS)) {
            return new Reply(503, Map.of("error", "the reasoner is busy; try again later"));
        }
        Callable<Reply> task = () -> {
            try {
                return work.run(job);
            } finally {
                // Freed only when the reasoning has really stopped.
                slots.release();
            }
        };
        Future<Reply> f;
        try {
            f = workers.submit(task);
        } catch (RuntimeException e) {
            slots.release();
            throw e;
        }
        try {
            return f.get(Math.max(1, job.remainingMillis()), TimeUnit.MILLISECONDS);
        } catch (TimeoutException e) {
            job.cancel();
            f.cancel(true);
            return timedOut(timeoutMs);
        } catch (ExecutionException e) {
            Throwable c = e.getCause();
            if (c instanceof Job.Expired || c instanceof ReasonerInterruptedException) {
                return timedOut(timeoutMs);
            }
            if (c instanceof Exception ex) {
                throw ex;
            }
            throw new RuntimeException(c);
        }
    }

    private Reply timedOut(long timeoutMs) {
        return new Reply(504, Map.of(
                "result", "unknown",
                "error", "no answer within the time limit of " + timeoutMs + " ms",
                "backend", backend));
    }

    // ── plumbing ─────────────────────────────────────────────────────────

    private interface Get {
        Object body();
    }

    private void get(HttpExchange ex, Get g) throws IOException {
        try (ex) {
            if (!ex.getRequestMethod().equals("GET")) {
                send(ex, 405, Map.of("error", "use GET"));
                return;
            }
            send(ex, 200, g.body());
        }
    }

    private void post(HttpExchange ex, Body handler) throws IOException {
        long started = System.nanoTime();
        int status = 500;
        try (ex) {
            if (!ex.getRequestMethod().equals("POST")) {
                send(ex, status = 405, Map.of("error", "use POST"));
                return;
            }
            if (!authorised(ex)) {
                send(ex, status = 401, Map.of("error", "missing or wrong bearer token"));
                return;
            }
            byte[] raw = readCapped(ex.getRequestBody(), cfg.maxBodyBytes());
            if (raw == null) {
                send(ex, status = 413, Map.of("error", "the request body is larger than " + cfg.maxBodyBytes() + " bytes"));
                return;
            }
            JsonNode body;
            try {
                body = JSON.readTree(raw);
            } catch (IOException e) {
                send(ex, status = 400, Map.of("error", "the body is not JSON"));
                return;
            }
            Reply reply;
            try {
                reply = handler.handle(body);
            } catch (Dl.BadInput e) {
                reply = new Reply(400, Map.of("error", e.getMessage()));
            } catch (Dl.NotInProfile e) {
                Map<String, Object> out = new LinkedHashMap<>();
                out.put("error", "the input is not in OWL 2 DL");
                out.put("in_profile", false);
                out.put("violations", e.violations);
                out.put("backend", backend);
                out.put("warnings", e.warnings);
                reply = new Reply(422, out);
            } catch (OutOfMemoryError e) {
                throw e;
            } catch (Exception | Error e) {
                // Unexpected: keep the whole trace in the log, not in the answer.
                e.printStackTrace();
                reply = new Reply(500, Map.of("error", e.getClass().getSimpleName() + ": " + Dl.firstLine(e)));
            }
            send(ex, status = reply.status(), reply.body());
        } finally {
            System.err.printf("%s %s %d %d ms%n", ex.getRequestMethod(), ex.getRequestURI().getPath(), status,
                    (System.nanoTime() - started) / 1_000_000);
        }
    }

    private boolean authorised(HttpExchange ex) {
        if (cfg.token() == null) {
            return true;
        }
        String h = ex.getRequestHeaders().getFirst("Authorization");
        if (h == null || !h.startsWith("Bearer ")) {
            return false;
        }
        byte[] given = h.substring(7).trim().getBytes(StandardCharsets.UTF_8);
        return MessageDigest.isEqual(given, cfg.token().getBytes(StandardCharsets.UTF_8));
    }

    private static byte[] readCapped(InputStream in, long max) throws IOException {
        byte[] out = in.readNBytes((int) Math.min(Integer.MAX_VALUE - 8, max + 1));
        return out.length > max ? null : out;
    }

    private static void send(HttpExchange ex, int status, Object body) throws IOException {
        byte[] bytes = JSON.writeValueAsBytes(body);
        ex.getResponseHeaders().set("Content-Type", "application/json");
        ex.sendResponseHeaders(status, bytes.length);
        try (OutputStream os = ex.getResponseBody()) {
            os.write(bytes);
        }
    }

    private static String text(JsonNode body, String field, boolean required) throws Dl.BadInput {
        JsonNode n = body.get(field);
        if (n == null || n.isNull()) {
            if (required) {
                throw new Dl.BadInput("`" + field + "` is required");
            }
            return null;
        }
        if (!n.isTextual()) {
            throw new Dl.BadInput("`" + field + "` must be a string");
        }
        return n.asText();
    }

    private long timeout(JsonNode body) throws Dl.BadInput {
        JsonNode n = body.get("timeout_ms");
        if (n == null || n.isNull()) {
            return cfg.defaultTimeoutMs();
        }
        if (!n.canConvertToLong() || n.asLong() <= 0) {
            throw new Dl.BadInput("`timeout_ms` must be a positive integer");
        }
        return Math.min(n.asLong(), cfg.maxTimeoutMs());
    }
}
