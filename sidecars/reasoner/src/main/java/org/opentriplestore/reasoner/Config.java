package org.opentriplestore.reasoner;

import java.util.Map;

/** The sidecar's settings, from the environment. */
record Config(
        String bind,
        int port,
        String token,
        int concurrency,
        long maxBodyBytes,
        long defaultTimeoutMs,
        long maxTimeoutMs,
        long queueWaitMs,
        int explainMaxAxioms) {

    static Config fromEnv(Map<String, String> env) {
        String token = env.getOrDefault("OTS_REASONER_TOKEN", "").trim();
        return new Config(
                env.getOrDefault("OTS_REASONER_BIND", "0.0.0.0"),
                (int) num(env, "OTS_REASONER_PORT", 8090),
                token.isEmpty() ? null : token,
                (int) Math.max(1, num(env, "OTS_REASONER_CONCURRENCY", 2)),
                num(env, "OTS_REASONER_MAX_BODY_MB", 512) * 1024 * 1024,
                num(env, "OTS_REASONER_DEFAULT_TIMEOUT_MS", 300_000),
                num(env, "OTS_REASONER_MAX_TIMEOUT_MS", 3_600_000),
                num(env, "OTS_REASONER_QUEUE_WAIT_MS", 60_000),
                (int) num(env, "OTS_REASONER_EXPLAIN_MAX_AXIOMS", 2_000));
    }

    static boolean flag(Map<String, String> env, String key) {
        String v = env.getOrDefault(key, "").trim().toLowerCase();
        return v.equals("1") || v.equals("true") || v.equals("yes");
    }

    private static long num(Map<String, String> env, String key, long dflt) {
        String v = env.get(key);
        if (v == null || v.isBlank()) {
            return dflt;
        }
        try {
            return Long.parseLong(v.trim());
        } catch (NumberFormatException e) {
            throw new IllegalArgumentException(key + "=" + v + " is not a number");
        }
    }
}
