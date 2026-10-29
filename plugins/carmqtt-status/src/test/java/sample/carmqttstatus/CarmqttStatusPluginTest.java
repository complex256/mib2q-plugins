package sample.carmqttstatus;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.File;
import java.util.HashMap;
import java.util.Map;
import mibsdk.test.PluginTestHarness;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

class CarmqttStatusPluginTest {

    @TempDir
    File tmp;

    private PluginTestHarness h;

    @AfterEach
    void tearDown() {
        if (h != null) {
            h.close();
        }
    }

    @Test
    void showsTheDimPlugWhileTheDaemonIsUnreachable() throws Exception {
        h = PluginTestHarness.start(tmp);
        assertEquals("ENABLED", h.state());
        assertEquals("carmqtt-status", h.pluginId());
        h.await(() -> !h.ui().statusParts().isEmpty(), "the icon-only item");
        String part = h.ui().statusParts().get(0);
        assertTrue(part.startsWith("[png:" + h.iconSha("mqtt-off")), part);
        assertTrue(h.ui().statusLine().isEmpty(), "an icon-only item has no text");
    }

    @Test
    void pageShowsTheUnreachableState() throws Exception {
        h = PluginTestHarness.start(tmp);
        String page = h.pageKey("status");
        h.await(() -> h.ui().render(page).contains("carmqtt: ?"), "the page");
    }

    @Test
    void mapsTheDaemonState() {
        assertEquals(CarmqttStatusPlugin.ON, CarmqttStatusPlugin.stateOf(map("mqtt", "1")));
        assertEquals(CarmqttStatusPlugin.OFF, CarmqttStatusPlugin.stateOf(map("mqtt", "0")));
        assertEquals(CarmqttStatusPlugin.OFF, CarmqttStatusPlugin.stateOf(new HashMap<String, String>()));
    }

    @Test
    void parsesTheKeyValueReply() {
        Map<String, String> m = CarmqttStatusPlugin.parse("daemon=carmqtt\nmqtt=1\nlast_publish_age_s=12\n");
        assertEquals("carmqtt", m.get("daemon"));
        assertEquals("1", m.get("mqtt"));
        assertEquals("12", m.get("last_publish_age_s"));
    }

    private static Map<String, String> map(String key, String value) {
        Map<String, String> m = new HashMap<String, String>();
        m.put(key, value);
        return m;
    }
}
