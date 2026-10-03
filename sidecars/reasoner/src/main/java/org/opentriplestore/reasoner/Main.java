package org.opentriplestore.reasoner;

import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.time.Duration;
import java.util.Map;

/**
 * Entry point. {@code java -jar ots-reasoner.jar} serves protocol v1;
 * {@code java -jar ots-reasoner.jar healthcheck} probes a running instance
 * (the image's HEALTHCHECK, which has no curl).
 */
public final class Main {
    private Main() {}

    public static void main(String[] args) throws Exception {
        Map<String, String> env = System.getenv();
        Config cfg = Config.fromEnv(env);
        if (args.length > 0 && args[0].equals("healthcheck")) {
            System.exit(healthcheck(cfg.port()));
        }
        if (cfg.token() == null && !Config.flag(env, "OTS_REASONER_ALLOW_NO_TOKEN")) {
            System.err.println("OTS_REASONER_TOKEN is not set. Set it to a shared secret (the triplestore "
                    + "sends it as a bearer token), or set OTS_REASONER_ALLOW_NO_TOKEN=1 to run without one.");
            System.exit(2);
        }
        HttpApi api = new HttpApi(cfg);
        int port = api.start();
        System.err.printf("ots-reasoner %s: HermiT %s, OWL API %s, protocol v%d on %s:%d (%d slot%s%s)%n",
                Versions.sidecar(), Versions.hermit(), Versions.owlapi(), HttpApi.PROTOCOL, cfg.bind(), port,
                cfg.concurrency(), cfg.concurrency() == 1 ? "" : "s",
                cfg.token() == null ? ", NO TOKEN" : "");
        Runtime.getRuntime().addShutdownHook(new Thread(api::stop));
    }

    static int healthcheck(int port) {
        try {
            HttpResponse<Void> r = HttpClient.newHttpClient().send(
                    HttpRequest.newBuilder(URI.create("http://127.0.0.1:" + port + "/health"))
                            .timeout(Duration.ofSeconds(3)).GET().build(),
                    HttpResponse.BodyHandlers.discarding());
            return r.statusCode() == 200 ? 0 : 1;
        } catch (Exception e) {
            return 1;
        }
    }
}
