package sample.tailscalestatus;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.File;
import java.util.HashMap;
import java.util.Map;
import mibsdk.test.PluginTestHarness;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

class TailscaleStatusPluginTest {

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
    void showsTheDimIconOnlyWhileTheSidecarIsUnreachable() throws Exception {
        h = PluginTestHarness.start(tmp);
        assertEquals("ENABLED", h.state());
        assertEquals("tailscale-status", h.pluginId());
        h.await(() -> !h.ui().statusParts().isEmpty(), "the icon-only item");
        String part = h.ui().statusParts().get(0);
        assertTrue(part.startsWith("[png:" + h.iconSha("tailscale-off")), part);
        assertTrue(h.ui().statusLine().isEmpty(), "an icon-only item has no text");
    }

    @Test
    void pageShowsTheUnreachableState() throws Exception {
        h = PluginTestHarness.start(tmp);
        String page = h.pageKey("status");
        h.await(() -> h.ui().render(page).contains("Tailscale: ?"), "the page");
    }

    @Test
    void mapsTheDaemonState() {
        assertEquals(TailscaleStatusPlugin.ON, TailscaleStatusPlugin.stateOf(map("state", "Running")));
        assertEquals(TailscaleStatusPlugin.FLICKER, TailscaleStatusPlugin.stateOf(map("state", "Starting")));
        assertEquals(TailscaleStatusPlugin.OFF, TailscaleStatusPlugin.stateOf(map("state", "Stopped")));
        assertEquals(TailscaleStatusPlugin.OFF, TailscaleStatusPlugin.stateOf(map("state", "NeedsLogin")));
        assertEquals(TailscaleStatusPlugin.OFF, TailscaleStatusPlugin.stateOf(map("state", "down")));
        assertEquals(TailscaleStatusPlugin.OFF, TailscaleStatusPlugin.stateOf(new HashMap<String, String>()));
    }

    @Test
    void parsesTheKeyValueReply() {
        Map<String, String> m = TailscaleStatusPlugin.parse("daemon=tailscale\nstate=Running\npeers_online=2\n");
        assertEquals("tailscale", m.get("daemon"));
        assertEquals("Running", m.get("state"));
        assertEquals("2", m.get("peers_online"));
    }

    private static Map<String, String> map(String key, String value) {
        Map<String, String> m = new HashMap<String, String>();
        m.put(key, value);
        return m;
    }
}
