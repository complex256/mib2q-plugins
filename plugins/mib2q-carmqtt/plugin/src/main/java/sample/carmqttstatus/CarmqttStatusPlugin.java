package sample.carmqttstatus;

import java.io.IOException;
import java.util.HashMap;
import java.util.Map;
import mibsdk.api.Icon;
import mibsdk.api.MibPlugin;
import mibsdk.api.Net;
import mibsdk.api.Page;
import mibsdk.api.PageSpec;
import mibsdk.api.PluginContext;
import mibsdk.api.Scheduler;
import mibsdk.api.StatusItem;
import mibsdk.api.Ui;
import mibsdk.api.UiAction;

/**
 * The built-in status bar's MQTT icon (a plug), from the unit's carmqtt daemon on {@code 127.0.0.1:19851}: bright
 * when {@code mqtt=1}, dim otherwise (including when the daemon is unreachable). An icon-only status item
 * ({@link StatusItem#setIcon}); a small page shows the MQTT/ExLAP state, the last publish age and an ExLAP error.
 * Polls every {@value #POLL_MS} ms. The port can be overridden with the storage key {@code port}.
 */
public class CarmqttStatusPlugin extends MibPlugin {

    static final String PAGE_ID = "status";
    private static final String HOST = "127.0.0.1";
    private static final int DEFAULT_PORT = 19851;
    private static final int TIMEOUT_MS = 2000;
    private static final long POLL_MS = 5000;
    /** A publish older than this is flagged stale: {@code mqtt} is only the last known state. */
    private static final int STALE_S = 300;

    /** What the icon shows; ints, not an enum: a plugin-local enum pulls in the enum backport's java.util.concurrent. */
    static final int OFF = 0;
    static final int ON = 1;

    private Net net;
    private Scheduler scheduler;
    private Ui ui;
    private StatusItem item;
    private Page page;
    private Icon on;
    private Icon off;
    private int port;

    private volatile int state = OFF;
    private volatile boolean queued;
    private Map<String, String> last = new HashMap<String, String>();

    @Override
    protected void onEnable(PluginContext ctx) {
        net = ctx.net();
        scheduler = ctx.scheduler();
        ui = ctx.ui();
        port = port(ctx.storage().get("port", null));

        on = ui.icon("mqtt-on", Icon.OWN_COLOURS);
        off = ui.icon("mqtt-off", Icon.OWN_COLOURS);
        item = ui.statusItem(null); // hidden; setIcon shows the icon alone
        item.setIcon(off);

        page = ui.page(spec(last), this::onAction);
        scheduler.after(0, this::refresh);
        scheduler.every(POLL_MS, this::refresh);
    }

    private void onAction(UiAction action) {
        if ("refresh".equals(action.itemId()) && !queued) {
            queued = true;
            scheduler.after(0, this::refresh);
        }
    }

    /** The daemon's snapshot; an unreachable or foreign daemon reads as empty, which is {@link State#OFF}. */
    private void refresh() {
        queued = false;
        Map<String, String> m = query();
        last = m;
        state = stateOf(m);
        apply();
        page.update(spec(m));
    }

    private void apply() {
        item.setIcon(state == ON ? on : off);
    }

    private Map<String, String> query() {
        try {
            Map<String, String> m = parse(net.query(HOST, port, null, TIMEOUT_MS));
            return "carmqtt".equals(m.get("daemon")) ? m : new HashMap<String, String>();
        } catch (IOException e) {
            return new HashMap<String, String>();
        }
    }

    /** MQTT connected -> on; everything else (including absent or unreachable) -> off. */
    static int stateOf(Map<String, String> m) {
        return "1".equals(m.get("mqtt")) ? ON : OFF;
    }

    /** The daemon's {@code key=value\n} lines; a hand-written split (the car has no regex). */
    static Map<String, String> parse(String reply) {
        Map<String, String> m = new HashMap<String, String>();
        if (reply == null) {
            return m;
        }
        int at = 0;
        while (at < reply.length()) {
            int nl = reply.indexOf('\n', at);
            String line = nl < 0 ? reply.substring(at) : reply.substring(at, nl);
            at = nl < 0 ? reply.length() : nl + 1;
            int eq = line.indexOf('=');
            if (eq > 0) {
                String k = line.substring(0, eq).trim();
                if (k.length() > 0) {
                    m.put(k, line.substring(eq + 1).trim());
                }
            }
        }
        return m;
    }

    private static PageSpec spec(Map<String, String> m) {
        PageSpec p = PageSpec.of(PAGE_ID, "MQTT");
        if (m.isEmpty()) {
            p = p.info("carmqtt: ?");
        } else {
            p = p.info("MQTT: " + onOff(m.get("mqtt")) + ", ExLAP: " + onOff(m.get("exlap")))
                    .info("Last publish: " + publishText(intOr(m.get("last_publish_age_s"), Integer.MIN_VALUE)));
            String err = get(m, "exlap_err");
            if (err.length() > 0) {
                p = p.info("Error: " + err);
            }
        }
        return p.item("refresh", "Refresh now", null);
    }

    private static int port(String stored) {
        try {
            int p = Integer.parseInt(stored);
            return p > 0 && p < 65536 ? p : DEFAULT_PORT;
        } catch (NumberFormatException e) {
            return DEFAULT_PORT;
        }
    }

    /** "(stale)" is informational only: it does not change the icon. */
    private static String publishText(int age) {
        if (age == Integer.MIN_VALUE) {
            return "?";
        }
        return age < 0 ? "never" : age + " s ago" + (age > STALE_S ? " (stale)" : "");
    }

    private static String onOff(String flag) {
        return "1".equals(flag) ? "on" : "off";
    }

    private static String get(Map<String, String> m, String key) {
        String v = m.get(key);
        return v == null ? "" : v;
    }

    private static int intOr(String s, int fallback) {
        try {
            return Integer.parseInt(s);
        } catch (NumberFormatException e) {
            return fallback;
        }
    }
}
