package local.bosecontrolcenter.app

import android.app.PendingIntent
import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import android.view.View
import android.widget.RemoteViews
import java.util.concurrent.LinkedBlockingQueue

/**
 * What passes between the home-screen widget and the native layer.
 *
 * The widget decides nothing. A tap on it is a request for a mode, by name,
 * left here; the native layer — which owns the connection to the headphones,
 * and the rule that a change counts only when they confirm it — takes
 * requests from here one at a time, and hands back what the headphones then
 * report, which is the only thing the widget ever shows as the mode.
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
    class Request(val mode: String, val at: Long)

    /** A request nobody has taken after this long was for a moment that has passed. */
    const val REQUEST_EXPIRES_MS = 20_000L

    /** The native layer asks at least this often while it is alive. */
    private const val ALIVE_WITHIN_MS = 75_000L

    /** More requests than this are never left waiting. */
    private const val MOST_WAITING = 4

    private val requests = LinkedBlockingQueue<Request>()

    /** When the native layer was last heard from: asking for a request, or
     *  reporting a state. Counted in time the processor was awake, as its own
     *  waiting is. */
    @Volatile
    private var lastAsked = 0L

    /** Whether the application's window is on screen. */
    @Volatile
    var visible = false

    /** The mode asked for and not yet answered by a report from the headphones. */
    @Volatile
    var pending: Request? = null
        private set

    /** The last request the native layer took, so a tap can tell it was heard. */
    @Volatile
    private var taken: Request? = null

    private fun now() = SystemClock.uptimeMillis()

    /**
     * Whether there is a native layer to ask, and one that is being run: on
     * screen, or kept going in the background by the service.
     */
    fun alive(): Boolean =
        lastAsked != 0L && now() - lastAsked < ALIVE_WITHIN_MS && (visible || AuroraService.running)

    fun markStopped() {
        lastAsked = 0L
    }

    /** The native layer has just said something, which only a live one does. */
    fun heard() {
        lastAsked = now()
    }

    /** Leaves a request for the native layer. `refresh` asks only to be told the state. */
    fun ask(mode: String): Request {
        val request = Request(mode, now())
        if (mode != "refresh") pending = request
        // With nothing taking them they would only pile up; the newest are
        // the ones that still mean anything.
        while (requests.size >= MOST_WAITING) requests.poll()
        requests.offer(request)
        return request
    }

    fun wasTaken(request: Request): Boolean = taken === request

    /**
     * Waits for the next request, up to `waitMs`. Called by the native layer
     * only; calling it at all is what shows the native layer to be alive.
     */
    fun next(waitMs: Long): Request? {
        lastAsked = now()
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
            // One left over from before the application was started, or from
            // a moment that has passed, is not acted on.
            if (now() - request.at > REQUEST_EXPIRES_MS) {
                if (pending === request) pending = null
                continue
            }
            taken = request
            return request
        }
    }

    /** The headphones have reported, or the attempt is over: nothing is pending. */
    fun answered() {
        pending = null
    }
}

/**
 * The home-screen widget: the three noise-control modes, and nothing else.
 *
 * It shows the mode the headphones last reported, never the one that was
 * asked for. Between a tap and the report, the button tapped reads as
 * "switching".
 */
class ModeWidget : AppWidgetProvider() {
    companion object {
        private const val TAG = "Aurora"
        const val ACTION_MODE = "local.bosecontrolcenter.app.widget.MODE"
        const val EXTRA_MODE = "mode"
        private val MODES = listOf("quiet", "aware", "custom")

        private const val PREFS = "aurora-widget"
        private const val KEY_STATUS = "status"
        private const val KEY_NAME = "name"
        private const val KEY_MODE = "mode"
        private const val KEY_CUSTOM = "customName"
        private const val KEY_NOTE = "note"

        /** How long a tap waits to be taken up before the application is judged not to be running. */
        private const val TAKEN_WITHIN_MS = 2_500L

        private val main = Handler(Looper.getMainLooper())

        /** Remembers what the native layer reported, and shows it. */
        fun report(context: Context, status: String, name: String?, mode: String?, customName: String?, note: String?) {
            context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
                .putString(KEY_STATUS, status)
                .putString(KEY_NAME, name)
                .putString(KEY_MODE, mode)
                .putString(KEY_CUSTOM, customName)
                .putString(KEY_NOTE, note)
                .apply()
            WidgetBus.heard()
            WidgetBus.answered()
            Log.i(TAG, "widget: " + status + (mode?.let { ", $it" } ?: ""))
            render(context)
        }

        /** The application is being closed: say so now, while there is still a process to say it. */
        fun appClosing(context: Context) {
            WidgetBus.markStopped()
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

        fun render(context: Context) {
            val manager = AppWidgetManager.getInstance(context) ?: return
            val ids = manager.getAppWidgetIds(ComponentName(context, ModeWidget::class.java))
            if (ids.isEmpty()) return

            val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            val alive = WidgetBus.alive()
            val status = if (alive) prefs.getString(KEY_STATUS, null) else null
            val connected = status == "connected"
            // A mode is shown only while the application is alive to vouch
            // for it and the headphones are connected.
            val mode = if (connected) prefs.getString(KEY_MODE, null) else null
            // And a request reads as under way only while there is something
            // to carry it out.
            val pending = if (alive) WidgetBus.pending?.mode else null
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
                note != null -> note
                else -> context.getString(R.string.widget_connected)
            }

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
                    context.getColor(
                        when {
                            active -> R.color.widget_on_accent
                            alive && connected -> R.color.widget_text
                            else -> R.color.widget_text_dim
                        }
                    )
                )
                // With the application running, a tap is a request to it.
                // Without, the tap opens it, carrying the request along.
                val tap = if (alive) broadcastFor(context, key, index + 1) else openApp(context, key, index + 11)
                views.setOnClickPendingIntent(id, tap)
            }
            views.setViewVisibility(R.id.widget_line, View.VISIBLE)
            manager.updateAppWidget(ids, views)
        }
    }

    override fun onUpdate(context: Context, manager: AppWidgetManager, ids: IntArray) {
        render(context)
        // And if the application is running, have it say where things stand.
        if (WidgetBus.alive()) WidgetBus.ask("refresh")
    }

    override fun onReceive(context: Context, intent: Intent) {
        super.onReceive(context, intent)
        if (intent.action != ACTION_MODE) return
        val mode = intent.getStringExtra(EXTRA_MODE)?.takeIf { it in MODES } ?: return
        val app = context.applicationContext

        val request = WidgetBus.ask(mode)
        Log.i(TAG, "widget: $mode asked for")
        // Nothing to take it: the process was started for this tap alone.
        if (!WidgetBus.alive()) {
            open(app)
            return
        }
        render(app)
        // The native layer takes a request within a moment if it is there.
        // If this one is still lying here, it is not. The receiver is held
        // open until that is known, so Android does not stop the process in
        // the middle of finding out.
        val held = goAsync()
        main.postDelayed({
            try {
                if (!WidgetBus.wasTaken(request) && WidgetBus.pending === request) open(app)
            } finally {
                held?.finish()
            }
        }, TAKEN_WITHIN_MS)
    }

    /**
     * Says the application is not running, and tries to open it; it will find
     * the request waiting when it starts. If Android will not let it be
     * opened from here, the widget's buttons now open it directly.
     */
    private fun open(app: Context) {
        Log.i(TAG, "widget: the application is not running; opening it")
        WidgetBus.markStopped()
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
