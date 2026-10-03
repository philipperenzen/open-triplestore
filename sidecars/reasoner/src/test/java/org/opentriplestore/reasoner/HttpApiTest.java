package org.opentriplestore.reasoner;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.util.Map;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;

import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

class HttpApiTest {
    static HttpApi api;
    static String base;
    static final ObjectMapper JSON = new ObjectMapper();

    @BeforeAll
    static void start() throws Exception {
        api = new HttpApi(Config.fromEnv(Map.of(
                "OTS_REASONER_BIND", "127.0.0.1", "OTS_REASONER_PORT", "0", "OTS_REASONER_TOKEN", "s3cret")));
        base = "http://127.0.0.1:" + api.start();
    }

    @AfterAll
    static void stop() {
        api.stop();
    }

    static HttpResponse<String> post(String path, String token, Object body) throws Exception {
        HttpRequest.Builder b = HttpRequest.newBuilder(URI.create(base + path))
                .header("Content-Type", "application/json")
                .POST(HttpRequest.BodyPublishers.ofString(JSON.writeValueAsString(body)));
        if (token != null) b.header("Authorization", "Bearer " + token);
        return HttpClient.newHttpClient().send(b.build(), HttpResponse.BodyHandlers.ofString());
    }

    @Test
    void tokenIsRequired() throws Exception {
        assertEquals(401, post("/v1/check", null, Map.of("task", "consistency", "data", "")).statusCode());
        assertEquals(401, post("/v1/check", "wrong", Map.of("task", "consistency", "data", "")).statusCode());
    }

    @Test
    void reasonOverHttp() throws Exception {
        HttpResponse<String> r = post("/v1/reason", "s3cret", Map.of(
                "data", DlTest.nt("ex:A rdfs:subClassOf ex:B", "ex:a rdf:type ex:A"), "timeout_ms", 30_000));
        assertEquals(200, r.statusCode(), r.body());
        JsonNode j = JSON.readTree(r.body());
        assertTrue(j.get("consistent").asBoolean());
        assertTrue(j.get("complete").asBoolean());
        assertEquals("hermit", j.get("backend").get("name").asText());
        assertTrue(j.get("inferred").asText().contains("<http://example.org/a> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/B> ."));
    }

    @Test
    void notInProfileIs422WithViolations() throws Exception {
        HttpResponse<String> r = post("/v1/check", "s3cret", Map.of("task", "consistency",
                "data", DlTest.nt("ex:p rdf:type owl:ObjectProperty", "ex:p rdf:type owl:DatatypeProperty")));
        assertEquals(422, r.statusCode(), r.body());
        JsonNode j = JSON.readTree(r.body());
        assertTrue(!j.get("in_profile").asBoolean() && j.get("violations").size() > 0, r.body());
    }

    @Test
    void badRequestsAre400() throws Exception {
        assertEquals(400, post("/v1/check", "s3cret", Map.of("task", "consistency")).statusCode());
        assertEquals(400, post("/v1/check", "s3cret", Map.of("task", "consistency", "data", "not n-triples")).statusCode());
    }

    @Test
    void healthNeedsNoToken() throws Exception {
        HttpResponse<String> r = HttpClient.newHttpClient().send(
                HttpRequest.newBuilder(URI.create(base + "/version")).GET().build(), HttpResponse.BodyHandlers.ofString());
        assertEquals(200, r.statusCode());
        assertEquals(1, JSON.readTree(r.body()).get("protocol").asInt());
    }
}
