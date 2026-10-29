package sample.bikerack

import mibsdk.api.IgnitionChanged
import mibsdk.api.MibPlugin
import mibsdk.api.OverrideConflictException
import mibsdk.api.OverrideScope
import mibsdk.api.Page
import mibsdk.api.PageSpec
import mibsdk.api.ParkingSoundChanged
import mibsdk.api.PluginContext
import mibsdk.api.SoundDirections
import mibsdk.api.StatusItem
import mibsdk.api.Storage
import mibsdk.api.Subscription
import mibsdk.api.UiAction
import mibsdk.api.VehicleOverride

/**
 * Silences the rear parking-sensor tones while a bike rack is fitted. The toggle is storage key `active`.
 * The runtime's override ledger restores the prior value (on ignition off, disable or quarantine), so this
 * plugin never writes the old value back itself.
 *
 * The arm state lives in memory ([armed], read from storage once in `onEnable`); every mute decision reads the
 * flag, never storage. Ignition off disarms in memory first, then removes the key. It removes it rather than
 * writing "false": a removal always works, while "false" is one byte longer than "true" and, at the storage cap
 * (the estimate includes a fixed per-entry header), the put would throw and leave the rack armed.
 * A KL15 blip re-arms the rack only within [REARM_WINDOW_MS] of the ignition off, measured on the runtime Clock.
 *
 * Residual: the disarm reaches disk within the storage debounce (2 s). A power cut inside that window re-mutes
 * on the next boot, where the runtime's override indicator shows it.
 *
 * UI: page `rack` (Plugins menu, plus a Car row) with the toggle, and a `RACK` status item while the mute is held.
 * Toggle presses are debounced [TOGGLE_DEBOUNCE_MS]; the last requested state wins. On takes the re-arm path, off the
 * ignition-off disarm path but opens no re-arm window. At ignition off a pending off press is applied at once (no
 * re-arm window, so a KL15 blip cannot override the user's off); a pending on press is dropped.
 * An on press while already armed changes nothing: if the mute was lost to an [OverrideConflictException] (another
 * plugin holds the parking override), toggle off and on again to retry it.
 *
 * Icons: the manifest (api 3) declares `bike` (72x72, the Car row) and `bike-20` (20x20, the status item), white
 * bicycles from `tools/make_builtin_icons.py --plugin-bike-rack`, both tinted. Without icon support, or with the
 * package's icons not installed, the row and the item show without them.
 *
 * Permissions: the manifest declares `ui.slot.carmenu` and `ui.slot.statusbar`. An install that was
 * granted only `vehicle.read,vehicle.parking.write` must be re-granted, or the runtime blocks the plugin:
 * `mibsdk-dev grant bike-rack vehicle.read,vehicle.parking.write,ui.slot.carmenu,ui.slot.statusbar`.
 */
class BikeRackPlugin : MibPlugin() {

    @Volatile private var armed = false
    @Volatile private var waiting: Subscription? = null
    @Volatile private var held: VehicleOverride? = null
    @Volatile private var window: Subscription? = null
    @Volatile private var windowToken: Any? = null
    @Volatile private var page: Page? = null
    @Volatile private var statusItem: StatusItem? = null
    @Volatile private var pendingToggle: Subscription? = null
    @Volatile private var requestedOn = false

    override fun onEnable(ctx: PluginContext) {
        val storage = ctx.storage()
        armed = storage.get(ACTIVE, "false") == "true"
        val rack = ctx.ui().page(spec()) { a -> onAction(ctx, storage, a) }
        page = rack
        // the manifest declares both slot permissions (all-or-nothing: a running plugin holds them); without a
        // slot, or with the SDK UI off, these handles are inert and the mute below works the same
        ctx.ui().carMenuRow("Bike rack", rack, ctx.ui().icon("bike"))
        statusItem = ctx.ui().statusItem(statusText(), ctx.ui().icon("bike-20"))
        ctx.events().on(IgnitionChanged::class.java) { e ->
            if (!e.isOn) ignitionOff(ctx, storage) else ignitionOn(ctx, storage)
            refresh()
        }
        // the ledger can end or reapply the mute on its own; keep the page and status item in step
        ctx.events().on(ParkingSoundChanged::class.java) { refresh() }
        if (armed) muteWhenKnown(ctx)
        refresh()
    }

    private fun onAction(ctx: PluginContext, storage: Storage, a: UiAction) {
        val on = a.requestedOn() ?: return
        if (a.itemId() != ACTIVE) return
        ctx.log().info("toggle requested: ${if (on) "on" else "off"}")
        requestedOn = on
        pendingToggle?.cancel()
        pendingToggle = ctx.scheduler().after(TOGGLE_DEBOUNCE_MS) {
            pendingToggle = null
            if (requestedOn) arm(ctx, storage) else disarm(ctx, storage)
            refresh()
        }
    }

    /** The re-arm path, from the page. */
    private fun arm(ctx: PluginContext, storage: Storage) {
        closeWindow()
        if (armed) return
        try {
            storage.put(ACTIVE, "true")
        } catch (e: Exception) {
            ctx.log().warn("could not arm: ${e.message}; staying disarmed")
            return
        }
        armed = true
        muteWhenKnown(ctx)
    }

    /** The ignition-off disarm path, from the page: no re-arm window. */
    private fun disarm(ctx: PluginContext, storage: Storage) {
        armed = false
        waiting?.cancel()
        waiting = null
        held?.release()
        held = null
        closeWindow()
        try {
            storage.remove(ACTIVE)
        } catch (e: Exception) {
            ctx.log().warn("could not save the disarm: ${e.message}; the rack stays disarmed for this run")
        }
    }

    private fun closeWindow() {
        windowToken = null
        window?.cancel()
        window = null
    }

    private fun ignitionOff(ctx: PluginContext, storage: Storage) {
        // a press still in its debounce: an off is the user's explicit choice, so apply it now with no re-arm
        // window; an on was made for the drive that just ended, so drop it
        val pending = pendingToggle
        if (pending != null) {
            pending.cancel()
            pendingToggle = null
            if (!requestedOn) {
                disarm(ctx, storage)
                return
            }
        }
        if (!armed) return
        armed = false
        waiting?.cancel()
        waiting = null
        held?.release()
        held = null
        try {
            storage.remove(ACTIVE)
        } catch (e: Exception) {
            ctx.log().warn("could not save the disarm: ${e.message}; the rack stays disarmed for this run")
        }
        val token = Any()
        windowToken = token
        window = ctx.scheduler().after(REARM_WINDOW_MS) {
            if (windowToken === token) {
                windowToken = null
                window = null
                ctx.log().info("re-arm window closed")
            }
        }
    }

    private fun ignitionOn(ctx: PluginContext, storage: Storage) {
        if (armed) {
            // armed while parked: the ledger ends a mute taken with the ignition off at ignition on; take it again
            if (waiting == null && held?.isActive != true) muteWhenKnown(ctx)
            return
        }
        if (windowToken == null) return
        windowToken = null
        window?.cancel()
        window = null
        try {
            storage.put(ACTIVE, "true")
        } catch (e: Exception) {
            ctx.log().warn("could not re-arm: ${e.message}; staying disarmed")
            return
        }
        armed = true
        muteWhenKnown(ctx)
    }

    private fun muteWhenKnown(ctx: PluginContext) {
        val current = ctx.vehicle().parking().soundDirections()
        if (current != null) {
            mute(ctx, current)
        } else {
            ctx.log().info("waiting for parking value")
            waiting = ctx.events().on(ParkingSoundChanged::class.java) { e ->
                waiting?.cancel()
                waiting = null
                if (armed) mute(ctx, e.value())
                refresh()
            }
        }
    }

    private fun mute(ctx: PluginContext, current: SoundDirections) {
        try {
            held = ctx.vehicle().parking().overrideSoundDirections(current.withRear(false), OverrideScope.UNTIL_IGNITION_OFF)
            ctx.log().info("rear parking tones muted")
        } catch (e: OverrideConflictException) {
            ctx.log().warn("parking sounds already overridden: ${e.message}")
        }
    }

    private fun spec(): PageSpec {
        val tones = when {
            held?.isActive == true -> "muted"
            waiting != null -> "waiting for parking system"
            else -> "normal"
        }
        return PageSpec.of(PAGE_ID, "Bike rack")
            .toggle(ACTIVE, "Rack fitted (rear PDC tones off)", armed)
            .info("Rear tones: $tones")
            .info("Turns off at ignition off")
    }

    private fun statusText() = if (held?.isActive == true) "RACK" else null

    /** Every state change ends here. */
    private fun refresh() {
        page?.update(spec())
        statusItem?.set(statusText())
    }

    private companion object {
        const val ACTIVE = "active"
        const val REARM_WINDOW_MS = 5000L
        const val TOGGLE_DEBOUNCE_MS = 500L
        const val PAGE_ID = "rack"
    }
}
