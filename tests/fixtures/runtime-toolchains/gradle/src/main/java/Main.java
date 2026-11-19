import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;

public class Main {
    public static void main(String[] args) throws Exception {
        try (var resource = Main.class.getResourceAsStream("/message.txt")) {
            if (resource == null || !new String(resource.readAllBytes(), StandardCharsets.UTF_8)
                    .trim().equals("local-resource-ok")) {
                throw new IllegalStateException("Bundled resource unavailable");
            }
        }
        System.out.println(new BufferedReader(new InputStreamReader(System.in)).readLine());
    }
}
