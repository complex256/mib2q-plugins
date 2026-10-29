package sample.inputsdemo;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.File;
import java.io.IOException;
import java.util.Arrays;
import java.util.List;
import java.util.Properties;
import mibsdk.adapter.fake.FakeUi;
import mibsdk.api.Features;
import mibsdk.test.PluginTestHarness;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

class InputsDemoPluginTest {

    @TempDir
    File tmp;

    private PluginTestHarness h;
    /** What the plugin stored on an earlier run: set before start. */
    private final Properties stored = new Properties();
    private FakeUi ui;
    private String page;
    private String lang;
    private String plug;

    @AfterEach
    void tearDown() {
        if (h != null) {
            h.close();
        }
    }

    /** Every feature the demo uses (F27). */
    private void start() {
        start(Features.UI_PAGES, Features.UI_INPUTS, Features.UI_ICONS, Features.SLOT_STATUS_BAR);
    }

    private void start(String... features) {
        h = PluginTestHarness.builder(tmp).features(features).storedValues(stored).start();
        ui = h.ui();
        page = h.pageKey("inputs");
        lang = page + "#lang";
        plug = h.iconSha("plug-24");
        h.await(() -> !ui.render(page).isEmpty(), "page registered");
    }

    private static final String CONFIRM_TEXT = "Are you sure? This long question checks how the confirm screen wraps"
            + " one line of text over all its three lines";

    /** Opens the page from the Plugins menu, as the user does: confirm and speller need it on screen. */
    private void openPage() {
        h.openPage("inputs");
    }

    /** The rendered line of the entry labelled {@code label}; null for none. */
    private String line(String label) {
        for (String l : ui.render(page)) {
            String t = l.endsWith(" [disabled]") ? l.substring(0, l.length() - " [disabled]".length()) : l;
            if (t.equals(label) || t.endsWith("] " + label) || t.startsWith(label + ":")) {
                return l;
            }
        }
        return null;
    }

    private void awaitLine(String exact) {
        h.await(() -> ui.render(page).contains(exact), "line '" + exact + "'");
    }

    private String last() {
        List<String> lines = ui.render(page);
        return lines.get(lines.size() - 1);
    }

    @Test
    void pageRendersEveryInput() throws IOException {
        start();
        assertEquals(Arrays.asList(
                "[off] Wi-Fi",
                "[off] Red",
                "[off] Green",
                "[off] Blue",
                "[off] Locked [disabled]",
                "Language: English >",
                "Version: 1.0.1",
                "[png:" + plug + "] Plug",
                "[png:" + plug + "] Plug value: on",
                "Progress: 60%",
                "Progress +20",
                "Swap kind",
                "[off] Swap me",
                "Confirm",
                "Confirm 2 lines",
                "Toast",
                "Toast in 5 s",
                "Name",
                "Level: Off v",
                "Bass: 0 [-6..6]",
                "Volume: 0 [0..10]",
                "Last: <none>"), ui.render(page));
        assertEquals(Arrays.asList("(*) English", "( ) Deutsch", "( ) Français"),
                ui.render(lang));
    }

    @Test
    void withoutInputsRendersTextLines() throws IOException {
        start(Features.UI_PAGES, Features.UI_ICONS, Features.SLOT_STATUS_BAR);
        List<String> lines = ui.render(page);
        assertEquals(22, lines.size(), lines.toString());
        for (String text : new String[] {"Level: Off", "Bass: 0", "Volume: 0", "Progress: 60%", "Language: English"}) {
            assertTrue(lines.contains(text), text + " in " + lines);
        }
        assertTrue(lines.contains("[off] Locked [disabled]"), "the toggle stays a disabled list row: " + lines);
    }

    @Test
    void toggleAndMultiChoice() throws IOException {
        start();
        ui.toggle(page, "wifi");
        awaitLine("[on] Wi-Fi");
        ui.toggle(page, "red");
        ui.toggle(page, "blue");
        awaitLine("[on] Blue");
        assertEquals("[on] Red", line("Red"));
        assertEquals("[off] Green", line("Green"), "multi-choice: each colour on its own");
        ui.toggle(page, "red");
        awaitLine("[off] Red");
        assertEquals("[on] Blue", line("Blue"));
        assertTrue(last().startsWith("Last: UiAction{page=inputs, item=red, requestedOn=false"), last());
        assertTrue(h.log().contains("UiAction{page=inputs, item=wifi, requestedOn=true"),
                "every action is logged");
    }

    @Test
    void lockedSendsNothing() throws IOException {
        start();
        ui.toggle(page, "locked");
        ui.toggle(page, "wifi"); // a later action proves the first one never arrived
        awaitLine("[on] Wi-Fi");
        assertEquals("[off] Locked [disabled]", line("Locked"));
        assertFalse(h.log().contains("item=locked"), h.log());
    }

    @Test
    void choicePicksDeutsch() throws IOException {
        start();
        ui.choose(page, "lang", 1);
        awaitLine("Language: Deutsch >");
        assertEquals(Arrays.asList("( ) English", "(*) Deutsch", "( ) Français"), ui.render(lang));
    }

    @Test
    void dropdownAndSlidersPersist() throws IOException {
        start();
        ui.pick(page, "level", 2);
        awaitLine("Level: Mid v");
        ui.turn(page, "bass", 9);
        awaitLine("Bass: 6 [-6..6]");
        ui.turn(page, "vol", 5);
        awaitLine("Volume: 4 [0..10]");
        Properties p = h.storedValues();
        assertEquals("2", p.getProperty("level"));
        assertEquals("6", p.getProperty("bass"));
        assertEquals("4", p.getProperty("vol"));
    }

    @Test
    void storedValuesAreRestored() throws IOException {
        stored.setProperty("name", "Al");
        stored.setProperty("level", "3");
        stored.setProperty("bass", "-2");
        stored.setProperty("vol", "8");
        start();
        assertEquals("Name: Al", line("Name"));
        assertEquals("Level: High v", line("Level"));
        assertEquals("Bass: -2 [-6..6]", line("Bass"));
        assertEquals("Volume: 8 [0..10]", line("Volume"));
    }

    @Test
    void progressNext() throws IOException {
        start();
        ui.select(page, "prog-next");
        awaitLine("Progress: 80%");
        ui.select(page, "prog-next");
        awaitLine("Progress: 100%");
        ui.select(page, "prog-next");
        awaitLine("Progress: 0%");
        ui.select(page, "prog-next");
        awaitLine("Progress: 20%");
    }

    @Test
    void swapKindAlternates() throws IOException {
        start();
        ui.toggle(page, "swapme");
        awaitLine("[on] Swap me");
        ui.select(page, "swap");
        awaitLine("Swap me: item");
        assertEquals(22, ui.render(page).size());
        ui.select(page, "swap");
        awaitLine("[on] Swap me");
    }

    @Test
    void confirmYesNo() throws IOException {
        start();
        openPage();
        assertEquals(110, CONFIRM_TEXT.length());
        assertEquals(-1, CONFIRM_TEXT.indexOf('\n'));
        ui.select(page, "confirm");
        h.await(() -> ("confirm:Confirm|" + CONFIRM_TEXT + "|Yes|No").equals(ui.dialog()), "the long confirm");
        ui.answer(1);
        h.await(() -> last().contains("value=1"), "the Yes answer: " + last());
        ui.select(page, "confirm2");
        h.await(() -> "confirm:Confirm|First line\nSecond line|Yes|No".equals(ui.dialog()), "the 2-line confirm");
        ui.leave(); // BACK is No
        h.await(() -> last().contains("value=0"), "the No answer: " + last());
        assertNull(ui.dialog());
    }

    @Test
    void toastAndDelayedToast() throws Exception {
        start();
        ui.select(page, "toast5");
        h.await(() -> last().contains("item=toast5"), "the toast5 action");
        Thread.sleep(200);
        assertNull(ui.toast(), "not before 5 s");
        h.advance(5000);
        h.await(() -> "[plug] Saved".equals(ui.toast()), "the delayed toast: " + ui.toast());
        long seq = ui.toastSeq();
        ui.select(page, "toast");
        h.await(() -> ui.toastSeq() > seq, "a second toast");
        assertEquals("[plug] Saved", ui.toast());
    }

    @Test
    void spellerSaveFlow() throws IOException {
        start();
        openPage();
        ui.select(page, "name");
        h.await(() -> "speller:Name|".equals(ui.dialog()), "the speller: " + ui.dialog());
        ui.type("BC");
        ui.leave(); // BACK is done
        awaitLine("Name: BC");
        List<String> lines = ui.render(page);
        assertEquals("Save name", lines.get(lines.indexOf("Name: BC") + 1), lines.toString());
        assertEquals(23, lines.size());
        ui.select(page, "save");
        h.await(() -> !ui.render(page).contains("Save name"), "the Save row removed");
        assertEquals("Name: BC", line("Name"));
        h.await(() -> "Name saved".equals(ui.toast()), "the saved toast: " + ui.toast());
        assertEquals("BC", h.storedValues().getProperty("name"));
    }

    @Test
    void spellerUnchangedAddsNoSaveRow() throws IOException {
        stored.setProperty("name", "Al");
        start();
        openPage();
        ui.select(page, "name");
        h.await(() -> "speller:Name|Al".equals(ui.dialog()), "the speller from the stored name: " + ui.dialog());
        ui.leave(); // nothing typed: the stored name comes back
        h.await(() -> last().contains("text=Al"), "the unchanged answer: " + last());
        assertFalse(ui.render(page).contains("Save name"), ui.render(page).toString());
        assertNull(ui.toast());
    }

    @Test
    void manifestIsApi5Version101() throws IOException {
        String json = manifest();
        assertTrue(json.contains("\"api\":5"), json);
        assertTrue(json.contains("\"version\":\"1.0.1\""), json);
        assertTrue(json.contains("\"permissions\":[\"ui.slot.statusbar\"]"), json);
        assertTrue(json.contains("\"icons\":[\"plug-24\"]"), json);
    }

    private static String manifest() throws IOException {
        try (java.util.zip.ZipFile z = new java.util.zip.ZipFile(System.getProperty("mibsdk.pluginJar"))) {
            java.io.InputStream in = z.getInputStream(z.getEntry("plugin.json"));
            return new String(in.readAllBytes(), "UTF-8");
        }
    }
}
