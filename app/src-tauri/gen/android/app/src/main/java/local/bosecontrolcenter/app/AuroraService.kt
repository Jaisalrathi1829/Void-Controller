package local.bosecontrolcenter.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat

/**
 * Keeps the application's process alive while it is not on screen.
 *
 * It does nothing itself. The watching — the headphones coming and going,
 * their mode, their battery — is done by the native layer, which lives in
 * this process and would be frozen with it. Android requires a visible
 * notification for as long as this runs, and that notification is the off
 * switch: the setting that starts this also stops it.
 */
class AuroraService : Service() {
    companion object {
        private const val CHANNEL = "background"
        private const val NOTIFICATION = 1

        /** Whether the process is being kept going. The widget goes by it. */
        @Volatile
        var running = false
            private set
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val manager = getSystemService(NotificationManager::class.java)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            manager.createNotificationChannel(
                NotificationChannel(CHANNEL, "Running in the background", NotificationManager.IMPORTANCE_LOW)
            )
        }
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        )
        val notification: Notification = NotificationCompat.Builder(this, CHANNEL)
            .setSmallIcon(android.R.drawable.stat_sys_headset)
            .setContentTitle(getString(R.string.app_name))
            .setContentText("Watching your headphones for changes to announce")
            .setContentIntent(open)
            .setOngoing(true)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .build()
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                startForeground(NOTIFICATION, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
            } else {
                startForeground(NOTIFICATION, notification)
            }
            running = true
            ModeWidget.render(applicationContext)
        } catch (e: Exception) {
            // Without the Bluetooth permission Android refuses this kind of
            // service. There is nothing to keep alive for in that case.
            stopSelf()
        }
        // If Android ends the process, the native layer goes with it; coming
        // back without it would be a notification with nothing behind it.
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        running = false
        // Switched off while the application was off screen: from here on
        // nothing is watching, and the widget should not look as if it were.
        ModeWidget.render(applicationContext)
        super.onDestroy()
    }

    /** Swiped away from recents: the user has closed the application. */
    override fun onTaskRemoved(rootIntent: Intent?) {
        // The widget is told while there is still a process to tell it, so
        // that it does not go on showing a mode nothing is watching.
        ModeWidget.appClosing(applicationContext)
        stopSelf()
        super.onTaskRemoved(rootIntent)
    }
}
