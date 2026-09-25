package io.github.matty.vrft.questprocamera;

import android.content.Context;
import android.content.SharedPreferences;

/**
 * User-facing stream settings, stored in {@link SharedPreferences} and applied
 * at the next stream start (never live). Values are validated against the
 * allowed choices so a corrupt preference cannot feed a bad relay argument.
 */
public final class Settings {
    public static final String PREFS = "vrft_camera";

    private static final String KEY_CAMERA_FPS = "camera_fps";
    private static final String KEY_EYE_PREVIEW_FPS = "eye_preview_fps";
    private static final String KEY_EYE_ENABLED = "eye_enabled";

    /** Camera FPS choices (relay {@code --max-fps}); default is 24. */
    public static final int[] CAMERA_FPS_CHOICES = {12, 15, 20, 24, 30, 36};
    public static final int DEFAULT_CAMERA_FPS = 24;

    /** Eye-preview FPS choices (relay {@code --eye-fps}); 0 = Off, default 2. */
    public static final int[] EYE_PREVIEW_FPS_CHOICES = {0, 1, 2, 5};
    public static final int DEFAULT_EYE_PREVIEW_FPS = 2;

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
        return clampToChoices(prefs(context).getInt(KEY_EYE_PREVIEW_FPS, DEFAULT_EYE_PREVIEW_FPS),
                EYE_PREVIEW_FPS_CHOICES, DEFAULT_EYE_PREVIEW_FPS);
    }

    public static void setEyePreviewFps(Context context, int fps) {
        prefs(context).edit().putInt(KEY_EYE_PREVIEW_FPS,
                clampToChoices(fps, EYE_PREVIEW_FPS_CHOICES, DEFAULT_EYE_PREVIEW_FPS)).apply();
    }

    public static boolean isEyeEnabled(Context context) {
        return prefs(context).getBoolean(KEY_EYE_ENABLED, false);
    }

    public static void setEyeEnabled(Context context, boolean enabled) {
        prefs(context).edit().putBoolean(KEY_EYE_ENABLED, enabled).apply();
    }

    private static int clampToChoices(int value, int[] choices, int fallback) {
        for (int choice : choices) if (choice == value) return value;
        return fallback;
    }
}
