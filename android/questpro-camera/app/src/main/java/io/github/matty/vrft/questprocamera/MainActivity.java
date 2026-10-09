package io.github.matty.vrft.questprocamera;

import android.app.Activity;
import android.content.Context;
import android.content.Intent;
import android.graphics.drawable.GradientDrawable;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.widget.FrameLayout;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;

import io.github.matty.vrft.questprocamera.CameraStreamService.Phase;
import io.github.matty.vrft.questprocamera.Ui.Tone;

/** Control panel for the camera service. Any streaming app can remain in front. */
public final class MainActivity extends Activity {
    /** Longest a stop takes. */
    private static final long RESTART_TIMEOUT_MS = 60_000;
    /** The panel's column stops growing here on a wide window. */
    private static final int MAX_WIDTH_DP = 720;
    private final Handler handler = new Handler(Looper.getMainLooper());
    /** An eye gaze change is restarting the stream. */
    private static volatile boolean restarting;
    private Ui ui;
    private Ui.StatusMark stateMark;
    private TextView stateLabel;
    private TextView headline;
    private TextView detail;
    private TextView startButton;
    private TextView stopButton;
    private Stat pcStat;
    private Stat cameraStat;
    private Stat eyeStat;
    private TextView eyeProblem;
    private final Runnable refresh = new Runnable() {
        @Override public void run() {
            showStatus();
            handler.postDelayed(this, 500);
        }
    };

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        applySettingExtras(getIntent());
        ui = new Ui(this);
        getWindow().setStatusBarColor(Ui.PAGE);
        getWindow().setNavigationBarColor(Ui.PAGE);

        LinearLayout column = new MaxWidthColumn(this, ui.dp(MAX_WIDTH_DP));
        column.setOrientation(LinearLayout.VERTICAL);
        column.setPadding(ui.dp(32), ui.dp(28), ui.dp(32), ui.dp(40));
        FrameLayout centre = new FrameLayout(this);
        centre.addView(column, new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT,
                Gravity.CENTER_HORIZONTAL));
        ScrollView scroll = new ScrollView(this);
        scroll.setBackgroundColor(Ui.PAGE);
        scroll.setFillViewport(true);
        scroll.addView(centre);
        setContentView(scroll);

        column.addView(header());
        column.addView(hero(), spaced(24));
        column.addView(ui.cap("Stream"), spaced(32));
        column.addView(settings(), spaced(12));
        TextView applies = ui.body(
                "Changes apply the next time streaming starts. Eye gaze restarts it straight away.",
                13, Ui.TEXT_3);
        column.addView(applies, spaced(12));
        column.addView(ui.cap("How it works"), spaced(32));
        column.addView(steps(), spaced(12));

        showStatus();
        handleProbeExtras(getIntent());
    }

    private LinearLayout.LayoutParams spaced(int topDp) {
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        params.topMargin = ui.dp(topDp);
        return params;
    }

    /** The app's mark on its blue tile, its name, and its version. */
    private View header() {
        LinearLayout row = new LinearLayout(this);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setGravity(Gravity.CENTER_VERTICAL);

        FrameLayout tile = new FrameLayout(this);
        GradientDrawable blue = new GradientDrawable(GradientDrawable.Orientation.TOP_BOTTOM,
                new int[] {0xff1d4ed8, 0xff0b1a4a});
        blue.setCornerRadius(ui.dp(10));
        tile.setBackground(blue);
        ImageView mark = new ImageView(this);
        mark.setImageResource(R.drawable.ic_mark);
        tile.addView(mark, new FrameLayout.LayoutParams(ui.dp(20), ui.dp(20), Gravity.CENTER));
        row.addView(tile, new LinearLayout.LayoutParams(ui.dp(38), ui.dp(38)));

        LinearLayout names = new LinearLayout(this);
        names.setOrientation(LinearLayout.VERTICAL);
        names.addView(ui.text("Quest Pro Cameras", ui.semibold, 18, Ui.TEXT));
        TextView app = ui.text("VRFaceTracking", ui.sans, 13, Ui.TEXT_3);
        LinearLayout.LayoutParams under = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        under.topMargin = ui.dp(4);
        names.addView(app, under);
        LinearLayout.LayoutParams grow = new LinearLayout.LayoutParams(
                0, ViewGroup.LayoutParams.WRAP_CONTENT, 1);
        grow.leftMargin = ui.dp(14);
        row.addView(names, grow);

        row.addView(ui.text(BuildConfig.VERSION_NAME, ui.mono, 11, Ui.TEXT_3));
        return row;
    }

    /** How the stream stands, the button that starts or stops it, and its parts. */
    private View hero() {
        LinearLayout card = ui.card();

        LinearLayout top = new LinearLayout(this);
        top.setOrientation(LinearLayout.VERTICAL);
        top.setPadding(ui.dp(28), ui.dp(26), ui.dp(28), ui.dp(26));

        LinearLayout label = new LinearLayout(this);
        label.setOrientation(LinearLayout.HORIZONTAL);
        label.setGravity(Gravity.CENTER_VERTICAL);
        stateMark = new Ui.StatusMark(this);
        label.addView(stateMark);
        stateLabel = ui.cap("");
        LinearLayout.LayoutParams gap = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        gap.leftMargin = ui.dp(9);
        label.addView(stateLabel, gap);
        top.addView(label);

        headline = ui.text("", ui.semibold, 30, Ui.TEXT);
        headline.setLetterSpacing(-0.01f);
        top.addView(headline, spaced(14));
        detail = ui.body("", 15, Ui.TEXT_2);
        top.addView(detail, spaced(10));

        LinearLayout buttons = new LinearLayout(this);
        buttons.setOrientation(LinearLayout.HORIZONTAL);
        startButton = ui.primaryButton("Start streaming");
        startButton.setOnClickListener(view -> startStream());
        stopButton = ui.outlineButton("Stop streaming");
        stopButton.setOnClickListener(view -> stopStream());
        buttons.addView(startButton);
        buttons.addView(stopButton);
        top.addView(buttons, spaced(22));
        card.addView(top);

        card.addView(ui.rule(Ui.LINE_SOFT));

        // Wide enough for the longest value, "Not working", so a changing
        // value never flips the row between side by side and stacked.
        Ui.Row stats = new Ui.Row(this, 18);
        stats.setPadding(ui.dp(28), ui.dp(20), ui.dp(28), ui.dp(22));
        pcStat = new Stat("PC");
        cameraStat = new Stat("Cameras");
        eyeStat = new Stat("Eye gaze");
        for (Stat stat : new Stat[] {pcStat, cameraStat, eyeStat}) {
            stats.addFlexible(stat.view, 120);
        }
        card.addView(stats);

        eyeProblem = ui.body("", 13, Ui.SIGNAL_TEXT);
        eyeProblem.setBackground(ui.box(Ui.SIGNAL_BG, Ui.SIGNAL_LINE, 9));
        eyeProblem.setPadding(ui.dp(14), ui.dp(10), ui.dp(14), ui.dp(10));
        LinearLayout.LayoutParams notice = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        notice.setMargins(ui.dp(28), 0, ui.dp(28), ui.dp(22));
        card.addView(eyeProblem, notice);
        return card;
    }

    /** The stream's settings, a row each. */
    private View settings() {
        LinearLayout card = ui.card();
        card.addView(settingRow("Camera frame rate",
                "Smoother mouth tracking, for more bandwidth.",
                ui.new Segments(labelsFor(Settings.CAMERA_FPS_CHOICES, null),
                        indexOf(Settings.CAMERA_FPS_CHOICES, Settings.getCameraFps(this)),
                        position -> Settings.setCameraFps(this,
                                Settings.CAMERA_FPS_CHOICES[position]))));
        card.addView(ui.rule(Ui.LINE_SOFT));
        card.addView(settingRow("Eye snapshots",
                "Measures pupil size on your PC. Frames a second.",
                ui.new Segments(labelsFor(Settings.EYE_PREVIEW_FPS_CHOICES, "Off"),
                        indexOf(Settings.EYE_PREVIEW_FPS_CHOICES, Settings.getEyePreviewFps(this)),
                        position -> Settings.setEyePreviewFps(this,
                                Settings.EYE_PREVIEW_FPS_CHOICES[position]))));
        card.addView(ui.rule(Ui.LINE_SOFT));
        Ui.Toggle eyeGaze = new Ui.Toggle(this);
        eyeGaze.setContentDescription("Independent eye gaze");
        eyeGaze.setChecked(Settings.isEyeEnabled(this));
        eyeGaze.setListener(checked -> {
            Settings.setEyeEnabled(this, checked);
            // A running stream reads the setting only as it starts.
            if (CameraStreamService.isActive()) restartStream();
            else if (!checked) restoreEyeModel();
            showStatus();
        });
        card.addView(settingRow("Independent eye gaze",
                "Tracks each eye on its own while streaming. Its eye model needs a few seconds' "
                        + "tracking restart, once after each reboot. Turning this off puts Meta's "
                        + "model back.",
                eyeGaze));
        card.addView(ui.rule(Ui.LINE_SOFT));
        Ui.Toggle fiveCameras = new Ui.Toggle(this);
        fiveCameras.setContentDescription("All five cameras");
        fiveCameras.setChecked(Settings.isFiveCameras(this));
        fiveCameras.setListener(checked -> {
            // Applies from the next stream start, like the frame rates.
            Settings.setFiveCameras(this, checked);
        });
        card.addView(settingRow("All five cameras",
                "Adds the brow camera and full-rate eye cameras for VRFT versions that read them. "
                        + "Needs about 19 MB/s at 24 fps.",
                fiveCameras));
        return card;
    }

    /** A setting's words beside its control, or above it on a narrow panel. */
    private View settingRow(String title, String description, View control) {
        Ui.Row row = new Ui.Row(this, 16);
        row.setPadding(ui.dp(24), ui.dp(18), ui.dp(24), ui.dp(18));
        LinearLayout words = new LinearLayout(this);
        words.setOrientation(LinearLayout.VERTICAL);
        words.addView(ui.text(title, ui.medium, 15, Ui.TEXT));
        words.addView(ui.body(description, 13, Ui.TEXT_3), spaced(6));
        row.addFlexible(words, 200);
        row.addFixed(control);
        return row;
    }

    /** The three steps from here to tracking. */
    private View steps() {
        LinearLayout card = ui.card();
        card.setPadding(ui.dp(24), ui.dp(8), ui.dp(24), ui.dp(8));
        String[] steps = {
                "Start streaming here.",
                "Open Virtual Desktop or Steam Link and connect to your PC.",
                "VRFaceTracking on your PC finds this headset by itself.",
        };
        for (int i = 0; i < steps.length; i++) {
            LinearLayout row = new LinearLayout(this);
            row.setOrientation(LinearLayout.HORIZONTAL);
            row.setGravity(Gravity.CENTER_VERTICAL);
            row.setPadding(0, ui.dp(12), 0, ui.dp(12));
            TextView number = ui.text(Integer.toString(i + 1), ui.monoMedium, 12, Ui.TEXT_2);
            number.setGravity(Gravity.CENTER);
            number.setBackground(ui.box(Ui.INSET, Ui.LINE_STRONG, 12));
            row.addView(number, new LinearLayout.LayoutParams(ui.dp(24), ui.dp(24)));
            LinearLayout.LayoutParams gap = new LinearLayout.LayoutParams(
                    0, ViewGroup.LayoutParams.WRAP_CONTENT, 1);
            gap.leftMargin = ui.dp(14);
            row.addView(ui.body(steps[i], 14, Ui.TEXT_2), gap);
            card.addView(row);
        }
        return card;
    }

    /** Shows how the stream stands. Runs twice a second while the panel is open. */
    private void showStatus() {
        Phase phase = CameraStreamService.getPhase();
        boolean active = CameraStreamService.isActive();
        boolean finishing = !active && CameraStreamService.isFinishing();
        String message = CameraStreamService.getStatus();
        Tone tone;
        String title;
        String line;
        if (restarting) {
            tone = Tone.WAITING;
            title = "Restarting\u2026";
            line = "Applying the eye gaze change.";
        } else if (finishing) {
            tone = Tone.WAITING;
            title = "Stopping\u2026";
            line = "Stopping the cameras and eye gaze.";
        } else if (!active && phase == Phase.FAILED) {
            tone = Tone.PROBLEM;
            title = "Couldn't start streaming";
            line = message;
        } else if (!active) {
            tone = Tone.OFF;
            title = "Not streaming";
            line = "Start, then open Virtual Desktop or Steam Link.";
        } else {
            switch (phase) {
                case STREAMING:
                    tone = Tone.GOOD;
                    title = "Streaming to your PC";
                    line = "Leave this running behind your streaming app.";
                    break;
                case CONNECTED:
                    tone = Tone.WAITING;
                    title = "Waiting for the cameras";
                    line = "Your PC is connected. The cameras start once your streaming app tracks your face.";
                    break;
                case WAITING:
                    tone = Tone.WAITING;
                    title = "Waiting for your PC";
                    line = "Open your streaming app. Your PC connects by itself.";
                    break;
                default:
                    tone = Tone.WAITING;
                    title = "Starting\u2026";
                    line = message + ".";
                    break;
            }
        }
        stateMark.setTone(tone);
        stateLabel.setText(stateWord(tone));
        stateLabel.setTextColor(tone == Tone.PROBLEM ? Ui.SIGNAL : Ui.TEXT_2);
        headline.setText(title);
        detail.setText(line);

        boolean busy = restarting || finishing;
        boolean showStop = active || restarting;
        startButton.setVisibility(showStop ? View.GONE : View.VISIBLE);
        startButton.setText(phase == Phase.FAILED ? "Try again" : "Start streaming");
        startButton.setEnabled(!busy);
        startButton.setAlpha(busy ? 0.4f : 1f);
        stopButton.setVisibility(showStop ? View.VISIBLE : View.GONE);
        stopButton.setEnabled(!restarting);
        stopButton.setAlpha(restarting ? 0.4f : 1f);

        boolean linked = active && (phase == Phase.CONNECTED || phase == Phase.STREAMING);
        if (linked) pcStat.show(Tone.GOOD, "Connected");
        else if (active) pcStat.show(Tone.WAITING, "Not yet");
        else pcStat.show(Tone.OFF, "\u2014");

        if (active && phase == Phase.STREAMING) {
            cameraStat.show(Tone.GOOD, Settings.getCameraFps(this) + " fps");
        } else if (active) {
            cameraStat.show(Tone.WAITING, "Waiting");
        } else {
            cameraStat.show(Tone.OFF, "Off");
        }

        EyePipeline.EyeStatus eye = CameraStreamService.getEyeStatus();
        String problem = null;
        if ((active || finishing) && eye != null) {
            switch (eye.state) {
                case EyePipeline.STATE_RUNNING: eyeStat.show(Tone.GOOD, "Tracking"); break;
                case EyePipeline.STATE_STARTING: eyeStat.show(Tone.WAITING, "Starting"); break;
                case EyePipeline.STATE_RESTORING: eyeStat.show(Tone.WAITING, "Restoring"); break;
                case EyePipeline.STATE_ERROR:
                    eyeStat.show(Tone.PROBLEM, "Not working");
                    problem = eye.message;
                    break;
                default: eyeStat.show(Tone.OFF, "Off"); break;
            }
        } else {
            eyeStat.show(Tone.OFF, Settings.isEyeEnabled(this) ? "On" : "Off");
        }
        eyeProblem.setText(problem == null ? "" : problem);
        eyeProblem.setVisibility(problem == null ? View.GONE : View.VISIBLE);
    }

    /** The word over the headline, as the desktop app says it. */
    private static String stateWord(Tone tone) {
        switch (tone) {
            case GOOD: return "LIVE";
            case WAITING: return "WORKING";
            case PROBLEM: return "NEEDS YOU";
            default: return "STOPPED";
        }
    }

    /** One part of the stream: a label over a mark and a value. */
    private final class Stat {
        final LinearLayout view = new LinearLayout(MainActivity.this);
        private final Ui.StatusMark mark = new Ui.StatusMark(MainActivity.this);
        private final TextView value = ui.text("", ui.medium, 15, Ui.TEXT);

        Stat(String label) {
            view.setOrientation(LinearLayout.VERTICAL);
            view.addView(ui.cap(label));
            LinearLayout row = new LinearLayout(MainActivity.this);
            row.setOrientation(LinearLayout.HORIZONTAL);
            row.setGravity(Gravity.CENTER_VERTICAL);
            row.addView(mark);
            LinearLayout.LayoutParams gap = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            gap.leftMargin = ui.dp(8);
            row.addView(value, gap);
            view.addView(row, spaced(10));
        }

        void show(Tone tone, String text) {
            mark.setTone(tone);
            value.setText(text);
            value.setTextColor(tone == Tone.OFF ? Ui.TEXT_2 : Ui.TEXT);
        }
    }

    private void startStream() {
        startForegroundService(new Intent(this, CameraStreamService.class)
                .setAction(CameraStreamService.ACTION_START));
        handler.postDelayed(this::showStatus, 100);
    }

    private void stopStream() {
        startService(new Intent(this, CameraStreamService.class)
                .setAction(CameraStreamService.ACTION_STOP));
        handler.postDelayed(this::showStatus, 100);
    }

    /**
     * Stops the stream and starts it again once it has stopped, so it picks
     * up the eye gaze setting; with eye gaze off, that start puts Meta's eye
     * model back. A second change while this waits is read by the same start.
     */
    private void restartStream() {
        if (restarting) return;
        restarting = true;
        Context app = getApplicationContext();
        app.startService(new Intent(app, CameraStreamService.class)
                .setAction(CameraStreamService.ACTION_STOP));
        new Thread(() -> {
            long deadline = SystemClock.elapsedRealtime() + RESTART_TIMEOUT_MS;
            while ((CameraStreamService.isActive() || CameraStreamService.isFinishing())
                    && SystemClock.elapsedRealtime() < deadline) {
                SystemClock.sleep(200);
            }
            handler.post(() -> {
                restarting = false;
                app.startForegroundService(new Intent(app, CameraStreamService.class)
                        .setAction(CameraStreamService.ACTION_START));
            });
        }, "restart-stream").start();
    }

    /**
     * Puts Meta's eye model back now, when eye gaze is turned off between
     * streams; a stream leaves the per-eye model in place for the next.
     */
    private void restoreEyeModel() {
        Context app = getApplicationContext();
        new Thread(() -> new EyePipeline(app, null, null).restore(), "restore-eye").start();
    }

    /**
     * A single-top {@code am start} reaches the open panel here with its
     * extras; without single-top, Horizon OS only brings the panel to the
     * front and drops them.
     */
    @Override protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        applySettingExtras(intent);
        handleProbeExtras(intent);
    }

    private void handleProbeExtras(Intent intent) {
        if (intent.getBooleanExtra("start_probe", false)) startStream();
        if (intent.getBooleanExtra("stop_probe", false)) stopStream();
        if (BuildConfig.DEBUG && intent.hasExtra("capture_width")) {
            int width = intent.getIntExtra("capture_width", 1280);
            handler.postDelayed(() -> capture(width), 1500);
        }
    }

    /**
     * For checking the layout in development, since a Quest screencap is
     * empty: {@code --ei capture_width 1280} draws the panel at that width to
     * {@code files/panel.png} in the app's external storage. Debug builds only.
     */
    private void capture(int width) {
        View content = ((ViewGroup) findViewById(android.R.id.content)).getChildAt(0);
        View page = ((ViewGroup) content).getChildAt(0);
        page.measure(View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(0, View.MeasureSpec.UNSPECIFIED));
        page.layout(0, 0, page.getMeasuredWidth(), page.getMeasuredHeight());
        android.graphics.Bitmap bitmap = android.graphics.Bitmap.createBitmap(
                page.getMeasuredWidth(), page.getMeasuredHeight(),
                android.graphics.Bitmap.Config.ARGB_8888);
        android.graphics.Canvas canvas = new android.graphics.Canvas(bitmap);
        canvas.drawColor(Ui.PAGE);
        page.draw(canvas);
        java.io.File out = new java.io.File(getExternalFilesDir(null), "panel.png");
        try (java.io.FileOutputStream stream = new java.io.FileOutputStream(out)) {
            bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, stream);
        } catch (java.io.IOException error) {
            android.util.Log.e("VRFTCamera", "Capture failed: " + error);
        }
        content.requestLayout();
    }

    /**
     * Development control over ADB, alongside {@code start_probe}, e.g.
     * {@code am start -f 0x24000000 -n .../.MainActivity --ez eye_enabled true
     * --ei camera_fps 24 --ei eye_preview_fps 5 --ez start_probe true}.
     * The settings are those of {@link Settings#applyExtras}.
     * {@code --ez five_cameras true} turns on the five-camera stream, as its
     * switch does. {@code --ez eye_alt_probe true} has no control: it picks the alternative
     * eye probe (see the README) from the next stream start.
     */
    private void applySettingExtras(Intent intent) {
        Settings.applyExtras(this, intent);
        // A start in the same intent puts it back itself.
        if (intent.hasExtra("eye_enabled") && !intent.getBooleanExtra("eye_enabled", false)
                && !CameraStreamService.isActive()
                && !intent.getBooleanExtra("start_probe", false)) {
            restoreEyeModel();
        }
    }

    private static String[] labelsFor(int[] values, String zeroLabel) {
        String[] labels = new String[values.length];
        for (int i = 0; i < values.length; i++) {
            labels[i] = (values[i] == 0 && zeroLabel != null)
                    ? zeroLabel : Integer.toString(values[i]);
        }
        return labels;
    }

    private static int indexOf(int[] values, int value) {
        for (int i = 0; i < values.length; i++) if (values[i] == value) return i;
        return 0;
    }

    /**
     * A column that stops growing at a width and stays centred beyond it, and
     * keeps narrower side margins on a narrow panel.
     */
    private static final class MaxWidthColumn extends LinearLayout {
        private final int maxWidth;
        private final float unit;

        MaxWidthColumn(Context context, int maxWidth) {
            super(context);
            this.maxWidth = maxWidth;
            unit = context.getResources().getDisplayMetrics().density;
        }

        @Override protected void onMeasure(int widthSpec, int heightSpec) {
            int width = MeasureSpec.getSize(widthSpec);
            int side = Math.round((width < 480 * unit ? 16 : 32) * unit);
            if (getPaddingLeft() != side) {
                setPadding(side, getPaddingTop(), side, getPaddingBottom());
            }
            if (width > maxWidth) {
                widthSpec = MeasureSpec.makeMeasureSpec(maxWidth, MeasureSpec.getMode(widthSpec));
            }
            super.onMeasure(widthSpec, heightSpec);
        }
    }

    @Override protected void onResume() {
        super.onResume();
        handler.post(refresh);
    }

    @Override protected void onPause() {
        handler.removeCallbacks(refresh);
        super.onPause();
    }
}
