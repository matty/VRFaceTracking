package io.github.matty.vrft.questprocamera;

import android.content.Context;
import android.content.SharedPreferences;

/**
 * User-facing stream settings, stored in {@link SharedPreferences} and applied
 * at the next stream start (never live; the panel restarts a running stream
 * when eye gaze changes). Values are validated against the
 * allowed choices so a corrupt preference cannot feed a bad relay argument.
 */
public final class Settings {
    public static final String PREFS = "vrft_camera";

    private static final String KEY_CAMERA_FPS = "camera_fps";
    private static final String KEY_EYE_PREVIEW_FPS = "eye_snapshot_fps";
    /**
     * Where the rate was kept while 2 fps was the default. The spinner saves
     * its value when the panel first opens, so a 2 there was most likely
     * never chosen and moves to the new default; anything else carries over.
     */
    private static final String LEGACY_KEY_EYE_PREVIEW_FPS = "eye_preview_fps";
    private static final int LEGACY_DEFAULT_EYE_PREVIEW_FPS = 2;
    private static final String KEY_EYE_ENABLED = "eye_enabled";

    /** Camera FPS choices (relay {@code --max-fps}); default is 24. */
    public static final int[] CAMERA_FPS_CHOICES = {12, 15, 20, 24, 30, 36};
    public static final int DEFAULT_CAMERA_FPS = 24;

    /**
     * Eye-camera snapshot FPS choices (relay {@code --eye-fps}); 0 = Off,
     * default 5. The PC measures pupil size from the snapshots.
     */
    public static final int[] EYE_PREVIEW_FPS_CHOICES = {0, 1, 2, 5};
    public static final int DEFAULT_EYE_PREVIEW_FPS = 5;

    /**
     * Independent eye gaze, on unless turned off. On firmware it doesn't
     * support, the stream carries on without it.
     */
    public static final boolean DEFAULT_EYE_ENABLED = true;

    private Settings() { }

    private static SharedPreferences prefs(Context context) {
        return context.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }

    public static int getCameraFps(Context context) {
        return clampToChoices(prefs(context).getInt(KEY_CAMERA_FPS, DEFAULT_CAMERA_FPS),
                CAMERA_FPS_CHOICES, DEFAULT_CAMERA_FPS);
    }

    public static void setCameraFps(Context context, int fps) {
        prefs(context).edit().putInt(KEY_CAMERA_FPS,
                clampToChoices(fps, CAMERA_FPS_CHOICES, DEFAULT_CAMERA_FPS)).apply();
    }

    public static int getEyePreviewFps(Context context) {
        SharedPreferences prefs = prefs(context);
        int fallback = DEFAULT_EYE_PREVIEW_FPS;
        if (!prefs.contains(KEY_EYE_PREVIEW_FPS) && prefs.contains(LEGACY_KEY_EYE_PREVIEW_FPS)) {
            int legacy = prefs.getInt(LEGACY_KEY_EYE_PREVIEW_FPS, LEGACY_DEFAULT_EYE_PREVIEW_FPS);
            if (legacy != LEGACY_DEFAULT_EYE_PREVIEW_FPS) fallback = legacy;
        }
        return clampToChoices(prefs.getInt(KEY_EYE_PREVIEW_FPS, fallback),
                EYE_PREVIEW_FPS_CHOICES, DEFAULT_EYE_PREVIEW_FPS);
    }

    public static void setEyePreviewFps(Context context, int fps) {
        prefs(context).edit().putInt(KEY_EYE_PREVIEW_FPS,
                clampToChoices(fps, EYE_PREVIEW_FPS_CHOICES, DEFAULT_EYE_PREVIEW_FPS)).apply();
    }

    public static boolean isEyeEnabled(Context context) {
        return prefs(context).getBoolean(KEY_EYE_ENABLED, DEFAULT_EYE_ENABLED);
    }

    public static void setEyeEnabled(Context context, boolean enabled) {
        prefs(context).edit().putBoolean(KEY_EYE_ENABLED, enabled).apply();
    }

    private static int clampToChoices(int value, int[] choices, int fallback) {
        for (int choice : choices) if (choice == value) return value;
        return fallback;
    }
}
