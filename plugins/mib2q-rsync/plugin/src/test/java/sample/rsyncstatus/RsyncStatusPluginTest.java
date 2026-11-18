package sample.rsyncstatus;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.BufferedReader;
import java.io.Closeable;
import java.io.File;
import java.io.IOException;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.net.InetAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Properties;
import mibsdk.test.PluginTestHarness;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

class RsyncStatusPluginTest {

    @TempDir
    File tmp;

    private PluginTestHarness h;
    private Daemon daemon;

    @AfterEach
    void tearDown() throws Exception {
        if (h != null) {
            h.close();
        }
        if (daemon != null) {
            daemon.close();
        }
    }

    @Test
    void showsTheDimIconOnlyWhileTheDaemonIsUnreachable() throws Exception {
        h = harness(closedPort());
        assertEquals("ENABLED", h.state());
        assertEquals("rsync-status", h.pluginId());
        h.await(() -> !h.ui().statusParts().isEmpty(), "the icon-only item");
        String part = h.ui().statusParts().get(0);
        assertTrue(part.startsWith("[png:" + h.iconSha("rsync-off") + "+own] "), part);
        assertTrue(h.ui().statusLine().isEmpty(), "an icon-only item has no text");
    }

    @Test
    void pageShowsTheUnreachableState() throws Exception {
        h = harness(closedPort());
        String page = h.pageKey("status");
        h.await(() -> h.ui().render(page).contains("rsyncd: ?"), "the page");
        assertTrue(h.ui().render(page).contains("Refresh now"), "the actions stay available");
    }

    @Test
    void parsesTheKeyValueSnapshotWithJobs() {
        Map<String, String> m = RsyncStatusPlugin.parse("daemon=rsyncd\nstate=syncing\nmode=auto\ntailnet=1\n"
                + "jobs=2\njob.0.id=sd\njob.0.name=SD card\njob.0.state=syncing\njob.0.last_ok_age_s=12\n"
                + "job.0.last_changed=3\njob.0.current_file=/media/sdcard/music/a.flac\njob.0.last_err=\n"
                + "job.1.id=usb\njob.1.name=USB\njob.1.state=wait_media\n");
        assertEquals("rsyncd", m.get("daemon"));
        assertEquals("syncing", m.get("state"));
        assertEquals("2", m.get("jobs"));
        assertEquals("sd", m.get("job.0.id"));
        assertEquals("SD card", m.get("job.0.name"));
        assertEquals("/media/sdcard/music/a.flac", m.get("job.0.current_file"));
        assertEquals("wait_media", m.get("job.1.state"));
    }

    @Test
    void mapsTheDaemonStates() {
        assertEquals(RsyncStatusPlugin.SYNCING, RsyncStatusPlugin.stateOf(map("state", "syncing")));
        assertEquals(RsyncStatusPlugin.IDLE, RsyncStatusPlugin.stateOf(map("state", "idle")));
        assertEquals(RsyncStatusPlugin.OFF, RsyncStatusPlugin.stateOf(map("state", "waiting")));
        assertEquals(RsyncStatusPlugin.OFF, RsyncStatusPlugin.stateOf(map("state", "error")));
        assertEquals(RsyncStatusPlugin.OFF, RsyncStatusPlugin.stateOf(new HashMap<String, String>()));
    }

    @Test
    void cyclesTheModes() {
        assertEquals("wifi_allowlist", RsyncStatusPlugin.nextMode("auto"));
        assertEquals("manual", RsyncStatusPlugin.nextMode("wifi_allowlist"));
        assertEquals("auto", RsyncStatusPlugin.nextMode("manual"));
        assertEquals("auto", RsyncStatusPlugin.nextMode(""));
    }

    @Test
    void rendersEveryJobWithItsStateAndAge() {
        Map<String, String> m = RsyncStatusPlugin.parse(snapshot("idle", "idle"));
        m.put("jobs", "3");
        m.put("job.1.id", "usb");
        m.put("job.1.name", "USB stick");
        m.put("job.1.state", "waiting_media");
        m.put("job.1.last_ok_age_s", "300");
        m.put("job.1.last_changed", "0");
        m.put("job.2.id", "bad id!");
        m.put("job.2.name", "Broken");
        m.put("job.2.state", "failed");
        m.put("job.2.last_err", "sdcard full");
        m.put("job.2.last_ok_age_s", "7");
        m.put("job.2.last_changed", "2");
        List<String> lines = lines(RsyncStatusPlugin.spec(m));
        assertTrue(lines.contains("State: idle, Mode: auto, Tailnet: on"), lines.toString());
        assertTrue(lines.contains("SD card: 12 (+3)"), lines.toString());
        assertTrue(lines.contains("USB stick: wait media"), lines.toString());
        assertTrue(lines.contains("Broken: err"), lines.toString());
        assertTrue(lines.contains("Sync all now"), lines.toString());
        assertTrue(lines.contains("Cycle mode: auto"), lines.toString());
        assertTrue(lines.contains("Refresh now"), lines.toString());
    }

    @Test
    void showsTheFirstJobsError() {
        Map<String, String> m = RsyncStatusPlugin.parse(snapshot("idle", "idle"));
        m.put("job.0.last_err", "sdcard full");
        List<String> lines = lines(RsyncStatusPlugin.spec(m));
        assertTrue(lines.contains("Error: sdcard full"), lines.toString());
    }

    @Test
    void queriesRendersAndSendsCommandsEndToEnd() throws Exception {
        daemon = new Daemon("idle");
        h = harness(daemon.port());
        String page = h.pageKey("status");

        h.await(() -> h.ui().carRows().contains("[png:" + h.iconSha("rsync-on") + "] Rsync"), "the Car row");
        h.await(() -> h.ui().render(page).contains("State: idle, Mode: auto, Tailnet: on"), "the status page");
        assertTrue(h.ui().render(page).contains("SD card: 12 (+3)"), h.ui().render(page).toString());
        assertTrue(h.ui().render(page).contains("USB: wait media"), h.ui().render(page).toString());
        assertTrue(h.ui().render(page).contains("Sync all now"), h.ui().render(page).toString());
        String part = h.ui().statusParts().get(0);
        assertTrue(part.startsWith("[png:" + h.iconSha("rsync-on") + "+own] "), part);
        assertTrue(h.ui().statusLine().isEmpty(), "idle is icon-only");

        h.ui().select(page, "sync");
        h.await(() -> daemon.commands().contains("sync"), "the sync command");
        h.await(() -> h.ui().render(page).contains("Cancel sync"), "the syncing page");
        assertEquals("Sync", h.ui().statusLine());

        h.ui().select(page, "cancel");
        h.await(() -> daemon.commands().contains("cancel"), "the cancel command");
        h.await(() -> h.ui().render(page).contains("Sync all now"), "the idle page");

        h.ui().select(page, "job.sd");
        h.await(() -> daemon.commands().contains("sync sd"), "the job command");

        h.ui().select(page, "mode");
        h.await(() -> daemon.commands().contains("mode wifi_allowlist"), "the mode command");

        int before = daemon.statusCount();
        h.ui().select(page, "refresh");
        h.await(() -> daemon.statusCount() > before, "a refreshed query");
    }

    @Test
    void toastsTheChangeCountWhenASyncCompletes() throws Exception {
        daemon = new Daemon("syncing");
        h = harness(daemon.port());
        String page = h.pageKey("status");
        h.await(() -> h.ui().render(page).contains("Cancel sync"), "the syncing page");

        daemon.setState("idle");
        h.ui().select(page, "refresh");
        h.await(() -> "Synced +3".equals(h.ui().toast()), "the completion toast");
    }

    @Test
    void asksTheDaemonToCancelWhenTheIgnitionTurnsOff() throws Exception {
        daemon = new Daemon("idle");
        h = harness(daemon.port());
        h.await(() -> h.ui().render(h.pageKey("status")).contains("State: idle, Mode: auto, Tailnet: on"),
                "the status page");

        h.car().setIgnition(false);
        h.await(() -> daemon.commands().contains("cancel"), "the cancel command");
    }

    private PluginTestHarness harness(int port) {
        Properties stored = new Properties();
        stored.setProperty("port", String.valueOf(port));
        return PluginTestHarness.builder(tmp).storedValues(stored).start();
    }

    private static int closedPort() throws IOException {
        ServerSocket s = new ServerSocket(0, 1, InetAddress.getByName("127.0.0.1"));
        try {
            return s.getLocalPort();
        } finally {
            s.close();
        }
    }

    private static Map<String, String> map(String key, String value) {
        Map<String, String> m = new HashMap<String, String>();
        m.put(key, value);
        return m;
    }

    private static List<String> lines(mibsdk.api.ui.PageSpec spec) {
        List<String> out = new ArrayList<String>();
        for (mibsdk.api.ui.PageSpec.Entry e : spec.entries()) {
            out.add(e.kind() == mibsdk.api.ui.PageSpec.Kind.INFO ? e.label() : line(e));
        }
        return out;
    }

    private static String line(mibsdk.api.ui.PageSpec.Entry e) {
        String value = e.value();
        return value == null || value.length() == 0 ? e.label() : e.label() + ": " + value;
    }

    private static String snapshot(String state, String jobState) {
        return "daemon=rsyncd\nstate=" + state + "\nmode=auto\ntailnet=1\njobs=2\n"
                + "job.0.id=sd\njob.0.name=SD card\njob.0.state=" + jobState + "\njob.0.last_ok_age_s=12\n"
                + "job.0.last_changed=3\njob.0.current_file=/media/sdcard/a.flac\njob.0.last_err=\n"
                + "job.1.id=usb\njob.1.name=USB\njob.1.state=wait_media\njob.1.last_ok_age_s=300\n"
                + "job.1.last_changed=0\njob.1.current_file=\njob.1.last_err=\n";
    }

    /** A tiny rsyncd stand-in: records each request line and answers with a full status snapshot. */
    private static final class Daemon implements Closeable {

        private final ServerSocket server;
        private final List<String> commands = Collections.synchronizedList(new ArrayList<String>());
        private final Thread thread;
        private volatile String state;
        private volatile int statuses;

        Daemon(String state) throws IOException {
            this.state = state;
            this.server = new ServerSocket(0, 50, InetAddress.getByName("127.0.0.1"));
            this.thread = new Thread(this::loop, "rsyncd-fake");
            this.thread.setDaemon(true);
            this.thread.start();
        }

        int port() {
            return server.getLocalPort();
        }

        void setState(String state) {
            this.state = state;
        }

        List<String> commands() {
            return commands;
        }

        int statusCount() {
            return statuses;
        }

        private void loop() {
            while (!server.isClosed()) {
                try (Socket s = server.accept()) {
                    BufferedReader in = new BufferedReader(new InputStreamReader(s.getInputStream(), "UTF-8"));
                    String cmd = in.readLine();
                    if (cmd == null) {
                        cmd = "";
                    }
                    commands.add(cmd);
                    String reply = reply(cmd);
                    OutputStream out = s.getOutputStream();
                    out.write(reply.getBytes("UTF-8"));
                    out.flush();
                } catch (IOException e) {
                    return;
                }
            }
        }

        private String reply(String cmd) {
            if ("status".equals(cmd)) {
                statuses++;
            }
            String next = state;
            if ("sync".equals(cmd) || cmd.startsWith("sync ")) {
                next = "syncing";
            } else if ("cancel".equals(cmd)) {
                next = "idle";
            }
            state = next;
            return snapshot(next, "syncing".equals(next) ? "syncing" : "idle");
        }

        @Override
        public void close() throws IOException {
            server.close();
        }
    }
}
