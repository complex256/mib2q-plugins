package sample.inputsdemo;

import mibsdk.api.Icon;
import mibsdk.api.Listener;
import mibsdk.api.Log;
import mibsdk.api.MibPlugin;
import mibsdk.api.Page;
import mibsdk.api.PageSpec;
import mibsdk.api.PluginContext;
import mibsdk.api.Scheduler;
import mibsdk.api.StockIcons;
import mibsdk.api.Storage;
import mibsdk.api.Ui;
import mibsdk.api.UiAction;

/**
 * Sample api-5 plugin: one page, "Inputs", with every native input. It has no Car row: open it from MENU, Plugins.
 *
 * <ul>
 *   <li>toggles (native checkboxes): Wi-Fi; Red, Green and Blue (a multi-choice, each on its own); Locked, disabled;
 *   <li>a choice (Language, its own sub-page), a dropdown (Level) and two sliders: Bass, -6..6 centred, and Volume,
 *       0..10 in steps of 2, not centred;
 *   <li>a value column (Version), item icons (Plug, Plug value), a progress bar and a row that moves it by 20;
 *   <li>Swap kind, which turns "Swap me" from a toggle into an item and back;
 *   <li>two confirms (one long line, two short lines), a toast and a toast 5 s later (the plugin's Scheduler);
 *   <li>Name, which opens the experimental speller (BACK is done); a changed name adds a Save row.
 * </ul>
 *
 * The last line shows the last action. Every action is logged at INFO. The name, the level and both slider levels
 * are kept in storage ({@code name}, {@code level}, {@code bass}, {@code vol}). The manifest declares
 * {@code ui.slot.statusbar}, which toasts need, and the 24-px item icon {@code plug-24}. Toasts show the built-in plug
 * (status icons are at most 20 px, so plug-24 cannot be a toast icon).
 */
public class InputsDemoPlugin extends MibPlugin {

    private static final String PAGE_ID = "inputs";
    private static final String[] COLOURS = {"red", "Red", "green", "Green", "blue", "Blue"};
    private static final String[] LANGUAGES = {"English", "Deutsch", "Français"};
    private static final String[] LEVELS = {"Off", "Low", "Mid", "High"};
    private static final String CONFIRM_TEXT = "Are you sure? This long question checks how the confirm screen wraps"
            + " one line of text over all its three lines";
    private static final int MAX_NAME = 16;
    private static final long TOAST_DELAY_MS = 5000;

    private Log log;
    private Storage storage;
    private Scheduler scheduler;
    private Ui ui;
    private Page page;
    private Icon plug;
    private Icon toastIcon;

    private boolean wifi;
    private final boolean[] colours = new boolean[COLOURS.length / 2];
    private int lang;
    private int progress = 60;
    private boolean swapIsToggle = true;
    private boolean swapOn;
    private String name;
    /** The speller's changed text, not yet saved; null for none. */
    private String pending;
    private int level;
    private int bass;
    private int vol;
    private String last = "<none>";

    @Override
    protected void onEnable(PluginContext ctx) {
        log = ctx.log();
        storage = ctx.storage();
        scheduler = ctx.scheduler();
        ui = ctx.ui();
        plug = ui.icon("plug-24");
        toastIcon = ui.stockIcon(StockIcons.PLUG);
        name = storage.get("name", "");
        level = stored("level", 0, 0, LEVELS.length - 1, 1);
        bass = stored("bass", 0, -6, 6, 1);
        vol = stored("vol", 0, 0, 10, 2);
        page = ui.page(spec(), new Listener<UiAction>() {
            public void on(UiAction a) {
                onAction(a);
            }
        });
    }

    private void onAction(UiAction a) {
        log.info("action " + a);
        last = a.toString();
        String id = a.itemId();
        Boolean on = a.requestedOn();
        Integer value = a.value();
        if ("wifi".equals(id) && on != null) {
            wifi = on.booleanValue();
        } else if (colour(id) >= 0 && on != null) {
            colours[colour(id)] = on.booleanValue();
        } else if ("lang".equals(id) && value != null) {
            lang = value.intValue();
        } else if ("prog-next".equals(id)) {
            progress = (progress + 20) % 120; // 60, 80, 100, 0, 20, ...
        } else if ("swap".equals(id)) {
            swapIsToggle = !swapIsToggle;
        } else if ("swapme".equals(id) && on != null) {
            swapOn = on.booleanValue();
        } else if ("confirm".equals(id)) {
            ui.confirm("Confirm", CONFIRM_TEXT, null, null, answer());
        } else if ("confirm2".equals(id)) {
            ui.confirm("Confirm", "First line\nSecond line", null, null, answer());
        } else if ("toast".equals(id)) {
            ui.toast("Saved", toastIcon);
        } else if ("toast5".equals(id)) {
            scheduler.after(TOAST_DELAY_MS, new Runnable() {
                public void run() {
                    ui.toast("Saved", toastIcon);
                }
            });
        } else if ("name".equals(id)) {
            ui.speller("Name", shownName(), MAX_NAME, new Listener<UiAction>() {
                public void on(UiAction done) {
                    onSpellerDone(done);
                }
            });
        } else if ("save".equals(id) && pending != null) {
            name = pending;
            pending = null;
            storage.put("name", name);
            ui.toast("Name saved", null);
        } else if ("level".equals(id) && value != null) {
            level = value.intValue();
            storage.put("level", String.valueOf(level));
        } else if ("bass".equals(id) && value != null) {
            bass = value.intValue();
            storage.put("bass", String.valueOf(bass));
        } else if ("vol".equals(id) && value != null) {
            vol = value.intValue();
            storage.put("vol", String.valueOf(vol));
        }
        page.update(spec());
    }

    /** A confirm's answer is an action like any other: logged and shown on the last line. */
    private Listener<UiAction> answer() {
        return new Listener<UiAction>() {
            public void on(UiAction a) {
                onAction(a);
            }
        };
    }

    private void onSpellerDone(UiAction done) {
        String text = done.text();
        if (text != null) {
            pending = text.equals(name) ? null : text;
        }
        onAction(done);
    }

    private String shownName() {
        return pending != null ? pending : name;
    }

    private PageSpec spec() {
        PageSpec s = PageSpec.of(PAGE_ID, "Inputs").toggle("wifi", "Wi-Fi", wifi);
        for (int i = 0; i < colours.length; i++) {
            s = s.toggle(COLOURS[2 * i], COLOURS[2 * i + 1], colours[i]);
        }
        s = s.toggle("locked", "Locked", false).enabled("locked", false)
                .choice("lang", "Language", LANGUAGES, lang)
                .item("ver", "Version", "1.0.0")
                .item("plug", "Plug", null, plug)
                .item("plugv", "Plug value", "on", plug)
                .progress("prog", "Progress", progress)
                .item("prog-next", "Progress +20", null)
                .item("swap", "Swap kind", null);
        s = swapIsToggle ? s.toggle("swapme", "Swap me", swapOn) : s.item("swapme", "Swap me", "item");
        s = s.item("confirm", "Confirm", null)
                .item("confirm2", "Confirm 2 lines", null)
                .item("toast", "Toast", null)
                .item("toast5", "Toast in 5 s", null)
                .item("name", "Name", shownName());
        if (pending != null) {
            s = s.item("save", "Save name", null);
        }
        return s.dropdown("level", "Level", LEVELS, level)
                .slider("bass", "Bass", -6, 6, 1, bass, true)
                .slider("vol", "Volume", 0, 10, 2, vol, false)
                .info("Last: " + last);
    }

    private static int colour(String id) {
        for (int i = 0; i < COLOURS.length; i += 2) {
            if (COLOURS[i].equals(id)) {
                return i / 2;
            }
        }
        return -1;
    }

    /** A stored level in min..max on the step grid, else {@code def}. */
    private int stored(String key, int def, int min, int max, int step) {
        try {
            int v = Integer.parseInt(storage.get(key, String.valueOf(def)));
            return v >= min && v <= max && (v - min) % step == 0 ? v : def;
        } catch (NumberFormatException e) {
            return def;
        }
    }
}
