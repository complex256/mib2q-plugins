package sample.rsyncstatus;

import java.io.IOException;
import java.util.HashMap;
import java.util.Map;
import mibsdk.api.Features;
import mibsdk.api.Icon;
import mibsdk.api.IgnitionChanged;
import mibsdk.api.MibPlugin;
import mibsdk.api.Net;
import mibsdk.api.Page;
import mibsdk.api.PageSpec;
import mibsdk.api.PluginContext;
import mibsdk.api.Scheduler;
import mibsdk.api.StatusItem;
import mibsdk.api.Ui;
import mibsdk.api.UiAction;
import mibsdk.api.WifiStatus;

/**
 * The built-in status bar's Rsync icon and page, from the unit's {@code rsyncd} on {@code 127.0.0.1:19852}:
 * <ul>
 * <li>bright when the daemon is healthy ({@code idle}) or a job is {@code syncing};</li>
 * <li>dim when the daemon is unreachable, waiting, or reports an error.</li>
 * </ul>
 * While a job syncs the item reads {@code Sync}; otherwise it is icon-only. The page shows the daemon state, mode and
 * tailnet, a row per job, the last error and the sync/cancel/mode/refresh actions. The row of a job starts that job.
 * Polls every {@value #POLL_MS} ms. The port can be overridden with the storage key {@code port}.
 *
 * <p>Ignition off asks the daemon to cancel, so an active SD-card write stops cleanly. Wi-Fi changes are forwarded
 * to the daemon as {@code ssid <name>}, so it can apply its {@code wifi_allowlist} mode.
 */
public class RsyncStatusPlugin extends MibPlugin {

    static final String PAGE_ID = "status";
    private static final String HOST = "127.0.0.1";
    private static final int DEFAULT_PORT = 19852;
    private static final int TIMEOUT_MS = 2000;
    private static final long POLL_MS = 5000;

    /** The most job rows the page draws (job.0 .. job.5). */
    private static final int MAX_JOBS = 6;

    /** The page item id limit, matching {@link PageSpec}'s {@code [A-Za-z0-9._-]{1,32}}. */
    private static final int MAX_ITEM_ID = 32;

    /** What the icon shows; ints, not an enum: a plugin-local enum pulls in the enum backport's java.util.concurrent. */
    static final int OFF = 0;
    static final int IDLE = 1;
    static final int SYNCING = 2;

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

        on = ui.icon("rsync-on", Icon.OWN_COLOURS);
        off = ui.icon("rsync-off", Icon.OWN_COLOURS);
        item = ui.statusItem(null); // hidden; setIcon shows the icon alone
        item.setIcon(off);

        page = ui.page(spec(last), this::onAction);
        ui.carMenuRow("Rsync", page, on);

        // Terminal 15 off: stop an active SD-card write before the unit loses power.
        if (ctx.has(Features.IGNITION)) {
            ctx.events().on(IgnitionChanged.class, ev -> {
                if (!ev.isOn()) {
                    scheduler.after(0, () -> sendCommand("cancel"));
                }
            });
        }

        // The daemon applies its wifi_allowlist mode from the current SSID.
        if (ctx.has(Features.WIFI)) {
            ctx.wifi().onChanged(ev -> {
                WifiStatus st = ev.status();
                String ssid = st != null && st.connected() ? st.ssid() : "";
                scheduler.after(0, () -> sendCommand("ssid " + ssid));
            });
        }

        scheduler.after(0, this::refresh);
        scheduler.every(POLL_MS, this::refresh);
    }

    private void onAction(UiAction action) {
        String id = action.itemId();
        if (id == null) {
            return;
        }
        if ("refresh".equals(id)) {
            if (!queued) {
                queued = true;
                scheduler.after(0, this::refresh);
            }
        } else if ("sync".equals(id)) {
            command("sync");
        } else if ("cancel".equals(id)) {
            command("cancel");
        } else if ("mode".equals(id)) {
            command("mode " + nextMode(get(last, "mode")));
        } else if (id.startsWith("job.")) {
            String job = jobIdFor(id);
            if (job.length() > 0) {
                command("sync " + job);
            }
        }
    }

    private void command(String cmd) {
        scheduler.after(0, () -> sendCommand(cmd));
    }

    /** The daemon's snapshot; an unreachable or foreign daemon reads as empty, which is {@link #OFF}. */
    private void refresh() {
        queued = false;
        int previous = state;
        Map<String, String> m = query();
        last = m;
        state = stateOf(m);
        apply();
        page.update(spec(m));
        if (previous == SYNCING && state == IDLE) {
            toastSynced(m);
        }
    }

    /** Sends {@code cmd}; the daemon answers with a fresh snapshot, which updates the icon and the page. */
    private void sendCommand(String cmd) {
        Map<String, String> m = query(cmd);
        last = m;
        state = stateOf(m);
        apply();
        page.update(spec(m));
    }

    private void apply() {
        if (state == SYNCING) {
            item.set("Sync", on);
        } else {
            item.setIcon(state == IDLE ? on : off);
        }
    }

    private void toastSynced(Map<String, String> m) {
        int changed = intOr(get(m, "job.0.last_changed"), 0);
        if (changed <= 0 || get(m, "job.0.last_err").length() > 0) {
            return;
        }
        String text = "Synced +" + changed;
        if (text.length() > Ui.MAX_TOAST) {
            text = text.substring(0, Ui.MAX_TOAST);
        }
        ui.toast(text, null);
    }

    private Map<String, String> query() {
        return query("status");
    }

    private Map<String, String> query(String request) {
        try {
            Map<String, String> m = parse(net.query(HOST, port, request, TIMEOUT_MS));
            return "rsyncd".equals(m.get("daemon")) ? m : new HashMap<String, String>();
        } catch (IOException e) {
            return new HashMap<String, String>();
        }
    }

    /** syncing -> syncing, idle -> idle, everything else (waiting, error, absent, unreachable) -> off. */
    static int stateOf(Map<String, String> m) {
        if (m.isEmpty()) {
            return OFF;
        }
        String s = m.get("state");
        if (s != null) {
            s = s.toLowerCase();
        }
        if ("syncing".equals(s)) {
            return SYNCING;
        }
        if ("idle".equals(s)) {
            return IDLE;
        }
        return OFF;
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

    static PageSpec spec(Map<String, String> m) {
        PageSpec p = PageSpec.of(PAGE_ID, "Rsync");
        if (m.isEmpty()) {
            p = p.info("rsyncd: ?");
        } else {
            p = p.info(clip("State: " + get(m, "state") + ", Mode: " + get(m, "mode") + ", Tailnet: "
                    + onOff(m.get("tailnet")), PageSpec.MAX_INFO));
            int jobs = intOr(get(m, "jobs"), 0);
            int shown = Math.min(jobs, MAX_JOBS);
            for (int i = 0; i < shown; i++) {
                String prefix = "job." + i + ".";
                String raw = get(m, prefix + "id");
                String id = jobId(raw, i);
                String name = get(m, prefix + "name");
                String file = get(m, prefix + "current_file");
                String jobState = get(m, prefix + "state");
                String err = get(m, prefix + "last_err");
                int age = intOr(get(m, prefix + "last_ok_age_s"), Integer.MIN_VALUE);
                int changed = intOr(get(m, prefix + "last_changed"), 0);
                String label = name.length() > 0 ? name : file.length() > 0 ? file : id;
                p = p.item(jobItemId(id, i), clip(label, PageSpec.MAX_LABEL),
                        clip(jobText(jobState, err, age, changed), PageSpec.MAX_VALUE));
            }
            String err = get(m, "job.0.last_err");
            if (err.length() > 0) {
                p = p.info(clip("Error: " + err, PageSpec.MAX_INFO));
            }
        }
        if ("syncing".equalsIgnoreCase(get(m, "state"))) {
            p = p.item("cancel", "Cancel sync", null);
        } else {
            p = p.item("sync", "Sync all now", null);
        }
        return p.item("mode", "Cycle mode", get(m, "mode")).item("refresh", "Refresh now", null);
    }

    /** The job's value cell: what it is doing, or its last successful age and change count. */
    private static String jobText(String state, String err, int age, int changed) {
        String s = state.toLowerCase();
        if ("syncing".equals(s)) {
            return "syncing";
        }
        if (s.indexOf("wait") >= 0) {
            return "wait media";
        }
        if ("error".equals(s) || "failed".equals(s) || "err".equals(s)) {
            return "err";
        }
        if (err.length() > 0) {
            return "err";
        }
        return age == Integer.MIN_VALUE ? "?" : age + " (+" + changed + ")";
    }

    /** auto -> wifi_allowlist -> manual -> auto; an absent or unknown mode starts at auto. */
    static String nextMode(String mode) {
        if ("auto".equals(mode)) {
            return "wifi_allowlist";
        }
        if ("wifi_allowlist".equals(mode)) {
            return "manual";
        }
        return "auto";
    }

    private String jobIdFor(String itemId) {
        int jobs = intOr(get(last, "jobs"), 0);
        int shown = Math.min(jobs, MAX_JOBS);
        for (int i = 0; i < shown; i++) {
            String raw = jobId(get(last, "job." + i + ".id"), i);
            if (jobItemId(raw, i).equals(itemId)) {
                return raw;
            }
        }
        return itemId.length() > 4 ? itemId.substring(4) : "";
    }

    /** A job's raw id, or its index when the daemon left it empty. */
    private static String jobId(String raw, int index) {
        return raw == null || raw.length() == 0 ? String.valueOf(index) : raw;
    }

    /** The page item id {@code "job." + id}, cleaned to the page-id alphabet and at most 32 characters. */
    private static String jobItemId(String raw, int index) {
        String id = jobId(raw, index);
        StringBuffer sb = new StringBuffer("job.");
        for (int i = 0; i < id.length() && sb.length() < MAX_ITEM_ID; i++) {
            char c = id.charAt(i);
            boolean ok = (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9')
                    || c == '.' || c == '_' || c == '-';
            sb.append(ok ? c : '_');
        }
        if (sb.length() == 4) {
            sb.append(index);
        }
        return sb.toString();
    }

    private static int port(String stored) {
        try {
            int p = Integer.parseInt(stored);
            return p > 0 && p < 65536 ? p : DEFAULT_PORT;
        } catch (NumberFormatException e) {
            return DEFAULT_PORT;
        }
    }

    private static String onOff(String flag) {
        return "1".equals(flag) ? "on" : "off";
    }

    private static String get(Map<String, String> m, String key) {
        String v = m.get(key);
        return v == null ? "" : v;
    }

    private static String clip(String s, int max) {
        return s.length() <= max ? s : s.substring(0, max);
    }

    private static int intOr(String s, int fallback) {
        try {
            return Integer.parseInt(s);
        } catch (NumberFormatException e) {
            return fallback;
        }
    }
}
