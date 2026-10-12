package local.bosecontrolcenter.app

import android.app.PendingIntent
import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.res.Configuration
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import android.util.SizeF
import android.view.View
import android.widget.RemoteViews
import java.util.concurrent.LinkedBlockingQueue

/**
 * What passes between the home-screen widget and the native layer.
 *
 * The widget decides nothing. A tap on it is a request by name — a mode, or
 * one band of the equalizer one step up or down — left here; the native layer,
 * which owns the connection to the headphones and the rule that a change
 * counts only when they confirm it, takes requests from here one at a time,
 * and hands back what the headphones then report, which is the only thing the
 * widget ever shows as the mode or as a band's level.
 *
 * All of it lives in the application's process. If that process is not
 * running there is no native layer, nothing takes the request, and the widget
 * says so instead of showing a mode it can no longer vouch for.
 *
 * The same goes for a process that is there but not being run. Off screen,
 * and without the service that keeps it going, Android stops the application
 * where it stands: nothing in it would notice the headphones changing mode,
 * so the widget does not go on showing one.
 */
object WidgetBus {
    /**
     * One request. `kind` is `mode`, `eq` or `refresh`; `what` is the mode or
     * the band; `step` is `up` or `down`. Names only, and never an amount.
     */
    class Request(val kind: String, val what: String?, val step: String?, val at: Long) {
        /** Taken up by the native layer, or given up on: no longer waiting. */
        @Volatile
        var settled = false
    }

    /** A request nobody has taken after this long was for a moment that has passed. */
    const val REQUEST_EXPIRES_MS = 20_000L

    /** The native layer asks at least this often while it is alive. */
    private const val ALIVE_WITHIN_MS = 75_000L

    /** More requests than this are never left waiting. */
    private const val MOST_WAITING = 12

    /** One request is not worked on for longer than this. */
    private const val WORK_WITHIN_MS = 15_000L

    private val requests = LinkedBlockingQueue<Request>()

    /** When the native layer was last heard from: asking for a request, or
     *  reporting a state. Counted in time the processor was awake, as its own
     *  waiting is. */
    @Volatile
    private var lastAsked = 0L

    /** When the native layer took the request it has not yet come back from. */
    @Volatile
    private var workingSince = 0L

    /** Whether the application's window is on screen. */
    @Volatile
    var visible = false

    /** The mode asked for and not yet answered by a report from the headphones. */
    @Volatile
    var pending: Request? = null
        private set

    /** Whether a step of the equalizer has been asked for and not yet answered. */
    @Volatile
    var adjusting = false
        private set

    private fun now() = SystemClock.uptimeMillis()

    /**
     * Whether there is a native layer to ask, and one that is being run: on
     * screen, or kept going in the background by the service.
     */
    fun alive(): Boolean =
        lastAsked != 0L && now() - lastAsked < ALIVE_WITHIN_MS && (visible || AuroraService.running)

    fun markStopped() {
        lastAsked = 0L
        workingSince = 0L
    }

    /** The native layer has just said something, which only a live one does. */
    fun heard() {
        lastAsked = now()
    }

    /**
     * Whether the native layer is in the middle of a request it took. One
     * that is busy with an earlier tap has not gone away.
     */
    fun working(): Boolean = workingSince != 0L && now() - workingSince < WORK_WITHIN_MS

    /** Whether a mode or a step is still waiting to be taken. */
    fun waiting(): Boolean = requests.any { it.kind != "refresh" }

    /** Leaves a request for the native layer. `refresh` asks only to be told the state. */
    fun ask(kind: String, what: String? = null, step: String? = null): Request {
        // One request to be told the state is as good as several.
        if (kind == "refresh") requests.firstOrNull { it.kind == "refresh" }?.let { return it }
        val request = Request(kind, what, step, now())
        when (kind) {
            "mode" -> pending = request
            "eq" -> adjusting = true
        }
        // With nothing taking them they would only pile up; the newest are
        // the ones that still mean anything.
        while (requests.size >= MOST_WAITING) requests.poll()?.let { it.settled = true }
        requests.offer(request)
        return request
    }

    /**
     * Gives up on the equalizer steps still waiting. A step is from a level
     * on show; it is not kept for an application that has yet to start and
     * may find a different one.
     */
    fun dropSteps() {
        requests.removeAll { request ->
            (request.kind == "eq").also { if (it) request.settled = true }
        }
        adjusting = false
    }

    /**
     * Waits for the next request, up to `waitMs`. Called by the native layer
     * only; calling it at all is what shows the native layer to be alive.
     */
    fun next(waitMs: Long): Request? {
        lastAsked = now()
        workingSince = 0L
        val deadline = now() + waitMs
        while (true) {
            val left = deadline - now()
            val request = try {
                if (left <= 0) requests.poll() else requests.poll(left, java.util.concurrent.TimeUnit.MILLISECONDS)
            } catch (e: InterruptedException) {
                null
            }
            lastAsked = now()
            if (request == null) return null
            request.settled = true
            // One left over from before the application was started, or from
            // a moment that has passed, is not acted on.
            if (now() - request.at > REQUEST_EXPIRES_MS) {
                if (pending === request) pending = null
                continue
            }
            workingSince = now()
            return request
        }
    }

    /** The headphones have reported, or the attempt is over: nothing is pending. */
    fun answered() {
        pending = null
        adjusting = false
    }
}

/**
 * The home-screen widget: the three noise-control modes and, when it has been
 * given the room, the three bands of the equalizer, each with a step down and
 * a step up.
 *
 * It shows what the headphones last reported, never what was asked for.
 * Between a tap and the report, the mode tapped reads as "switching"; a
 * band's number moves only when the headphones say it has.
 */
class ModeWidget : AppWidgetProvider() {
    private class Band(val key: String, val value: Int, val down: Int, val up: Int)

    companion object {
        private const val TAG = "Aurora"
        const val ACTION_MODE = "local.bosecontrolcenter.app.widget.MODE"
        const val ACTION_EQ = "local.bosecontrolcenter.app.widget.EQ"
        const val EXTRA_MODE = "mode"
        const val EXTRA_BAND = "band"
        const val EXTRA_STEP = "step"
        private val MODES = listOf("quiet", "aware", "custom")
        private val BANDS = listOf("bass", "mid", "treble")
        private val STEPS = listOf("down", "up")

        /** The ends of a band's range, for greying the step that leads past
         *  one. The native layer is what holds a step to the range. */
        private const val EQ_MIN = -10
        private const val EQ_MAX = 10

        /** The room the widget needs, in dp, with and without the equalizer. */
        private const val MIN_WIDTH_DP = 110f
        private const val MIN_HEIGHT_DP = 40f
        private const val EQ_FROM_WIDTH_DP = 220f
        private const val EQ_FROM_HEIGHT_DP = 150f

        private const val PREFS = "aurora-widget"
        private const val KEY_STATUS = "status"
        private const val KEY_NAME = "name"
        private const val KEY_MODE = "mode"
        private const val KEY_CUSTOM = "customName"
        private const val KEY_EQ = "eq"
        private const val KEY_NOTE = "note"

        /** How long a tap waits to be taken up before the application is judged not to be running. */
        private const val TAKEN_WITHIN_MS = 2_500L

        private val main = Handler(Looper.getMainLooper())

        private val bands = listOf(
            Band("bass", R.id.widget_bass_value, R.id.widget_bass_down, R.id.widget_bass_up),
            Band("mid", R.id.widget_mid_value, R.id.widget_mid_down, R.id.widget_mid_up),
            Band("treble", R.id.widget_treble_value, R.id.widget_treble_down, R.id.widget_treble_up),
        )

        /**
         * Remembers what the native layer reported, and shows it. Answers
         * whether a widget on a home screen has the equalizer on it, so the
         * native layer knows whether there is any call to keep one read.
         */
        fun report(
            context: Context,
            status: String,
            name: String?,
            mode: String?,
            customName: String?,
            bass: Int?,
            mid: Int?,
            treble: Int?,
            note: String?,
        ): Boolean {
            // All three bands or none: a level is never shown beside a guess.
            val eq = if (bass != null && mid != null && treble != null) "$bass,$mid,$treble" else null
            context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
                .putString(KEY_STATUS, status)
                .putString(KEY_NAME, name)
                .putString(KEY_MODE, mode)
                .putString(KEY_CUSTOM, customName)
                .putString(KEY_EQ, eq)
                .putString(KEY_NOTE, note)
                .apply()
            WidgetBus.heard()
            // Nothing is pending any more, unless more taps are still waiting
            // their turn.
            if (!WidgetBus.waiting()) WidgetBus.answered()
            Log.i(TAG, "widget: " + status + (mode?.let { ", $it" } ?: "") + (eq?.let { ", eq $it" } ?: ""))
            render(context)
            return showsEq(context)
        }

        /** The application is being closed: say so now, while there is still a process to say it. */
        fun appClosing(context: Context) {
            WidgetBus.markStopped()
            WidgetBus.dropSteps()
            WidgetBus.answered()
            render(context)
        }

        /** Whether a mode was asked for by the tap that opened the activity. */
        fun requestFrom(intent: Intent?): String? {
            if (intent == null) return null
            // Brought back from the recent-apps list, the activity is handed
            // the intent it was first started with. That is not a tap.
            if (intent.flags and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY != 0) return null
            val mode = intent.getStringExtra(EXTRA_MODE) ?: return null
            intent.removeExtra(EXTRA_MODE)
            return mode.takeIf { it in MODES }
        }

        private fun broadcastFor(context: Context, mode: String, code: Int): PendingIntent =
            PendingIntent.getBroadcast(
                context, code,
                Intent(context, ModeWidget::class.java).setAction(ACTION_MODE).putExtra(EXTRA_MODE, mode),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
            )

        private fun stepFor(context: Context, band: String, step: String, code: Int): PendingIntent =
            PendingIntent.getBroadcast(
                context, code,
                Intent(context, ModeWidget::class.java).setAction(ACTION_EQ)
                    .putExtra(EXTRA_BAND, band).putExtra(EXTRA_STEP, step),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
            )

        /** Opens the application; with `mode`, asks it for that mode once it is up. */
        private fun openApp(context: Context, mode: String?, code: Int): PendingIntent {
            val intent = Intent(context, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            if (mode != null) intent.putExtra(EXTRA_MODE, mode)
            return PendingIntent.getActivity(
                context, code, intent,
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
            )
        }

        /** A band's level as it is written: +4, 0, −2. */
        private fun written(gain: Int): String = when {
            gain > 0 -> "+$gain"
            gain < 0 -> "−${-gain}"
            else -> "0"
        }

        /**
         * Whether the launcher says this widget has room for the equalizer.
         * `null` if it does not say.
         */
        private fun roomForEq(context: Context, manager: AppWidgetManager, id: Int): Boolean? {
            val options = manager.getAppWidgetOptions(id) ?: return null
            val landscape = context.resources.configuration.orientation == Configuration.ORIENTATION_LANDSCAPE
            val height = options.getInt(
                if (landscape) AppWidgetManager.OPTION_APPWIDGET_MIN_HEIGHT else AppWidgetManager.OPTION_APPWIDGET_MAX_HEIGHT, 0
            )
            val width = options.getInt(
                if (landscape) AppWidgetManager.OPTION_APPWIDGET_MAX_WIDTH else AppWidgetManager.OPTION_APPWIDGET_MIN_WIDTH, 0
            )
            if (height <= 0 || width <= 0) return null
            return height >= EQ_FROM_HEIGHT_DP && width >= EQ_FROM_WIDTH_DP
        }

        /** Whether any widget on a home screen has the equalizer on it. */
        private fun showsEq(context: Context): Boolean {
            val manager = AppWidgetManager.getInstance(context) ?: return false
            val ids = manager.getAppWidgetIds(ComponentName(context, ModeWidget::class.java))
            // A launcher that does not give sizes may be showing it.
            return ids.any { roomForEq(context, manager, it) ?: (Build.VERSION.SDK_INT >= 31) }
        }

        fun render(context: Context) {
            val manager = AppWidgetManager.getInstance(context) ?: return
            val ids = manager.getAppWidgetIds(ComponentName(context, ModeWidget::class.java))
            if (ids.isEmpty()) return

            if (Build.VERSION.SDK_INT >= 31) {
                // Both forms are handed over, and Android shows whichever the
                // widget has room for, as it is resized.
                val views = RemoteViews(
                    mapOf(
                        SizeF(MIN_WIDTH_DP, MIN_HEIGHT_DP) to build(context, eq = false),
                        SizeF(EQ_FROM_WIDTH_DP, EQ_FROM_HEIGHT_DP) to build(context, eq = true),
                    )
                )
                manager.updateAppWidget(ids, views)
            } else {
                for (id in ids) {
                    manager.updateAppWidget(id, build(context, eq = roomForEq(context, manager, id) ?: false))
                }
            }
        }

        private fun build(context: Context, eq: Boolean): RemoteViews {
            val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            val alive = WidgetBus.alive()
            val status = if (alive) prefs.getString(KEY_STATUS, null) else null
            val connected = status == "connected"
            // A mode is shown only while the application is alive to vouch
            // for it and the headphones are connected.
            val mode = if (connected) prefs.getString(KEY_MODE, null) else null
            // And a request reads as under way only while there is something
            // to carry it out.
            val pending = if (alive) WidgetBus.pending?.what else null
            val adjusting = alive && WidgetBus.adjusting
            val name = prefs.getString(KEY_NAME, null)?.takeIf { it.isNotBlank() }
                ?: context.getString(R.string.widget_headphones)
            val customName = prefs.getString(KEY_CUSTOM, null)?.takeIf { it.isNotBlank() }
                ?: context.getString(R.string.widget_custom)
            val note = if (alive) prefs.getString(KEY_NOTE, null)?.takeIf { it.isNotBlank() } else null

            val line = when {
                !alive -> context.getString(R.string.widget_not_running)
                status == "simulated" -> context.getString(R.string.widget_simulated)
                !connected -> context.getString(R.string.widget_not_connected)
                pending != null -> context.getString(R.string.widget_switching)
                adjusting -> context.getString(R.string.widget_adjusting)
                note != null -> note
                else -> context.getString(R.string.widget_connected)
            }
            val text = context.getColor(R.color.widget_text)
            val dim = context.getColor(R.color.widget_text_dim)

            val views = RemoteViews(context.packageName, R.layout.widget_modes)
            views.setTextViewText(R.id.widget_name, name)
            views.setTextViewText(R.id.widget_line, line)
            views.setOnClickPendingIntent(R.id.widget_header, openApp(context, null, 0))

            val buttons = listOf(
                Triple(R.id.widget_quiet, "quiet", context.getString(R.string.widget_quiet)),
                Triple(R.id.widget_aware, "aware", context.getString(R.string.widget_aware)),
                Triple(R.id.widget_custom, "custom", customName),
            )
            for ((index, button) in buttons.withIndex()) {
                val (id, key, label) = button
                val active = mode == key && pending == null
                val waiting = pending == key
                views.setTextViewText(id, label)
                views.setInt(
                    id, "setBackgroundResource",
                    when {
                        active -> R.drawable.widget_button_active
                        waiting -> R.drawable.widget_button_waiting
                        else -> R.drawable.widget_button
                    }
                )
                views.setTextColor(
                    id,
                    when {
                        active -> context.getColor(R.color.widget_on_accent)
                        connected -> text
                        else -> dim
                    }
                )
                // With the application running, a tap is a request to it.
                // Without, the tap opens it, carrying the request along.
                val tap = if (alive) broadcastFor(context, key, index + 1) else openApp(context, key, index + 11)
                views.setOnClickPendingIntent(id, tap)
            }

            views.setViewVisibility(R.id.widget_eq, if (eq) View.VISIBLE else View.GONE)
            if (!eq) return views

            // The equalizer the headphones last reported, or none.
            val gains = if (connected) {
                prefs.getString(KEY_EQ, null)?.split(",")?.mapNotNull { it.toIntOrNull() }?.takeIf { it.size == 3 }
            } else {
                null
            }
            for ((index, band) in bands.withIndex()) {
                val gain = gains?.get(index)
                views.setTextViewText(band.value, gain?.let(::written) ?: "–")
                views.setTextColor(band.value, if (gain != null) text else dim)
                for ((offset, step) in STEPS.withIndex()) {
                    val id = if (step == "up") band.up else band.down
                    val atEnd = gain != null && (if (step == "up") gain >= EQ_MAX else gain <= EQ_MIN)
                    views.setTextColor(id, if (gain != null && !atEnd) text else dim)
                    // A step is asked of a running application only. With
                    // none, there is no level on show to step from, and the
                    // tap opens the application and asks for nothing.
                    val tap = if (alive) {
                        stepFor(context, band.key, step, 21 + index * 2 + offset)
                    } else {
                        openApp(context, null, 0)
                    }
                    views.setOnClickPendingIntent(id, tap)
                }
            }
            return views
        }
    }

    override fun onUpdate(context: Context, manager: AppWidgetManager, ids: IntArray) {
        render(context)
        // And if the application is running, have it say where things stand.
        if (WidgetBus.alive()) WidgetBus.ask("refresh")
    }

    /** Resized: it may now have the equalizer on it, or no longer. */
    override fun onAppWidgetOptionsChanged(context: Context, manager: AppWidgetManager, id: Int, options: Bundle) {
        render(context)
        if (WidgetBus.alive()) WidgetBus.ask("refresh")
    }

    override fun onReceive(context: Context, intent: Intent) {
        super.onReceive(context, intent)
        val app = context.applicationContext
        // Whether there is anything to take a request: there is not if the
        // process was started for this tap alone.
        val alive = WidgetBus.alive()
        val request = when (intent.action) {
            ACTION_MODE -> {
                val mode = intent.getStringExtra(EXTRA_MODE)?.takeIf { it in MODES } ?: return
                Log.i(TAG, "widget: $mode asked for")
                WidgetBus.ask("mode", mode)
            }
            ACTION_EQ -> {
                val band = intent.getStringExtra(EXTRA_BAND)?.takeIf { it in BANDS } ?: return
                val step = intent.getStringExtra(EXTRA_STEP)?.takeIf { it in STEPS } ?: return
                Log.i(TAG, "widget: $band $step asked for")
                if (!alive) {
                    open(app)
                    return
                }
                WidgetBus.ask("eq", band, step)
            }
            else -> return
        }
        if (!alive) {
            open(app)
            return
        }
        render(app)
        // The native layer takes a request within a moment if it is there —
        // or is busy with the tap before this one. If this request is still
        // lying here and nothing is being worked on, it is not there.
        //
        // The receiver is not held open while that is found out. Android
        // hands a receiver one tap at a time, so holding it would make every
        // tap wait out the one before; and the process is in no danger of
        // being stopped meanwhile, since it is only counted alive while it
        // is on screen or kept running by the service.
        main.postDelayed({
            if (!request.settled && !WidgetBus.working()) open(app)
        }, TAKEN_WITHIN_MS)
    }

    /**
     * Says the application is not running, and tries to open it; it will find
     * a mode that was asked for waiting when it starts. If Android will not
     * let it be opened from here, the widget's buttons now open it directly.
     */
    private fun open(app: Context) {
        Log.i(TAG, "widget: the application is not running; opening it")
        WidgetBus.markStopped()
        WidgetBus.dropSteps()
        render(app)
        try {
            app.startActivity(
                Intent(app, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            )
        } catch (e: Exception) {
            Log.w(TAG, "widget: the application could not be opened from here", e)
        }
    }
}
