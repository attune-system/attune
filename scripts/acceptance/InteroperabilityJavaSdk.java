import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.core.type.TypeReference;
import io.attune.AttuneClient;
import io.attune.ApprovalModels.InputRequest;
import io.attune.ApprovalModels.RuntimeRequest;
import io.attune.AttuneApiException;
import java.util.LinkedHashMap;
import java.util.Map;

/** Centrally compile against the actual SDK/Jackson jars, then run without Maven. */
public final class InteroperabilityJavaSdk {
    private static final ObjectMapper JSON = new ObjectMapper();
    private static final TypeReference<Map<String, Object>> MAP = new TypeReference<>() {};

    private static Map<String, Object> map(JsonNode value) {
        return JSON.convertValue(value, MAP);
    }

    private static Map<String, Object> data(Map<String, Object> response) {
        return response.containsKey("data") ? JSON.convertValue(response.get("data"), MAP) : response;
    }

    private static Object call(JsonNode r) throws Exception {
        var client = AttuneClient.controlPlane(r.get("api_url").asText(), r.get("access_token").asText());
        var approvals = client.approvals();
        String pack = r.path("pack").asText();
        String ref = r.path("ref").asText();
        // References are validated by the parent against run-owned safe names.
        String packPath = "/api/v1/packs/" + pack;
        if (r.path("expect_denial").asBoolean()) {
            if (!r.get("operation").asText().equals("execute")) throw new IllegalArgumentException("Denial must execute actual invocation");
            try {
                client.post("/api/v1/executions/execute", map(r.get("body")));
            } catch (AttuneApiException error) {
                var diagnostic = new LinkedHashMap<String, Object>();
                diagnostic.put("status", error.status());
                diagnostic.put("code", error.code());
                diagnostic.put("request_id", error.requestId());
                return Map.of("native_denial", diagnostic);
            }
            throw new IllegalStateException("Native SDK accepted revoked/expired authority");
        }
        switch (r.get("operation").asText()) {
            case "runtime_issue": return approvals.issueRuntime(pack, JSON.treeToValue(r.get("body"), RuntimeRequest.class));
            case "runtime_replace": return approvals.replaceRuntime(pack, r.get("id").asLong(), JSON.treeToValue(r.get("body"), RuntimeRequest.class));
            case "enqueue": return client.post("/api/v1/queues/" + ref + "/items", map(r.get("body")));
            case "targets": return approvals.listInputApprovalTargets(pack);
            case "issue": return approvals.issueInput(pack, JSON.treeToValue(r.get("body"), InputRequest.class));
            case "approval_get": return approvals.get(pack, r.get("id").asLong());
            case "approval_revoke": return approvals.revoke(pack, r.get("id").asLong());
            case "execute": return client.post("/api/v1/executions/execute", map(r.get("body")));
            case "pack_get": return client.get(packPath);
            case "config_set": return client.put(packPath, Map.of("config", map(r.get("config"))));
            case "config_reset": return client.put(packPath, Map.of("config_reset_fields", JSON.convertValue(r.get("fields"), Object.class)));
            case "schema_get": return client.get("/api/v1/schema-types/" + ref);
            case "queue_get": return client.get("/api/v1/queues/" + ref);
            case "queue_grouping": {
                String path = "/api/v1/queues/" + ref;
                var current = data(client.get(path));
                var config = current.get("config") == null ? new LinkedHashMap<String, Object>() : JSON.convertValue(current.get("config"), MAP);
                var dispatch = config.get("dispatch") == null ? new LinkedHashMap<String, Object>() : JSON.convertValue(config.get("dispatch"), MAP);
                dispatch.put("separate_requesters", r.get("grouping").asText().equals("separate"));
                config.put("dispatch", dispatch);
                return client.put(path, Map.of("config", config));
            }
            case "permission_export": {
                var current = data(client.get("/api/v1/permissions/sets/by-ref/" + ref));
                if (current.get("authored_definition") == null) throw new IllegalStateException("No canonical authored definition");
                var definition = JSON.convertValue(current.get("authored_definition"), MAP);
                definition.put("label", current.get("label"));
                definition.put("description", current.get("description"));
                return definition;
            }
            case "permission_import": {
                var current = data(client.get("/api/v1/permissions/sets/by-ref/" + ref));
                var definition = map(r.get("definition"));
                definition.remove("ref");
                return client.put("/api/v1/permissions/sets/" + current.get("id"), definition);
            }
            default: throw new IllegalArgumentException("Unsupported SDK operation");
        }
    }

    public static void main(String[] args) {
        try {
            byte[] input = System.in.readNBytes(1024 * 1024 + 1);
            if (input.length > 1024 * 1024) throw new IllegalArgumentException("Input limit");
            System.out.print(JSON.writeValueAsString(call(JSON.readTree(input))));
        } catch (Exception failure) {
            System.err.println("Real Java SDK operation failed; response and credentials suppressed");
            System.exit(1);
        }
    }
}
