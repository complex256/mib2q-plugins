package sample.tailscalestatus;

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
 * The built-in status bar's Tailscale icon, from the unit's {@code tsstatus} sidecar on {@code 127.0.0.1:19850}:
 * <ul>
 * <li>bright when {@code state=Running};</li>
 * <li>flickering (bright/dim every {@value #FLICKER_MS} ms) while it is {@code Starting};</li>
 * <li>dim otherwise, including when the sidecar is unreachable.</li>
 * </ul>
 * An icon-only status item ({@link StatusItem#setIcon}); a small page shows the state, address, tailnet and peers.
 * Polls every {@value #POLL_MS} ms. The port can be overridden with the storage key {@code port}.
 */
public class TailscaleStatusPlugin extends MibPlugin {

    static final String PAGE_ID = "status";
    private static final String HOST = "127.0.0.1";
    private static final int DEFAULT_PORT = 19850;
    private static final int TIMEOUT_MS = 2000;
    private static final long POLL_MS = 5000;
    private static final long FLICKER_MS = 400;

    /** What the icon shows; ints, not an enum: a plugin-local enum pulls in the enum backport's java.util.concurrent. */
    static final int OFF = 0;
    static final int FLICKER = 1;
    static final int ON = 2;

    private Net net;
    private Scheduler scheduler;
    private Ui ui;
    private StatusItem item;
    private Page page;
    private Icon on;
    private Icon off;
    private int port;

    private volatile int state = OFF;
    private volatile boolean flickOn;
    private volatile boolean queued;
    private Map<String, String> last = new HashMap<String, String>();

    @Override
    protected void onEnable(PluginContext ctx) {
        net = ctx.net();
        scheduler = ctx.scheduler();
        ui = ctx.ui();
        port = port(ctx.storage().get("port", null));

        on = ui.icon("tailscale-on", Icon.OWN_COLOURS);
        off = ui.icon("tailscale-off", Icon.OWN_COLOURS);
        item = ui.statusItem(null); // hidden; setIcon shows the icon alone
        item.setIcon(off);

        page = ui.page(spec(last), this::onAction);
        scheduler.after(0, this::refresh);
        scheduler.every(POLL_MS, this::refresh);
        scheduler.every(FLICKER_MS, this::flick);
    }

    private void onAction(UiAction action) {
        if ("refresh".equals(action.itemId()) && !queued) {
            queued = true;
            scheduler.after(0, this::refresh);
        }
    }

    /** The sidecar's snapshot; an unreachable or foreign daemon reads as empty, which is {@link State#OFF}. */
    private void refresh() {
        queued = false;
        Map<String, String> m = query();
        last = m;
        state = stateOf(m);
        apply();
        page.update(spec(m));
    }

    private void flick() {
        if (state == FLICKER) {
            flickOn = !flickOn;
            apply();
        }
    }

    private void apply() {
        if (state == ON) {
            item.setIcon(on);
        } else if (state == FLICKER) {
            item.setIcon(flickOn ? on : off);
        } else {
            item.setIcon(off);
        }
    }

    private Map<String, String> query() {
        try {
            Map<String, String> m = parse(net.query(HOST, port, null, TIMEOUT_MS));
            return "tailscale".equals(m.get("daemon")) ? m : new HashMap<String, String>();
        } catch (IOException e) {
            return new HashMap<String, String>();
        }
    }

    /** Running -> on, Starting -> flicker, everything else (Stopped, NeedsLogin, down, absent) -> off. */
    static int stateOf(Map<String, String> m) {
        String s = m.get("state");
        if ("Running".equals(s)) {
            return ON;
        }
        if ("Starting".equals(s)) {
            return FLICKER;
        }
        return OFF;
    }

    /** The sidecar's {@code key=value\n} lines; a hand-written split (the car has no regex). */
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
        PageSpec p = PageSpec.of(PAGE_ID, "Tailscale");
        if (m.isEmpty()) {
            p = p.info("Tailscale: ?");
        } else {
            String ip = get(m, "ip");
            p = p.info("State: " + get(m, "state") + (ip.length() == 0 ? "" : " " + ip))
                    .info("Tailnet: " + get(m, "tailnet"))
                    .info("Peers: " + num(m, "peers_online") + "/" + num(m, "peers"));
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

    private static String get(Map<String, String> m, String key) {
        String v = m.get(key);
        return v == null ? "" : v;
    }

    private static String num(Map<String, String> m, String key) {
        try {
            return String.valueOf(Integer.parseInt(get(m, key)));
        } catch (NumberFormatException e) {
            return "?";
        }
    }
}
