package io.github.matty.vrft.questprocamera;

import android.app.Activity;
import android.content.Intent;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.view.View;
import android.widget.AdapterView;
import android.widget.ArrayAdapter;
import android.widget.Button;
import android.widget.CheckBox;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.Spinner;
import android.widget.TextView;

/** Control panel for the camera service. Virtual Desktop can remain in front. */
public final class MainActivity extends Activity {
    private final Handler handler = new Handler(Looper.getMainLooper());
    private TextView status;
    private Button startButton;
    private Button stopButton;
    private final Runnable refresh = new Runnable() {
        @Override public void run() {
            status.setText(CameraStreamService.getStatus());
            handler.postDelayed(this, 500);
        }
    };

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        applySettingExtras(getIntent());
        LinearLayout column = new LinearLayout(this);
        column.setOrientation(LinearLayout.VERTICAL);
        column.setPadding(24, 24, 24, 24);
        ScrollView scroll = new ScrollView(this);
        scroll.addView(column);
        setContentView(scroll);

        TextView title = new TextView(this);
        title.setText("Quest Pro camera stream");
        title.setTextSize(22);
        column.addView(title);
        TextView note = new TextView(this);
        note.setText("Start streaming, then open Virtual Desktop. The VRFT Rust app discovers this headset automatically on the local network and shows the cameras at http://127.0.0.1:27275/ on your PC.");
        column.addView(note);

        addLabel(column, "Camera FPS");
        Spinner cameraFps = addSpinner(column, labelsFor(Settings.CAMERA_FPS_CHOICES, null),
                indexOf(Settings.CAMERA_FPS_CHOICES, Settings.getCameraFps(this)));
        cameraFps.setOnItemSelectedListener(new SelectionListener(
                position -> Settings.setCameraFps(this, Settings.CAMERA_FPS_CHOICES[position])));

        addLabel(column, "Eye-camera preview snapshots (fps)");
        Spinner eyePreviewFps = addSpinner(column,
                labelsFor(Settings.EYE_PREVIEW_FPS_CHOICES, "Off"),
                indexOf(Settings.EYE_PREVIEW_FPS_CHOICES, Settings.getEyePreviewFps(this)));
        eyePreviewFps.setOnItemSelectedListener(new SelectionListener(
                position -> Settings.setEyePreviewFps(this, Settings.EYE_PREVIEW_FPS_CHOICES[position])));

        CheckBox eyeGaze = new CheckBox(this);
        eyeGaze.setText("Independent eye gaze (temporarily replaces Meta's eye model while streaming)");
        eyeGaze.setChecked(Settings.isEyeEnabled(this));
        eyeGaze.setOnCheckedChangeListener((view, checked) -> Settings.setEyeEnabled(this, checked));
        column.addView(eyeGaze);
        TextView eyeWarning = new TextView(this);
        eyeWarning.setText("When on, streaming temporarily replaces Meta's eye model to expose per-eye gaze, and restores the stock model when you press Stop.");
        eyeWarning.setTextSize(12);
        column.addView(eyeWarning);

        TextView applyNote = new TextView(this);
        applyNote.setText("Settings apply on the next stream start.");
        applyNote.setTextSize(12);
        column.addView(applyNote);

        Button start = new Button(this);
        start.setText("Start camera stream");
        start.setOnClickListener(view -> startForegroundService(
                new Intent(this, CameraStreamService.class).setAction(CameraStreamService.ACTION_START)));
        column.addView(start);
        Button stop = new Button(this);
        stop.setText("Stop camera stream");
        stop.setOnClickListener(view -> startService(
                new Intent(this, CameraStreamService.class).setAction(CameraStreamService.ACTION_STOP)));
        column.addView(stop);
        Button virtualDesktop = new Button(this);
        virtualDesktop.setText("Open Virtual Desktop");
        virtualDesktop.setOnClickListener(view -> {
            Intent launch = getPackageManager().getLaunchIntentForPackage("VirtualDesktop.Android");
            if (launch != null) startActivity(launch);
            else status.setText("Virtual Desktop is not installed or cannot be launched.");
        });
        column.addView(virtualDesktop);
        status = new TextView(this);
        status.setText(CameraStreamService.getStatus());
        status.setTextSize(16);
        column.addView(status);
        startButton = start;
        stopButton = stop;
        handleProbeExtras(getIntent());
    }

    /** Quest reuses the open panel for a new {@code am start}, so extras arrive here. */
    @Override protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        applySettingExtras(intent);
        handleProbeExtras(intent);
    }

    private void handleProbeExtras(Intent intent) {
        if (intent.getBooleanExtra("start_probe", false)) startButton.performClick();
        if (intent.getBooleanExtra("stop_probe", false)) stopButton.performClick();
    }

    /**
     * Development control over ADB, alongside {@code start_probe}, e.g.
     * {@code am start -f 0x24000000 -n .../.MainActivity --ez eye_enabled true
     * --ei camera_fps 24 --ei eye_preview_fps 2 --ez start_probe true}.
     * Values go through the same validation as the on-screen controls.
     */
    private void applySettingExtras(Intent intent) {
        if (intent.hasExtra("eye_enabled")) {
            Settings.setEyeEnabled(this, intent.getBooleanExtra("eye_enabled", false));
        }
        if (intent.hasExtra("camera_fps")) {
            Settings.setCameraFps(this, intent.getIntExtra("camera_fps", Settings.DEFAULT_CAMERA_FPS));
        }
        if (intent.hasExtra("eye_preview_fps")) {
            Settings.setEyePreviewFps(this,
                    intent.getIntExtra("eye_preview_fps", Settings.DEFAULT_EYE_PREVIEW_FPS));
        }
    }

    private void addLabel(LinearLayout column, String text) {
        TextView label = new TextView(this);
        label.setText(text);
        label.setTextSize(14);
        column.addView(label);
    }

    private Spinner addSpinner(LinearLayout column, String[] labels, int selected) {
        Spinner spinner = new Spinner(this);
        ArrayAdapter<String> adapter = new ArrayAdapter<>(this,
                android.R.layout.simple_spinner_dropdown_item, labels);
        spinner.setAdapter(adapter);
        if (selected >= 0) spinner.setSelection(selected);
        column.addView(spinner);
        return spinner;
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

    private interface OnSelected {
        void onSelected(int position);
    }

    private static final class SelectionListener implements AdapterView.OnItemSelectedListener {
        private final OnSelected callback;

        SelectionListener(OnSelected callback) { this.callback = callback; }

        @Override public void onItemSelected(AdapterView<?> parent, View view, int position, long id) {
            callback.onSelected(position);
        }

        @Override public void onNothingSelected(AdapterView<?> parent) { }
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
