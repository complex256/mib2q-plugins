package sample.bikerack

import java.io.File
import java.util.Properties
import mibsdk.adapter.fake.FakePlatform
import mibsdk.adapter.fake.FakeUi
import mibsdk.api.Features
import mibsdk.api.SoundDirections
import mibsdk.platform.HookTable
import mibsdk.platform.ParkingIo
import mibsdk.platform.PlatformAdapter
import mibsdk.platform.UiInput
import mibsdk.platform.UiTable
import mibsdk.runtime.Clock
import mibsdk.runtime.PluginState
import mibsdk.runtime.RingLog
import mibsdk.runtime.RuntimeConfig
import mibsdk.runtime.SdkRuntime
import org.junit.jupiter.api.AfterEach
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.io.TempDir

class BikeRackPluginTest {

    @TempDir
    lateinit var tmp: File

    private val f1r1l1x1 = SoundDirections(true, true, true, true)
    private val f1r0l1x1 = SoundDirections(true, false, true, true)

    private val now = object : Clock {
        @Volatile var ms = 0L
        override fun uptimeMs() = ms
    }
    private val fake = FakePlatform("HMI_TEST", setOf(Features.IGNITION, Features.PARKING, Features.HOOK_PARKING,
        Features.UI_PAGES, Features.SLOT_CAR_MENU, Features.SLOT_STATUS_BAR))
    private val table = UiTable(64)
    private val ui = FakeUi(table)

    /** The fake (its ECU starts at F1R1L1X1), except that the ECU's parking value can be hidden until "first read". */
    @Volatile private var ecuKnown = true
    private val platform = object : PlatformAdapter by fake {
        override fun parking() = object : ParkingIo {
            override fun current() = if (ecuKnown) fake.parking().current() else null
            override fun apply(value: SoundDirections) = fake.parking().apply(value)
        }
    }

    private var rt: SdkRuntime? = null
    private lateinit var base: File

    @AfterEach
    fun tearDown() {
        rt?.stop()
    }

    private fun start(active: Boolean?, extraProps: String = "", startDelayMs: Long = 0) {
        base = File(tmp, "mibsdk")
        File(base, "plugins").mkdirs()
        File(System.getProperty("mibsdk.pluginJar")).copyTo(File(base, "plugins/bike-rack.jar"))
        File(base, "sdk.properties").writeText(
            "plugin.start.delay.ms=$startDelayMs\nheap.low.bytes=0\nui.publish.min.ms=0\nlog.file=${File(tmp, "mibsdk.log").path}\n$extraProps")
        File(base, "grants.properties").writeText("bike-rack=vehicle.read,vehicle.parking.write,ui.slot.carmenu,ui.slot.statusbar\n")
        installIcons()
        if (active != null) {
            File(FakePlatform.roots(tmp).persist(), "data").mkdirs()
            File(FakePlatform.roots(tmp).persist(), "data/bike-rack.properties").writeText("active=$active\n")
        }
        rt = SdkRuntime(RuntimeConfig.load(FakePlatform.roots(tmp)), platform, now, HookTable(64), table, RingLog(500)).also {
            it.start()
            if (startDelayMs > 0) return@also
            assertEquals(PluginState.ENABLED, it.statuses().single().state(), it.log().snapshot().toString())
        }
    }

    /** What mibpkg installs for the package: index.properties and each declared PNG under its sha16 name. */
    private fun installIcons() {
        val dir = File(FakePlatform.roots(tmp).install(), "icons/bike-rack")
        dir.mkdirs()
        File(dir, "index.properties").writeText("bike=$BIG_SHA,72,72\nbike-20=$SMALL_SHA,20,20\n")
        File("src/main/icons/bike.png").copyTo(File(dir, "$BIG_SHA.png"))
        File("src/main/icons/bike-20.png").copyTo(File(dir, "$SMALL_SHA.png"))
    }

    private fun awaitApplied(size: Int, last: SoundDirections) {
        for (i in 0 until 500) {
            if (fake.appliedLog().size >= size) break
            Thread.sleep(10)
        }
        assertEquals(size, fake.appliedLog().size, "${fake.appliedLog()}\n${rt?.log()?.snapshot()}")
        assertEquals(last, fake.appliedLog()[size - 1])
    }

    private fun assertNoMoreThan(size: Int) {
        Thread.sleep(200)
        assertEquals(size, fake.appliedLog().size, fake.appliedLog().toString())
    }

    private fun awaitWindowClosed(count: Int) {
        for (i in 0 until 500) {
            if (windowClosedCount() >= count) break
            Thread.sleep(10)
        }
        assertEquals(count, windowClosedCount(), rt?.log()?.snapshot().toString())
    }

    private fun windowClosedCount() = rt!!.log().snapshot().count { it.toString().contains("re-arm window closed") }

    private fun storedActive(): String? {
        rt!!.stop() // flushes storage
        val p = Properties()
        File(FakePlatform.roots(tmp).persist(), "data/bike-rack.properties").inputStream().use { p.load(it) }
        return p.getProperty("active")
    }

    @Test
    fun activeOnEnableMutesRearOnly() {
        start(true)
        awaitApplied(1, f1r0l1x1)
    }

    @Test
    fun uiOffStillMutesAndDisarms() {
        // UI off (wrap-loop latch, ui.enabled=false, count or init failure): no UI features, inert UI handles
        fake.setFeatures(setOf(Features.IGNITION, Features.PARKING, Features.HOOK_PARKING))
        start(true)
        awaitApplied(1, f1r0l1x1)
        assertFalse(table.snapshot().pages().containsKey("bike-rack/rack"), table.snapshot().pages().keys.toString())
        fake.setIgnition(false)
        awaitApplied(2, f1r1l1x1)
        assertEquals(null, storedActive())
    }

    @Test
    fun inactiveDoesNothing() {
        start(false)
        assertNoMoreThan(0)
        assertTrue(rt!!.activeOverrides().isEmpty())
    }

    @Test
    fun ignitionOffRestoresAndClearsToggle() {
        start(true)
        awaitApplied(1, f1r0l1x1)
        fake.setIgnition(false)
        awaitApplied(2, f1r1l1x1)
        assertEquals(null, storedActive())
    }

    @Test
    fun waitsForFirstValueWhenUnknown() {
        ecuKnown = false
        start(true)
        assertNoMoreThan(0)
        ecuKnown = true
        fake.ecuSetSoundDirections(f1r1l1x1)
        awaitApplied(1, f1r0l1x1)
    }

    @Test
    fun ignitionOffWhileWaitingNeverMutes() {
        ecuKnown = false
        start(true)
        fake.setIgnition(false)
        ecuKnown = true
        fake.ecuSetSoundDirections(f1r1l1x1)
        assertNoMoreThan(0)
        assertTrue(rt!!.activeOverrides().isEmpty())
        assertEquals(null, storedActive())
    }

    @Test
    fun ecuFlipsBackGetsReapplied() {
        start(true)
        awaitApplied(1, f1r0l1x1)
        fake.ecuSetSoundDirections(f1r1l1x1)
        now.ms += 1000
        awaitApplied(2, f1r0l1x1)
    }

    @Test
    fun disablingPluginRestores() {
        start(true)
        awaitApplied(1, f1r0l1x1)
        rt!!.stop()
        awaitApplied(2, f1r1l1x1)
    }

    @Test
    fun ignitionOffWhileWaitingAtTheStorageCapNeverMutes() {
        ecuKnown = false
        // entryBytes("active","true") estimates 12; "false" would be 13 and throw
        start(true, "storage.plugin.max.bytes=12\n")
        fake.setIgnition(false)
        ecuKnown = true
        fake.ecuSetSoundDirections(f1r1l1x1)
        assertNoMoreThan(0)
        assertTrue(rt!!.activeOverrides().isEmpty())
        assertEquals(null, storedActive())
    }

    @Test
    fun armedWhileParkedMutesAgainAtIgnitionOn() {
        start(true, startDelayMs = 30000)
        fake.setIgnition(false) // KL15 is off before the plugin loads (enable or reload while parked)
        now.ms += 30000
        awaitApplied(1, f1r0l1x1) // muted while off: the ledger ends that override at ignition on
        fake.setIgnition(true)
        // the ledger ends the off-mute; the armed rack takes the mute again for this drive (the restore write
        // may or may not happen first, depending on which runs first)
        awaitHeld { it.size == 1 && !it[0].restorePending() && fake.ecuSoundDirections() == f1r0l1x1 }
        assertNoMoreThan(fake.appliedLog().size)
        assertEquals(f1r0l1x1, fake.ecuSoundDirections())
        assertEquals("bike-rack", rt!!.activeOverrides().single().pluginId())

        fake.setIgnition(false) // the drive ends: the rear tones come back
        awaitHeld { it.isEmpty() && fake.ecuSoundDirections() == f1r1l1x1 }
        assertNoMoreThan(fake.appliedLog().size)
        assertEquals(f1r1l1x1, fake.ecuSoundDirections())
        assertEquals(null, storedActive())
    }

    private fun awaitHeld(done: (List<mibsdk.runtime.ActiveOverride>) -> Boolean) {
        for (i in 0 until 500) {
            if (done(rt!!.activeOverrides())) return
            Thread.sleep(10)
        }
        throw AssertionError("${rt!!.activeOverrides()} ecu=${fake.ecuSoundDirections()} " +
            "applied=${fake.appliedLog()}\n${rt!!.log().snapshot()}")
    }

    @Test
    fun ignitionBlipWithinFiveSecondsReArmsAndMutesAgain() {
        start(true)
        awaitApplied(1, f1r0l1x1)
        fake.setIgnition(false)
        awaitApplied(2, f1r1l1x1)
        now.ms += 3000
        fake.setIgnition(true)
        awaitApplied(3, f1r0l1x1)
        assertEquals("true", storedActive())
    }

    @Test
    fun ignitionOnAfterTheWindowStaysDisarmed() {
        start(true)
        awaitApplied(1, f1r0l1x1)
        fake.setIgnition(false)
        awaitApplied(2, f1r1l1x1)
        now.ms += 5000
        awaitWindowClosed(1)
        fake.setIgnition(true)
        assertNoMoreThan(2)
        assertEquals(null, storedActive())
    }

    @Test
    fun bouncingIgnitionOffEndsDisarmedAndUnmuted() {
        start(true)
        awaitApplied(1, f1r0l1x1)
        fake.setIgnition(false)
        awaitApplied(2, f1r1l1x1)
        now.ms += 1000
        fake.setIgnition(true)
        awaitApplied(3, f1r0l1x1)
        fake.setIgnition(false)
        awaitApplied(4, f1r1l1x1)
        now.ms += 1000
        fake.setIgnition(false)
        now.ms += 5000
        awaitWindowClosed(1)
        fake.setIgnition(true)
        assertNoMoreThan(4)
        assertEquals(null, storedActive())
    }

    // ---- UI (the toggle drives the same arm/disarm paths as above; M6 itself is covered by the cases above) ----

    private val page = "bike-rack/rack"

    private companion object {
        const val BIG_SHA = "0123456789abcdef"
        const val SMALL_SHA = "fedcba9876543210"
    }

    private fun await(what: String, done: () -> Boolean) {
        for (i in 0 until 500) {
            if (done()) return
            Thread.sleep(10)
        }
        throw AssertionError("never $what: page=${ui.render(page)} status=${ui.statusLine()} " +
            "applied=${fake.appliedLog()}\n${rt?.log()?.snapshot()}")
    }

    private fun logCount(text: String) = rt!!.log().snapshot().count { it.toString().contains(text) }

    /** One toggle press as the HMI sends it; returns once the plugin has taken the request (not yet applied). */
    private fun press(on: Boolean) {
        val before = logCount("toggle requested")
        table.input(UiInput(UiInput.TOGGLE_REQUESTED, page, "active", on))
        await("toggle request taken") { logCount("toggle requested") > before }
    }

    /** Ends the 500 ms debounce. */
    private fun settle() {
        now.ms += 500
    }

    private fun toggleLine() = ui.render(page).firstOrNull() ?: ""
    private fun status() = ui.statusLine() ?: ""

    // ---- icons (api 3): the Car row and the RACK status item carry the bicycle, tinted ----

    @Test
    fun rowAndStatusItemCarryTheBikeIcons() {
        fake.setFeatures(setOf(Features.IGNITION, Features.PARKING, Features.HOOK_PARKING, Features.UI_PAGES,
            Features.SLOT_CAR_MENU, Features.SLOT_STATUS_BAR, Features.UI_ICONS))
        start(true)
        awaitApplied(1, f1r0l1x1)
        await("row with the bike icon") { ui.carRows().contains("[png:$BIG_SHA] Bike rack") }
        await("RACK with the tinted bike-20") { ui.statusParts().contains("[png:$SMALL_SHA] RACK") }
    }

    @Test
    fun withoutIconSupportTheRowAndItemShowPlain() {
        start(true)
        awaitApplied(1, f1r0l1x1)
        await("plain row") { ui.carRows().contains("Bike rack") }
        await("plain RACK") { ui.statusParts().contains("RACK") }
    }

    @Test
    fun manifestDeclaresTheIconsAtApiThree() {
        val m = File("src/main/resources/plugin.json").readText()
        assertTrue(m.contains("\"api\":3"), m)
        assertTrue(m.contains("\"icons\":[\"bike\",\"bike-20\"]"), m)
    }

    @Test
    fun toggleOnArmsMutesRearAndShowsStatusItem() {
        start(null)
        await("page") { toggleLine() == "[off] Rack fitted (rear PDC tones off)" }
        assertTrue(ui.carRows().contains("Bike rack"), ui.carRows().toString())
        press(true)
        settle()
        awaitApplied(1, f1r0l1x1)
        await("RACK status item") { status().contains("RACK") }
        await("page on") { toggleLine() == "[on] Rack fitted (rear PDC tones off)" }
        assertTrue(ui.render(page).contains("Rear tones: muted"), ui.render(page).toString())
        assertTrue(ui.render(page).contains("Turns off at ignition off"), ui.render(page).toString())
        assertEquals("true", storedActive())
    }

    @Test
    fun toggleOffDisarmsRestoresAndHidesItem() {
        start(true)
        awaitApplied(1, f1r0l1x1)
        await("RACK status item") { status().contains("RACK") }
        press(false)
        settle()
        awaitApplied(2, f1r1l1x1)
        await("RACK hidden") { !status().contains("RACK") }
        await("page off") { toggleLine() == "[off] Rack fitted (rear PDC tones off)" }
        assertTrue(ui.render(page).contains("Rear tones: normal"), ui.render(page).toString())
        assertEquals(null, storedActive())
    }

    @Test
    fun toggleOffOpensNoReArmWindow() {
        start(true)
        awaitApplied(1, f1r0l1x1)
        press(false)
        settle()
        awaitApplied(2, f1r1l1x1)
        fake.setIgnition(false)
        now.ms += 1000
        fake.setIgnition(true)
        assertNoMoreThan(2)
        assertEquals(f1r1l1x1, fake.ecuSoundDirections())
        assertEquals(null, storedActive())
    }

    @Test
    fun doublePressEndsInLastState() {
        start(null)
        await("page") { toggleLine().isNotEmpty() }
        press(true)
        press(false)
        press(true)
        settle()
        awaitApplied(1, f1r0l1x1)
        await("page on") { toggleLine() == "[on] Rack fitted (rear PDC tones off)" }
        now.ms += 1000
        assertNoMoreThan(1)
        assertEquals("bike-rack", rt!!.activeOverrides().single().pluginId())
    }

    @Test
    fun toggleOnAtStorageCapStaysOff() {
        // entryBytes("active","true") estimates 12: the put throws
        start(null, "storage.plugin.max.bytes=11\n")
        await("page") { toggleLine().isNotEmpty() }
        press(true)
        settle()
        await("arm failure logged") { logCount("could not arm") == 1 }
        assertNoMoreThan(0)
        assertTrue(rt!!.activeOverrides().isEmpty())
        assertEquals("[off] Rack fitted (rear PDC tones off)", toggleLine())
        assertFalse(status().contains("RACK"), status())
    }

    @Test
    fun pageShowsWaitingWithoutParkingValue() {
        ecuKnown = false
        start(true)
        await("waiting line") { ui.render(page).contains("Rear tones: waiting for parking system") }
        assertEquals("[on] Rack fitted (rear PDC tones off)", toggleLine())
        assertFalse(status().contains("RACK"), status())
        ecuKnown = true
        fake.ecuSetSoundDirections(f1r1l1x1)
        awaitApplied(1, f1r0l1x1)
        await("muted line") { ui.render(page).contains("Rear tones: muted") }
        await("RACK status item") { status().contains("RACK") }
    }

    @Test
    fun ignitionOffTurnsPageToggleOff() {
        start(true)
        awaitApplied(1, f1r0l1x1)
        await("page on") { toggleLine() == "[on] Rack fitted (rear PDC tones off)" }
        fake.setIgnition(false)
        awaitApplied(2, f1r1l1x1)
        await("page off") { toggleLine() == "[off] Rack fitted (rear PDC tones off)" }
        await("RACK hidden") { !status().contains("RACK") }
        assertTrue(ui.render(page).contains("Rear tones: normal"), ui.render(page).toString())
    }

    @Test
    fun pendingOffAtIgnitionOffDisarmsWithoutReArmWindow() {
        start(true)
        awaitApplied(1, f1r0l1x1)
        press(false) // still inside its debounce at ignition off
        fake.setIgnition(false)
        awaitApplied(2, f1r1l1x1)
        now.ms += 1000
        fake.setIgnition(true)
        now.ms += 500
        assertNoMoreThan(2)
        assertEquals(f1r1l1x1, fake.ecuSoundDirections())
        assertEquals(null, storedActive())
    }
}
