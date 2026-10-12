package local.bosecontrolcenter.app

import android.content.Intent
import android.os.Bundle
import android.view.View
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)

    // Opened by a tap on the widget while the application was not running:
    // the mode that was tapped is left for the native layer, which takes it
    // up as soon as it has started. Not when the activity is being rebuilt.
    if (savedInstanceState == null) takeWidgetRequest(intent)

    // The window runs edge to edge, so the interface would otherwise sit
    // under the status bar, the camera cut-out, the gesture bar and the
    // keyboard. Keep it inside them.
    val content = findViewById<View>(android.R.id.content)
    ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
      val bars = insets.getInsets(
        WindowInsetsCompat.Type.systemBars() or
          WindowInsetsCompat.Type.displayCutout() or
          WindowInsetsCompat.Type.ime()
      )
      view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
      WindowInsetsCompat.CONSUMED
    }
  }

  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    takeWidgetRequest(intent)
  }

  private fun takeWidgetRequest(intent: Intent?) {
    val mode = ModeWidget.requestFrom(intent) ?: return
    WidgetBus.ask("mode", mode)
    ModeWidget.render(applicationContext)
  }

  override fun onStart() {
    super.onStart()
    // On screen, the native layer is being run, whatever the setting for the
    // background is. It is asked to say where things stand: it may have been
    // stopped where it stood for some time.
    WidgetBus.visible = true
    WidgetBus.ask("refresh")
    ModeWidget.render(applicationContext)
  }

  override fun onStop() {
    // Off screen. Unless the service is keeping the process going, the widget
    // stops showing a mode here: nothing would notice it changing.
    WidgetBus.visible = false
    ModeWidget.render(applicationContext)
    super.onStop()
  }

  override fun onDestroy() {
    // Closed for good, not turned over for a new layout: with no background
    // service to keep it, the native layer goes with the window.
    if (isFinishing) ModeWidget.appClosing(applicationContext)
    super.onDestroy()
  }
}
