package local.bosecontrolcenter.app

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.bluetooth.BluetoothSocket
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.media.AudioAttributes
import android.media.AudioDeviceInfo
import android.media.AudioFocusRequest
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioTrack
import android.net.Uri
import android.os.Build
import android.os.Debug
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.util.Log
import android.webkit.WebView
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File
import java.io.IOException
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ExecutionException
import java.util.concurrent.Executors
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException
import java.util.zip.ZipInputStream

@InvokeArg
class OpenArgs {
    var address: String = ""
    /** A fixed RFCOMM channel; absent means "the Serial Port service". */
    var channel: Int? = null
}

@InvokeArg
class SendArgs {
    var data: String = ""
}

@InvokeArg
class RecvArgs {
    var timeoutMs: Long = 0
}

@InvokeArg
class OutputsArgs {
    var address: String? = null
}

@InvokeArg
class PlayArgs {
    /** 16-bit little-endian mono PCM, as hexadecimal. */
    var pcm: String = ""
    var sampleRate: Int = 24000
    /** The `AudioDeviceInfo` id to play through; absent lets Android route. */
    var deviceId: Int? = null
}

@InvokeArg
class KeepAliveArgs {
    var on: Boolean = false
}

@InvokeArg
class LogArgs {
    var message: String = ""
}

@InvokeArg
class VolumeArgs {
    /** A step of the media volume, counted from its lowest. */
    var level: Int = 0
}

@InvokeArg
class MuteArgs {
    var muted: Boolean = false
}

@InvokeArg
class NoteArgs {
    var what: String = ""
}

@InvokeArg
class WidgetNextArgs {
    var waitMs: Long = 0
}

@InvokeArg
class WidgetStateArgs {
    /** `connected`, `disconnected` or `simulated`. */
    var status: String = "disconnected"
    var name: String? = null
    /** The mode the headphones last reported: `quiet`, `aware` or `custom`. */
    var mode: String? = null
    var customName: String? = null
    /** The equalizer the headphones last reported, in whole decibels. */
    var bass: Int? = null
    var mid: Int? = null
    var treble: Int? = null
    /** A few words about the last attempt, if it did not simply succeed. */
    var note: String? = null
}

/**
 * Everything the native (Rust) layer needs from Android, and nothing else.
 *
 * The Rust side owns every decision: which device, which request, what a reply
 * means, what is announced. This class only carries bytes to and from the
 * headphones, reports what Android knows about paired devices and audio
 * outputs, and plays samples it is handed. It is called from Rust alone — the
 * web view has no permission to invoke it — so the rule that the interface
 * cannot send arbitrary Bluetooth bytes holds here as it does on the desktop.
 */
@TauriPlugin
class AuroraPlugin(private val activity: Activity) : Plugin(activity) {
    companion object {
        private const val TAG = "Aurora"
        private val SPP: UUID = UUID.fromString("00001101-0000-1000-8000-00805F9B34FB")
        private const val CONNECT_TIMEOUT_SECONDS = 8L
        private const val PERMISSION_REQUEST = 7001
        private const val BATTERY_CHANGED = "android.bluetooth.device.action.BATTERY_LEVEL_CHANGED"
        private const val BATTERY_EXTRA = "android.bluetooth.device.extra.BATTERY_LEVEL"
        /** Marks the end of the stream in [inbox]. Compared by identity. */
        private val CLOSED = ByteArray(0)
        private const val HEX = "0123456789abcdef"

        // Bytes cross to and from the native layer as hexadecimal text.
        fun toHex(bytes: ByteArray): String {
            val out = StringBuilder(bytes.size * 2)
            for (b in bytes) {
                val v = b.toInt() and 0xFF
                out.append(HEX[v ushr 4]).append(HEX[v and 0x0F])
            }
            return out.toString()
        }

        fun fromHex(text: String): ByteArray {
            require(text.length % 2 == 0) { "odd-length hexadecimal" }
            val out = ByteArray(text.length / 2)
            for (i in out.indices) {
                val hi = Character.digit(text[i * 2], 16)
                val lo = Character.digit(text[i * 2 + 1], 16)
                require(hi >= 0 && lo >= 0) { "not hexadecimal" }
                out[i] = ((hi shl 4) or lo).toByte()
            }
            return out
        }
    }

    // One conversation with the headphones at a time, in order.
    private val link = Executors.newSingleThreadExecutor()
    private val connecting = Executors.newCachedThreadPool()
    private val work = Executors.newCachedThreadPool()
    private val main = Handler(Looper.getMainLooper())

    private var socket: BluetoothSocket? = null
    private var inbox = LinkedBlockingQueue<ByteArray>()

    private var a2dp: BluetoothProfile? = null
    private var headset: BluetoothProfile? = null
    private var proxiesRequested = false
    private val batteries = ConcurrentHashMap<String, Int>()
    private var batteryReceiver: BroadcastReceiver? = null

    private class Playing(val track: AudioTrack, val frames: Int, val startedAt: Long, val millis: Long)

    @Volatile
    private var playing: Playing? = null
    private var focus: AudioFocusRequest? = null

    private val adapter: BluetoothAdapter?
        get() = (activity.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager)?.adapter

    private val audio: AudioManager
        get() = activity.getSystemService(Context.AUDIO_SERVICE) as AudioManager

    override fun load(webView: WebView) {
        requestMissingPermissions()
    }

    /** One line in Android's log, from the native layer. `adb logcat -s Aurora`. */
    @Command
    fun log(invoke: Invoke) {
        val args = invoke.parseArgs(LogArgs::class.java)
        Log.i(TAG, args.message)
        invoke.resolve()
    }

    /**
     * One line saying how much native memory the process holds, labelled with
     * what has just happened. The voice model is nearly all of it while it is
     * loaded, so this is how its coming and going can be seen from outside.
     */
    @Command
    fun noteMemory(invoke: Invoke) {
        val args = invoke.parseArgs(NoteArgs::class.java)
        val megabytes = Debug.getNativeHeapAllocatedSize() / (1024 * 1024)
        Log.i(TAG, "${args.what}: native heap $megabytes MB")
        invoke.resolve()
    }

    /**
     * What this phone calls itself. Its Bluetooth name is what the headphones
     * know it by; without leave to read that, the name in its settings; and
     * failing both, the model.
     */
    @SuppressLint("MissingPermission")
    @Command
    fun hostName(invoke: Invoke) {
        work.execute {
            val fromBluetooth = try {
                if (hasBluetooth()) adapter?.name else null
            } catch (e: Exception) {
                null
            }
            val fromSettings = try {
                Settings.Global.getString(activity.contentResolver, Settings.Global.DEVICE_NAME)
            } catch (e: Exception) {
                null
            }
            val name = listOf(fromBluetooth, fromSettings, Build.MODEL)
                .firstOrNull { !it.isNullOrBlank() }
            // Where it came from, not what it is.
            Log.i(
                TAG,
                "phone name taken from " + when {
                    !fromBluetooth.isNullOrBlank() -> "its Bluetooth name"
                    !fromSettings.isNullOrBlank() -> "its settings"
                    name != null -> "its model"
                    else -> "nowhere: the system gave none"
                }
            )
            val result = JSObject()
            if (name != null) result.put("name", name.trim())
            invoke.resolve(result)
        }
    }

    // --- the home-screen widget ----------------------------------------------

    /**
     * Waits for the next thing the widget wants: a mode by name, one band of
     * the equalizer one step up or down, or simply to be told the state.
     * Answers `none` when the wait runs out. The native
     * layer asks this in a loop for as long as it lives, and that it is
     * asking is how the widget knows there is a native layer to ask.
     */
    @Command
    fun widgetNext(invoke: Invoke) {
        val args = invoke.parseArgs(WidgetNextArgs::class.java)
        work.execute {
            val request = WidgetBus.next(args.waitMs.coerceIn(0, 60_000))
            val result = JSObject()
            when {
                request == null -> result.put("kind", "none")
                request.kind == "mode" -> {
                    Log.i(TAG, "widget: ${request.what} taken up")
                    result.put("kind", "mode").put("mode", request.what)
                }
                request.kind == "eq" -> {
                    Log.i(TAG, "widget: ${request.what} ${request.step} taken up")
                    result.put("kind", "eq").put("band", request.what).put("step", request.step)
                }
                else -> result.put("kind", "refresh")
            }
            invoke.resolve(result)
        }
    }

    /**
     * What the headphones now report, for the widget to show. Answers whether
     * a widget on a home screen has the equalizer on it.
     */
    @Command
    fun widgetState(invoke: Invoke) {
        val args = invoke.parseArgs(WidgetStateArgs::class.java)
        main.post {
            try {
                val showsEq = ModeWidget.report(
                    activity.applicationContext,
                    args.status, args.name, args.mode, args.customName,
                    args.bass, args.mid, args.treble, args.note
                )
                invoke.resolve(JSObject().put("showsEq", showsEq))
            } catch (e: Exception) {
                invoke.reject(e.message ?: "the widget could not be updated")
            }
        }
    }

    // --- permissions ---------------------------------------------------------

    private fun wanted(): Array<String> {
        val list = ArrayList<String>()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            list.add(Manifest.permission.BLUETOOTH_CONNECT)
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            list.add(Manifest.permission.POST_NOTIFICATIONS)
        }
        return list.toTypedArray()
    }

    private fun granted(permission: String) =
        ContextCompat.checkSelfPermission(activity, permission) == PackageManager.PERMISSION_GRANTED

    private fun hasBluetooth(): Boolean =
        Build.VERSION.SDK_INT < Build.VERSION_CODES.S || granted(Manifest.permission.BLUETOOTH_CONNECT)

    private fun requestMissingPermissions(): Boolean {
        val missing = wanted().filter { !granted(it) }
        if (missing.isEmpty()) return false
        ActivityCompat.requestPermissions(activity, missing.toTypedArray(), PERMISSION_REQUEST)
        return true
    }

    /**
     * Asks again for the Bluetooth permission. Once Android has stopped
     * showing the prompt (it does after two refusals) this opens the
     * application's page in Settings instead, where it can be granted.
     */
    @Command
    fun askPermissions(invoke: Invoke) {
        main.post {
            if (!hasBluetooth() && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                val canAsk = ActivityCompat.shouldShowRequestPermissionRationale(
                    activity, Manifest.permission.BLUETOOTH_CONNECT
                )
                if (canAsk) {
                    requestMissingPermissions()
                } else {
                    // Either never asked or permanently refused: asking is
                    // harmless in the first case and silent in the second, so
                    // do both.
                    requestMissingPermissions()
                    try {
                        activity.startActivity(
                            Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS)
                                .setData(Uri.fromParts("package", activity.packageName, null))
                        )
                    } catch (e: Exception) {
                        Log.w(TAG, "could not open application settings", e)
                    }
                }
            } else {
                requestMissingPermissions()
            }
            invoke.resolve(JSObject().put("bluetooth", hasBluetooth()))
        }
    }

    // --- what Android knows about paired devices -----------------------------

    @SuppressLint("MissingPermission")
    private fun ensureProxies() {
        if (proxiesRequested || !hasBluetooth()) return
        val adapter = adapter ?: return
        proxiesRequested = true
        val listener = object : BluetoothProfile.ServiceListener {
            override fun onServiceConnected(profile: Int, proxy: BluetoothProfile) {
                if (profile == BluetoothProfile.A2DP) a2dp = proxy
                if (profile == BluetoothProfile.HEADSET) headset = proxy
            }

            override fun onServiceDisconnected(profile: Int) {
                if (profile == BluetoothProfile.A2DP) a2dp = null
                if (profile == BluetoothProfile.HEADSET) headset = null
            }
        }
        try {
            adapter.getProfileProxy(activity.applicationContext, listener, BluetoothProfile.A2DP)
            adapter.getProfileProxy(activity.applicationContext, listener, BluetoothProfile.HEADSET)
        } catch (e: Exception) {
            Log.w(TAG, "profile proxies unavailable", e)
        }

        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                val device: BluetoothDevice? = if (Build.VERSION.SDK_INT >= 33) {
                    intent.getParcelableExtra(BluetoothDevice.EXTRA_DEVICE, BluetoothDevice::class.java)
                } else {
                    @Suppress("DEPRECATION") intent.getParcelableExtra(BluetoothDevice.EXTRA_DEVICE)
                }
                val level = intent.getIntExtra(BATTERY_EXTRA, -1)
                val address = device?.address ?: return
                if (level in 0..100) batteries[address] = level else batteries.remove(address)
            }
        }
        try {
            ContextCompat.registerReceiver(
                activity.applicationContext, receiver, IntentFilter(BATTERY_CHANGED),
                ContextCompat.RECEIVER_EXPORTED
            )
            batteryReceiver = receiver
        } catch (e: Exception) {
            Log.w(TAG, "battery broadcasts unavailable", e)
        }
    }

    @SuppressLint("MissingPermission")
    private fun isConnected(device: BluetoothDevice): Boolean {
        try {
            if (a2dp?.connectedDevices?.any { it.address == device.address } == true) return true
            if (headset?.connectedDevices?.any { it.address == device.address } == true) return true
        } catch (e: Exception) {
            // Fall through to the direct question.
        }
        return try {
            device.javaClass.getMethod("isConnected").invoke(device) as? Boolean ?: false
        } catch (e: Throwable) {
            false
        }
    }

    /** The level Android holds for the device, as the headphones reported it. */
    private fun batteryOf(device: BluetoothDevice): Int? {
        val direct = try {
            device.javaClass.getMethod("getBatteryLevel").invoke(device) as? Int
        } catch (e: Throwable) {
            null
        }
        if (direct != null && direct in 0..100) return direct
        return batteries[device.address]
    }

    @SuppressLint("MissingPermission")
    private fun nameOf(device: BluetoothDevice): String {
        val alias = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) device.alias else null
        return alias ?: device.name ?: "Unnamed device"
    }

    /** The radio, the permission, and every paired device. Sends nothing. */
    @SuppressLint("MissingPermission")
    @Command
    fun paired(invoke: Invoke) {
        work.execute {
            val result = JSObject()
            val adapter = adapter
            result.put("radioPresent", adapter != null)
            result.put("permission", hasBluetooth())
            val devices = JSArray()
            var enabled = false
            if (adapter != null && hasBluetooth()) {
                try {
                    enabled = adapter.isEnabled
                    ensureProxies()
                    for (device in adapter.bondedDevices) {
                        val entry = JSObject()
                        entry.put("address", device.address)
                        entry.put("name", nameOf(device))
                        entry.put("connected", isConnected(device))
                        batteryOf(device)?.let { entry.put("battery", it) }
                        val uuids = JSArray()
                        device.uuids?.forEach { uuids.put(it.uuid.toString()) }
                        entry.put("uuids", uuids)
                        entry.put(
                            "lowEnergyOnly",
                            device.type == BluetoothDevice.DEVICE_TYPE_LE
                        )
                        devices.put(entry)
                    }
                } catch (e: SecurityException) {
                    result.put("permission", false)
                }
            }
            result.put("radioEnabled", enabled)
            result.put("devices", devices)
            invoke.resolve(result)
        }
    }

    @Command
    fun openBluetoothSettings(invoke: Invoke) {
        try {
            activity.startActivity(Intent(Settings.ACTION_BLUETOOTH_SETTINGS))
            invoke.resolve()
        } catch (e: Exception) {
            invoke.reject("Could not open Bluetooth settings: ${e.message}")
        }
    }

    // --- the control channel -------------------------------------------------

    private fun closeSocket() {
        try {
            socket?.close()
        } catch (e: IOException) {
            // Already gone.
        }
        socket = null
    }

    /**
     * Opens an RFCOMM connection: to the Serial Port service, or to a fixed
     * channel. Never throws to the caller; the answer says what happened and
     * whether the device itself seemed to be out of reach.
     */
    @SuppressLint("MissingPermission")
    @Command
    fun rfcommOpen(invoke: Invoke) {
        val args = invoke.parseArgs(OpenArgs::class.java)
        link.execute {
            closeSocket()
            val result = JSObject()
            fun fail(message: String, unreachable: Boolean) {
                result.put("ok", false).put("message", message).put("unreachable", unreachable)
                invoke.resolve(result)
            }

            val adapter = adapter
            if (adapter == null || !hasBluetooth()) {
                fail("Bluetooth is not available to this application", true)
                return@execute
            }
            val device = try {
                adapter.getRemoteDevice(args.address)
            } catch (e: IllegalArgumentException) {
                fail("not a Bluetooth address", true)
                return@execute
            }

            val candidate = try {
                val channel = args.channel
                if (channel == null) {
                    device.createRfcommSocketToServiceRecord(SPP)
                } else {
                    device.javaClass
                        .getMethod("createRfcommSocket", Int::class.javaPrimitiveType)
                        .invoke(device, channel) as BluetoothSocket
                }
            } catch (e: Throwable) {
                fail("this route cannot be opened on this version of Android (${e.javaClass.simpleName})", false)
                return@execute
            }

            try {
                adapter.cancelDiscovery()
            } catch (e: Exception) {
                // Not discovering, or not allowed to ask: neither matters here.
            }

            val attempt = connecting.submit { candidate.connect() }
            try {
                attempt.get(CONNECT_TIMEOUT_SECONDS, TimeUnit.SECONDS)
            } catch (e: TimeoutException) {
                try { candidate.close() } catch (io: IOException) {}
                fail("this route did not answer in time", !isConnected(device))
                return@execute
            } catch (e: ExecutionException) {
                try { candidate.close() } catch (io: IOException) {}
                val cause = e.cause
                // Without the permission this is where Android says no.
                val why = if (cause is SecurityException) {
                    "this application has not been allowed to use Bluetooth"
                } else {
                    cause?.message ?: "the connection failed"
                }
                fail(why, !isConnected(device))
                return@execute
            } catch (e: Exception) {
                try { candidate.close() } catch (io: IOException) {}
                fail(e.message ?: "the connection failed", !isConnected(device))
                return@execute
            }

            // A fresh queue per connection: nothing left over from the last
            // one can be mistaken for a reply on this one.
            val queue = LinkedBlockingQueue<ByteArray>()
            inbox = queue
            socket = candidate
            Thread({
                val buffer = ByteArray(1024)
                try {
                    val input = candidate.inputStream
                    while (true) {
                        val n = input.read(buffer)
                        if (n < 0) break
                        if (n > 0) queue.put(buffer.copyOf(n))
                    }
                } catch (e: IOException) {
                    // Closed, from either end.
                }
                queue.put(CLOSED)
            }, "aurora-rfcomm-reader").apply { isDaemon = true }.start()

            Log.i(TAG, "control channel open" + (args.channel?.let { " on channel $it" } ?: " through the Serial Port service"))
            result.put("ok", true)
            invoke.resolve(result)
        }
    }

    @Command
    fun rfcommSend(invoke: Invoke) {
        val args = invoke.parseArgs(SendArgs::class.java)
        link.execute {
            val current = socket
            if (current == null) {
                invoke.reject("the control channel is not open")
                return@execute
            }
            try {
                val bytes = fromHex(args.data)
                current.outputStream.write(bytes)
                current.outputStream.flush()
                invoke.resolve()
            } catch (e: Exception) {
                invoke.reject(e.message ?: "the send failed")
            }
        }
    }

    /** Waits up to `timeoutMs` for bytes: `data`, `timeout` or `closed`. */
    @Command
    fun rfcommRecv(invoke: Invoke) {
        val args = invoke.parseArgs(RecvArgs::class.java)
        link.execute {
            val result = JSObject()
            val queue = inbox
            if (socket == null) {
                invoke.resolve(result.put("kind", "closed"))
                return@execute
            }
            val chunk = try {
                queue.poll(args.timeoutMs.coerceAtLeast(0), TimeUnit.MILLISECONDS)
            } catch (e: InterruptedException) {
                null
            }
            when {
                chunk == null -> result.put("kind", "timeout")
                chunk === CLOSED -> {
                    // Leave the marker for whoever asks next.
                    queue.put(CLOSED)
                    result.put("kind", "closed")
                }
                else -> {
                    result.put("kind", "data")
                    result.put("data", toHex(chunk))
                }
            }
            invoke.resolve(result)
        }
    }

    @Command
    fun rfcommClose(invoke: Invoke) {
        link.execute {
            closeSocket()
            invoke.resolve()
        }
    }

    // --- audio ---------------------------------------------------------------

    private fun isBluetoothOutput(info: AudioDeviceInfo): Boolean =
        info.type == AudioDeviceInfo.TYPE_BLUETOOTH_A2DP ||
            (Build.VERSION.SDK_INT >= 31 && info.type == AudioDeviceInfo.TYPE_BLE_HEADSET)

    @SuppressLint("MissingPermission")
    private fun a2dpConnected(address: String): Boolean = try {
        a2dp?.connectedDevices?.any { it.address.equals(address, ignoreCase = true) } == true
    } catch (e: Exception) {
        false
    }

    /**
     * The output that belongs to the headphones with this address.
     *
     * Matched by address where Android reveals it. Where it does not, the
     * only Bluetooth media output counts as theirs if Android's own media
     * profile says they are the device connected for media.
     */
    @SuppressLint("MissingPermission")
    private fun outputFor(address: String): AudioDeviceInfo? {
        val outputs = audio.getDevices(AudioManager.GET_DEVICES_OUTPUTS).filter { isBluetoothOutput(it) }
        outputs.firstOrNull { it.address.equals(address, ignoreCase = true) }?.let { return it }

        // Recent versions of Android hide all but the end of the address
        // ("XX:XX:XX:XX:F0:2E"). The end is still a match.
        val tail = address.takeLast(5)
        outputs.firstOrNull {
            it.address.startsWith("XX", ignoreCase = true) && it.address.takeLast(5).equals(tail, ignoreCase = true)
        }?.let { return it }

        // No address at all: the only Bluetooth output there is counts as
        // theirs if Android itself says they are connected.
        if (outputs.size != 1 || !outputs[0].address.isNullOrEmpty()) return null
        val connected = a2dpConnected(address) || try {
            adapter?.getRemoteDevice(address)?.let { isConnected(it) } == true
        } catch (e: Exception) {
            false
        }
        return if (connected) outputs[0] else null
    }

    private fun describe(info: AudioDeviceInfo): JSObject =
        JSObject().put("id", info.id).put("name", info.productName?.toString() ?: "Audio output")

    /** The headphones' own output, and the phone's speaker. */
    @Command
    fun outputs(invoke: Invoke) {
        val args = invoke.parseArgs(OutputsArgs::class.java)
        work.execute {
            ensureProxies()
            val result = JSObject()
            args.address?.let { address -> outputFor(address)?.let { result.put("headphones", describe(it)) } }
            audio.getDevices(AudioManager.GET_DEVICES_OUTPUTS)
                .firstOrNull { it.type == AudioDeviceInfo.TYPE_BUILTIN_SPEAKER }
                ?.let { result.put("speaker", JSObject().put("id", it.id).put("name", "Phone speaker")) }
            invoke.resolve(result)
        }
    }

    // --- the phone's media volume ---------------------------------------------

    private fun outputLabel(info: AudioDeviceInfo): String = when (info.type) {
        AudioDeviceInfo.TYPE_BUILTIN_SPEAKER, AudioDeviceInfo.TYPE_BUILTIN_EARPIECE -> "Phone speaker"
        AudioDeviceInfo.TYPE_WIRED_HEADPHONES, AudioDeviceInfo.TYPE_WIRED_HEADSET -> "Wired headphones"
        else -> info.productName?.toString()?.takeIf { it.isNotBlank() } ?: "Audio output"
    }

    /**
     * What media is playing through at the moment. Android 13 and later say;
     * before that it is worked out the way Android itself chooses: a
     * Bluetooth output if there is one, then a wired one, then the speaker.
     */
    private fun mediaOutputName(): String? {
        val outputs = audio.getDevices(AudioManager.GET_DEVICES_OUTPUTS)
        if (Build.VERSION.SDK_INT >= 33) {
            try {
                val media = AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_MEDIA)
                    .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                    .build()
                val routed = audio.getAudioDevicesForAttributes(media).firstOrNull()
                if (routed != null) {
                    val sameKind = outputs.filter { it.type == routed.type }
                    val exact = sameKind.firstOrNull { it.address == routed.address }
                    (exact ?: sameKind.firstOrNull())?.let { return outputLabel(it) }
                }
            } catch (e: Throwable) {
                // Fall through to working it out.
            }
        }
        outputs.firstOrNull { isBluetoothOutput(it) }?.let { return outputLabel(it) }
        outputs.firstOrNull {
            it.type == AudioDeviceInfo.TYPE_WIRED_HEADPHONES ||
                it.type == AudioDeviceInfo.TYPE_WIRED_HEADSET ||
                it.type == AudioDeviceInfo.TYPE_USB_HEADSET
        }?.let { return outputLabel(it) }
        outputs.firstOrNull { it.type == AudioDeviceInfo.TYPE_BUILTIN_SPEAKER }?.let { return outputLabel(it) }
        return null
    }

    private fun lowestMediaVolume(): Int =
        if (Build.VERSION.SDK_INT >= 28) audio.getStreamMinVolume(AudioManager.STREAM_MUSIC) else 0

    /** The media volume as Android holds it, in its own steps. */
    private fun volumeState(): JSObject {
        val lowest = lowestMediaVolume()
        val highest = audio.getStreamMaxVolume(AudioManager.STREAM_MUSIC)
        val now = audio.getStreamVolume(AudioManager.STREAM_MUSIC)
        val result = JSObject()
            .put("level", (now - lowest).coerceAtLeast(0))
            .put("steps", (highest - lowest).coerceAtLeast(0))
            .put("muted", audio.isStreamMute(AudioManager.STREAM_MUSIC))
        mediaOutputName()?.let { result.put("output", it) }
        return result
    }

    /** Reads the media volume. Changes nothing. */
    @Command
    fun volumeGet(invoke: Invoke) {
        work.execute {
            try {
                invoke.resolve(volumeState())
            } catch (e: Exception) {
                invoke.reject(e.message ?: "the media volume could not be read")
            }
        }
    }

    /**
     * Sets the media volume to a step and answers with what Android then
     * holds, which is for the caller to compare. No volume panel is shown.
     */
    @Command
    fun volumeSet(invoke: Invoke) {
        val args = invoke.parseArgs(VolumeArgs::class.java)
        work.execute {
            try {
                val lowest = lowestMediaVolume()
                val highest = audio.getStreamMaxVolume(AudioManager.STREAM_MUSIC)
                val step = (lowest + args.level).coerceIn(lowest, highest)
                audio.setStreamVolume(AudioManager.STREAM_MUSIC, step, 0)
                invoke.resolve(volumeState())
            } catch (e: Exception) {
                invoke.reject(e.message ?: "the media volume could not be changed")
            }
        }
    }

    @Command
    fun muteSet(invoke: Invoke) {
        val args = invoke.parseArgs(MuteArgs::class.java)
        work.execute {
            try {
                val direction = if (args.muted) AudioManager.ADJUST_MUTE else AudioManager.ADJUST_UNMUTE
                audio.adjustStreamVolume(AudioManager.STREAM_MUSIC, direction, 0)
                invoke.resolve(volumeState())
            } catch (e: Exception) {
                invoke.reject(e.message ?: "the media volume could not be changed")
            }
        }
    }

    private fun releasePlayback(stopNow: Boolean) {
        val current = playing ?: return
        playing = null
        val finish = Runnable {
            try {
                current.track.stop()
            } catch (e: Exception) {
            }
            current.track.release()
            focus?.let { audio.abandonAudioFocusRequest(it) }
            focus = null
        }
        if (stopNow) {
            try {
                current.track.pause()
                current.track.flush()
            } catch (e: Exception) {
            }
            finish.run()
        } else {
            // The last of the audio is still on its way to the headphones.
            main.postDelayed(finish, 400)
        }
    }

    /**
     * Starts playing samples and returns at once. Speech volume was applied
     * by the caller; no stream or device volume is read or changed here.
     */
    @Command
    fun playStart(invoke: Invoke) {
        val args = invoke.parseArgs(PlayArgs::class.java)
        work.execute {
            try {
                releasePlayback(stopNow = true)
                val bytes = fromHex(args.pcm)
                val frames = bytes.size / 2
                if (frames == 0) {
                    invoke.reject("there is nothing to play")
                    return@execute
                }
                val attributes = AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_MEDIA)
                    .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                    .build()
                val format = AudioFormat.Builder()
                    .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                    .setSampleRate(args.sampleRate)
                    .setChannelMask(AudioFormat.CHANNEL_OUT_MONO)
                    .build()
                val track = AudioTrack.Builder()
                    .setAudioAttributes(attributes)
                    .setAudioFormat(format)
                    .setTransferMode(AudioTrack.MODE_STATIC)
                    .setBufferSizeInBytes(bytes.size)
                    .build()
                if (track.write(bytes, 0, bytes.size) < 0) {
                    track.release()
                    invoke.reject("the audio buffer could not be filled")
                    return@execute
                }
                val deviceId = args.deviceId
                if (deviceId != null) {
                    val target = audio.getDevices(AudioManager.GET_DEVICES_OUTPUTS).firstOrNull { it.id == deviceId }
                    if (target == null || !track.setPreferredDevice(target)) {
                        track.release()
                        invoke.reject("that audio output is no longer there")
                        return@execute
                    }
                }
                // Other audio dips while this is spoken, and comes back after.
                val request = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK)
                    .setAudioAttributes(attributes)
                    .build()
                audio.requestAudioFocus(request)
                focus = request
                track.play()
                val millis = frames * 1000L / args.sampleRate
                playing = Playing(track, frames, System.currentTimeMillis(), millis)
                invoke.resolve(JSObject().put("frames", frames))
            } catch (e: Exception) {
                invoke.reject(e.message ?: "playback could not start")
            }
        }
    }

    /** Whether the samples handed to [playStart] have all been played. */
    @Command
    fun playPoll(invoke: Invoke) {
        val current = playing
        val result = JSObject()
        if (current == null) {
            invoke.resolve(result.put("done", true))
            return
        }
        val played = try {
            current.track.playbackHeadPosition
        } catch (e: Exception) {
            current.frames
        }
        val overdue = System.currentTimeMillis() - current.startedAt > current.millis + 2000
        val done = played >= current.frames || overdue
        if (done) releasePlayback(stopNow = false)
        invoke.resolve(result.put("done", done))
    }

    @Command
    fun playStop(invoke: Invoke) {
        releasePlayback(stopNow = true)
        invoke.resolve()
    }

    // --- staying alive in the background -------------------------------------

    /**
     * Starts or stops the foreground service whose only job is to keep this
     * process running while the application is not on screen, so that the
     * native watcher can go on noticing changes and announcing them.
     */
    @Command
    fun keepAlive(invoke: Invoke) {
        val args = invoke.parseArgs(KeepAliveArgs::class.java)
        main.post {
            try {
                val intent = Intent(activity, AuroraService::class.java)
                if (args.on) {
                    ContextCompat.startForegroundService(activity, intent)
                } else {
                    activity.stopService(intent)
                }
                invoke.resolve()
            } catch (e: Exception) {
                invoke.reject(e.message ?: "the background service could not be changed")
            }
        }
    }

    // --- the voice model that came inside the package -------------------------

    /**
     * Unpacks the voice model shipped in the package, once, and says where it
     * is. The speech runtime opens files by path, and files inside an APK do
     * not have one.
     */
    @Command
    fun modelRoot(invoke: Invoke) {
        work.execute {
            val result = JSObject()
            try {
                val shipped = activity.assets.list("")?.contains("voice-model.zip") == true
                if (!shipped) {
                    invoke.resolve(result)
                    return@execute
                }
                val version = activity.packageManager.getPackageInfo(activity.packageName, 0).let {
                    if (Build.VERSION.SDK_INT >= 28) it.longVersionCode else @Suppress("DEPRECATION") it.versionCode.toLong()
                }
                val updated = activity.packageManager.getPackageInfo(activity.packageName, 0).lastUpdateTime
                val root = File(activity.filesDir, "voice-bundled")
                val marker = File(root, ".unpacked-$version-$updated")
                if (!marker.exists()) {
                    root.deleteRecursively()
                    root.mkdirs()
                    val base = root.canonicalPath + File.separator
                    ZipInputStream(activity.assets.open("voice-model.zip").buffered(1 shl 16)).use { zip ->
                        val buffer = ByteArray(1 shl 16)
                        while (true) {
                            val entry = zip.nextEntry ?: break
                            val name = entry.name.replace('\\', '/').trimStart('/')
                            if (name.isEmpty() || name == "." || name == "./") continue
                            val target = File(root, name)
                            // Nothing may land outside the folder being filled.
                            if (!target.canonicalPath.startsWith(base)) continue
                            if (entry.isDirectory || name.endsWith("/")) {
                                target.mkdirs()
                                continue
                            }
                            target.parentFile?.mkdirs()
                            target.outputStream().use { out ->
                                while (true) {
                                    val n = zip.read(buffer)
                                    if (n < 0) break
                                    out.write(buffer, 0, n)
                                }
                            }
                        }
                    }
                    marker.createNewFile()
                }
                invoke.resolve(result.put("path", root.absolutePath))
            } catch (e: Exception) {
                Log.e(TAG, "could not unpack the voice model", e)
                invoke.reject("The voice model in the package could not be unpacked: ${e.message}")
            }
        }
    }
}
