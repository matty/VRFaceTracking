package io.github.matty.vrft.questprocamera;

import android.content.Context;
import android.content.SharedPreferences;
import android.util.Base64;
import android.util.Log;

import java.io.BufferedReader;
import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicLong;

/**
 * Headset eye-gaze pipeline: temporarily replaces Meta's stock eye model with a
 * byte-length-preserving patch that keeps the two eyes on independent visual
 * axes, taps the tracking engine's per-eye detector output with a uprobe, and
 * streams paired raw vectors to the PC. A port of the fork's
 * {@code native-eye-local-branch-test.ps1} / {@code prepare-eye-model.ps1} /
 * {@code native_raw_eye_probe.py} into the APK.
 *
 * <p>Safety: every root command runs under Magisk {@code su --mount-master -c}
 * (global mount namespace) with a timeout; restore state is persisted before
 * mounting; any failure after mounting attempts a full restore; the engine is
 * matched by size (profile 2 also by sha256); paths are quoted. There is never
 * a fallback to plain {@code su} for mount operations.
 */
public final class EyePipeline {
    private static final String TAG = "VRFTCamera";

    static final String ENGINE = "/odm/lib64/libtrackingengines.so";
    static final String TARGET =
            "/odm/etc/eyetracking/runtime/models/Seacliff_V1_5/fbnet/int8/experimental/bolt/bolt.ptl";
    static final String PATCHED = "/data/local/tmp/vrft-camera/bolt-independent-axes.ptl";
    private static final String ROOT_DIR = "/data/local/tmp/vrft-camera";
    static final String PROPERTY =
            "persist.device_config.oculus_shared_vision.oculus_eyetracking_enable_experimental_model";
    private static final String SELINUX_CONTEXT = "u:object_r:vendor_configs_file:s0";

    private static final String TRACE_ROOT = "/sys/kernel/tracing";
    private static final String INSTANCE = "vrft_eye";
    private static final String GROUP = "vrft_eye";
    private static final String EVENT = "detector_output";
    private static final String INSTANCE_PATH = TRACE_ROOT + "/instances/" + INSTANCE;

    private static final String RESTORE_PREFS = "vrft_eye_restore";
    private static final String KEY_RESTORE_PENDING = "restore_pending";
    private static final String KEY_ORIG_PROP = "orig_prop";

    private static final int MIN_MODEL_BYTES = 100000;

    static final String STATE_OFF = "off";
    static final String STATE_STARTING = "starting";
    static final String STATE_RUNNING = "running";
    static final String STATE_ERROR = "error";
    static final String STATE_RESTORING = "restoring";

    /** Supported tracking-engine builds, selected by {@code stat -c %s}. */
    static final class Profile {
        final int id;
        final long size;
        final long offset;
        final String args;
        final String sha256; // required only for profile 2; null otherwise

        Profile(int id, long size, long offset, String args, String sha256) {
            this.id = id;
            this.size = size;
            this.offset = offset;
            this.args = args;
            this.sha256 = sha256;
        }
    }

    static final Profile[] PROFILES = {
            new Profile(1, 47_724_232L, 0xB63FE4L,
                    "x=+0x30(%sp):x32 y=+0x34(%sp):x32 z=+0x38(%sp):x32 tag=+0x0(%x19):x32",
                    null),
            new Profile(2, 47_418_280L, 0xB1F3E8L,
                    "x=+0x300(%x19):x32 y=+0x304(%x19):x32 z=+0x308(%x19):x32 tag=+0x0(%x19):x32",
                    "0fb6f54a3e190bec791d757ea18d32a8ecc1af4a861992d04b1703c93293cd03"),
    };

    /** Immutable snapshot of the eye pipeline's status, mirrored into QPSTAT1. */
    public static final class EyeStatus {
        public final boolean enabled;
        public final String state;
        public final String message;
        public final int engineProfile; // 0 = unknown
        public final boolean modelPatched;
        public final boolean restorePending;

        EyeStatus(boolean enabled, String state, String message, int engineProfile,
                  boolean modelPatched, boolean restorePending) {
            this.enabled = enabled;
            this.state = state;
            this.message = message;
            this.engineProfile = engineProfile;
            this.modelPatched = modelPatched;
            this.restorePending = restorePending;
        }

        /** Ordered map for the {@code eye} object inside QPSTAT1. */
        public java.util.Map<String, Object> toMap() {
            java.util.Map<String, Object> map = new java.util.LinkedHashMap<>();
            map.put("enabled", enabled);
            map.put("state", state);
            map.put("message", message);
            if (engineProfile > 0) map.put("engine_profile", (long) engineProfile);
            map.put("model_patched", modelPatched);
            map.put("restore_pending", restorePending);
            return map;
        }
    }

    /** Notified whenever the eye status changes, so the service resends QPSTAT1. */
    public interface StatusListener {
        void onEyeStatusChanged();
    }

    /** Sink for encoded QPGAZE1 packets, written by the service under its lock. */
    public interface GazeSink {
        void sendGazePacket(byte[] packet);
    }

    private final Context context;
    private final StatusListener statusListener;
    private final GazeSink gazeSink;
    private final AtomicLong gazeSequence = new AtomicLong();
    /**
     * Serializes recover/start/stop. Stop can be requested from another thread
     * while start is still applying the patch; without this, a restore could
     * run in the middle of start, which would then mount again after the
     * restore state had been cleared.
     */
    private final Object lifecycle = new Object();

    private volatile EyeStatus status;
    private volatile int engineProfileId;
    private volatile boolean modelActive;
    private volatile boolean stopping;
    private volatile Process traceCat;
    private Thread traceThread;

    public EyePipeline(Context context, StatusListener statusListener, GazeSink gazeSink) {
        this.context = context;
        this.statusListener = statusListener;
        this.gazeSink = gazeSink;
        this.status = new EyeStatus(false, STATE_OFF, "Eye gaze off", 0, false,
                restorePendingFlag());
    }

    public EyeStatus getStatus() { return status; }

    public boolean isEnabled() { return Settings.isEyeEnabled(context); }

    // ---- lifecycle -------------------------------------------------------

    /** Crash recovery: if a previous run left a mount, restore it at start. */
    public void recoverIfNeeded() {
        synchronized (lifecycle) {
            if (!restorePendingFlag()) return;
            setStatus(STATE_RESTORING, "Restoring stock eye model after a previous run",
                    engineProfileId, modelActive, true);
            if (!mountRootAvailable()) {
                setStatus(STATE_ERROR,
                        "Cannot restore stock eye model: su --mount-master root unavailable",
                        engineProfileId, modelActive, true);
                return;
            }
            doRestore();
        }
    }

    /**
     * Apply the patch and start tracing. Returns {@code true} when gaze is
     * streaming. On any failure it restores whatever was applied, sets an
     * error status, and returns {@code false}; camera streaming continues.
     * Each pipeline instance is started at most once; after {@link #stop()}
     * it refuses to start.
     */
    public boolean start() {
        synchronized (lifecycle) {
            if (stopping) return false;
            gazeSequence.set(0);
            modelActive = false;
            engineProfileId = 0;
            setStatus(STATE_STARTING, "Preparing independent eye gaze", 0, false, false);
            try {
                if (!mountRootAvailable()) {
                    setStatus(STATE_ERROR,
                            "su --mount-master did not grant root; eye gaze disabled",
                            0, false, false);
                    return false;
                }
                Profile profile = detectProfile();
                engineProfileId = profile.id;

                String mounts = rootAllow("grep -F " + quote(TARGET) + " /proc/mounts", 15).out;
                if (!mounts.trim().isEmpty()) {
                    setStatus(STATE_ERROR,
                            "Another tool already has an eye model mounted", profile.id,
                            false, false);
                    return false;
                }

                String warning = applyAndTrace(profile);
                setStatus(STATE_RUNNING,
                        warning != null ? warning : "Independent eye gaze active",
                        profile.id, modelActive, true);
                return true;
            } catch (EyeException expected) {
                setStatus(STATE_ERROR, expected.getMessage(), engineProfileId, false,
                        restorePendingFlag());
                return false;
            } catch (Exception unexpected) {
                setStatus(STATE_ERROR, "Eye pipeline failed: " + unexpected.getMessage(),
                        engineProfileId, false, restorePendingFlag());
                return false;
            }
        }
    }

    /** Stop tracing and restore the stock model. Safe to call more than once. */
    public void stop() {
        // Flag first so a start() still applying the patch aborts at its next
        // checkpoint and restores; then wait for it before restoring here.
        stopping = true;
        synchronized (lifecycle) {
            if (restorePendingFlag() || modelActive) {
                if (!mountRootAvailable()) {
                    setStatus(STATE_ERROR,
                            "Cannot restore stock eye model: su --mount-master root unavailable",
                            engineProfileId, modelActive, true);
                    return;
                }
                doRestore();
            } else {
                destroyTraceCat();
                setStatus(STATE_OFF, "Eye gaze off", 0, false, false);
            }
        }
    }

    private void abortIfStopping() throws EyeException {
        if (stopping) throw new EyeException("Eye gaze stopped during startup");
    }

    // ---- apply sequence (SPEC steps 4-9) ---------------------------------

    /** @return a warning for the running status, or null when fully verified. */
    private String applyAndTrace(Profile profile) throws Exception {
        setStatus(STATE_STARTING, "Reading stock eye model", profile.id, false, false);
        byte[] stock = rootBinary("cat " + quote(TARGET), 30);
        if (stock.length < MIN_MODEL_BYTES) {
            throw new EyeException("Stock eye model too small (" + stock.length + " bytes)");
        }
        ModelPatcher.Result patched;
        try {
            patched = ModelPatcher.patch(stock);
        } catch (ModelPatcher.ModelPatchException refusal) {
            throw new EyeException("Model patch refused: " + refusal.getMessage());
        }

        File appFile = new File(context.getFilesDir(), "bolt-independent-axes.ptl");
        writeFile(appFile, patched.patched);
        String appPath = appFile.getAbsolutePath();
        rootChecked("mkdir -p " + quote(ROOT_DIR)
                + " && cp " + quote(appPath) + " " + quote(PATCHED + ".new")
                + " && mv -f " + quote(PATCHED + ".new") + " " + quote(PATCHED)
                + " && chown root:root " + quote(PATCHED)
                + " && chmod 0644 " + quote(PATCHED)
                + " && chcon " + SELINUX_CONTEXT + " " + quote(PATCHED), 30);

        abortIfStopping();
        // Persist restore state BEFORE mounting (commit, not apply).
        String orig = rootAllow("getprop " + PROPERTY, 15).out.trim();
        if (orig.isEmpty()) orig = "false";
        persistRestore(true, orig);
        setStatus(STATE_STARTING, "Mounting patched eye model", profile.id, false, true);

        boolean traced = false;
        try {
            rootAllow("umount " + quote(TARGET), 15);
            rootChecked("mount --bind " + quote(PATCHED) + " " + quote(TARGET), 15);
            String mountedSum = firstToken(
                    rootChecked("sha256sum " + quote(TARGET), 30).trim()).toLowerCase();
            if (!mountedSum.equals(patched.sha256)) {
                throw new EyeException("Mounted eye model failed its hash check");
            }
            abortIfStopping();
            rootChecked("setprop " + PROPERTY + " true", 15);
            rootChecked("stop trackingservice", 15);
            rootChecked("start trackingservice", 15);
            waitTrackingRunning();
            // Mounted and the tracking service is back. Gaze packets carry bit 2
            // only if the service can actually see the patched model; otherwise
            // both eyes still follow Meta's blended gaze.
            MountCheck check = verifyMountVisible();
            modelActive = check != MountCheck.NOT_VISIBLE;
            abortIfStopping();
            startTrace(profile);
            traced = true;
            switch (check) {
                case NOT_VISIBLE:
                    return "Tracking service cannot see the patched model; streaming "
                            + "Meta's blended gaze (no convergence)";
                case UNVERIFIED:
                    return "Independent eye gaze active (tracking service pid not found; "
                            + "patched model visibility unverified)";
                default:
                    return null;
            }
        } catch (Exception failure) {
            if (traced) safeTraceCleanup();
            try {
                doRestore();
            } catch (Exception ignored) {
                // best effort; original failure is the one that matters
            }
            if (failure instanceof EyeException) throw failure;
            throw new EyeException("Eye pipeline failed: " + failure.getMessage());
        }
    }

    private void startTrace(Profile profile) throws Exception {
        safeTraceCleanup(); // clear any stale instance first
        rootChecked("mkdir " + INSTANCE_PATH, 15);
        String event = "p:" + GROUP + "/" + EVENT + " " + ENGINE + ":0x"
                + Long.toHexString(profile.offset) + " " + profile.args;
        String encoded = Base64.encodeToString(event.getBytes(StandardCharsets.UTF_8),
                Base64.NO_WRAP);
        rootChecked("echo " + encoded + " | base64 -d >> " + TRACE_ROOT + "/uprobe_events", 15);
        rootChecked("echo 1 > " + INSTANCE_PATH + "/events/" + GROUP + "/" + EVENT + "/enable", 15);
        rootChecked("echo 1 > " + INSTANCE_PATH + "/tracing_on", 15);

        Process cat = new ProcessBuilder("su", "--mount-master", "-c",
                "cat " + INSTANCE_PATH + "/trace_pipe").start();
        traceCat = cat;
        traceThread = new Thread(() -> readTrace(cat, profile), "eye-trace");
        traceThread.setDaemon(true);
        traceThread.start();
    }

    private void readTrace(Process cat, Profile profile) {
        TraceParser parser = new TraceParser();
        try (BufferedReader reader = new BufferedReader(
                new InputStreamReader(cat.getInputStream(), StandardCharsets.UTF_8))) {
            String line;
            while (!stopping && (line = reader.readLine()) != null) {
                TraceParser.GazePair pair = parser.parse(line);
                if (pair == null) continue;
                long sequence = gazeSequence.incrementAndGet();
                int flags = (pair.tag0Valid ? GazePackets.FLAG_TAG0_VALID : 0)
                        | (pair.tag1Valid ? GazePackets.FLAG_TAG1_VALID : 0)
                        | (modelActive ? GazePackets.FLAG_MODEL_ACTIVE : 0);
                byte[] packet = GazePackets.encodeGaze(sequence, pair.kernelTimeNs,
                        flags, profile.id, pair.tag0, pair.tag1);
                gazeSink.sendGazePacket(packet);
            }
        } catch (IOException stopped) {
            if (!stopping) Log.i(TAG, "Eye trace reader ended: " + stopped);
        }
    }

    // ---- restore / cleanup ----------------------------------------------

    private void doRestore() {
        setStatus(STATE_RESTORING, "Restoring stock eye model", engineProfileId,
                modelActive, true);
        safeTraceCleanup();
        String orig = readOrigProp();
        rootAllow("stop trackingservice", 15);
        rootAllow("setprop " + PROPERTY + " " + orig, 15);
        rootAllow("umount " + quote(TARGET), 15);
        rootAllow("start trackingservice", 15);
        try {
            waitTrackingRunning();
        } catch (Exception ignored) {
            // service will come back on its own; nothing more we can do here
        }
        rootAllow("rm -f " + quote(PATCHED), 15);
        persistRestore(false, "false");
        modelActive = false;
        engineProfileId = 0;
        setStatus(STATE_OFF, "Stock eye model restored", 0, false, false);
    }

    private void safeTraceCleanup() {
        destroyTraceCat();
        rootAllow("echo 0 > " + INSTANCE_PATH + "/tracing_on", 5);
        rootAllow("echo 0 > " + INSTANCE_PATH + "/events/" + GROUP + "/" + EVENT + "/enable", 5);
        rootAllow("echo '-:" + GROUP + "/" + EVENT + "' >> " + TRACE_ROOT + "/uprobe_events", 5);
        rootAllow("for p in $(pidof cat); do grep -q '" + INSTANCE_PATH
                + "/trace_pipe' /proc/$p/cmdline && kill $p; done", 5);
        for (int attempt = 0; attempt < 8; attempt++) {
            rootAllow("echo 1 > " + INSTANCE_PATH + "/free_buffer", 5);
            RootResult result = rootAllow(
                    "rmdir " + INSTANCE_PATH + "; test ! -d " + INSTANCE_PATH, 5);
            if (result.exit == 0) break;
            sleepMillis(150);
        }
    }

    private void destroyTraceCat() {
        Process cat = traceCat;
        traceCat = null;
        if (cat != null) {
            cat.destroy();
            try {
                if (!cat.waitFor(1, TimeUnit.SECONDS)) cat.destroyForcibly();
            } catch (InterruptedException interrupted) {
                cat.destroyForcibly();
                Thread.currentThread().interrupt();
            }
        }
        Thread thread = traceThread;
        if (thread != null) {
            try {
                thread.join(1000);
            } catch (InterruptedException interrupted) {
                Thread.currentThread().interrupt();
            }
        }
    }

    // ---- engine profile / verification ----------------------------------

    private Profile detectProfile() throws Exception {
        String statOut = rootChecked("stat -c %s " + quote(ENGINE), 15).trim();
        long size;
        try {
            size = Long.parseLong(lastToken(statOut));
        } catch (NumberFormatException bad) {
            throw new EyeException("Could not read tracking-engine size");
        }
        Profile profile = null;
        for (Profile candidate : PROFILES) {
            if (candidate.size == size) { profile = candidate; break; }
        }
        if (profile == null) {
            throw new EyeException("Unsupported tracking-engine build (" + size + ")");
        }
        if (profile.sha256 != null) {
            String sum = firstToken(
                    rootChecked("sha256sum " + quote(ENGINE), 30).trim()).toLowerCase();
            if (!sum.equals(profile.sha256)) {
                throw new EyeException(
                        "Tracking-engine hash mismatch for build " + size);
            }
        }
        return profile;
    }

    private void waitTrackingRunning() throws Exception {
        for (int attempt = 0; attempt < 40; attempt++) {
            if ("running".equals(rootAllow("getprop init.svc.trackingservice", 10).out.trim())) {
                sleepMillis(1500);
                return;
            }
            sleepMillis(250);
        }
        throw new EyeException("Tracking service did not return to running");
    }

    private enum MountCheck { VISIBLE, NOT_VISIBLE, UNVERIFIED }

    /** Whether the running tracking service's mount namespace has our bind mount. */
    private MountCheck verifyMountVisible() {
        String pid = rootAllow("getprop init.svc_debug_pid.trackingservice", 10).out.trim();
        if (pid.isEmpty() || !pid.matches("\\d+")) {
            pid = firstToken(rootAllow("pidof trackingservice", 10).out.trim());
        }
        if (pid.isEmpty() || !pid.matches("\\d+")) return MountCheck.UNVERIFIED;
        String seen = rootAllow(
                "grep -F " + quote(TARGET) + " /proc/" + pid + "/mounts", 10).out;
        return seen.trim().isEmpty() ? MountCheck.NOT_VISIBLE : MountCheck.VISIBLE;
    }

    // ---- persistence -----------------------------------------------------

    private SharedPreferences restorePrefs() {
        return context.getSharedPreferences(RESTORE_PREFS, Context.MODE_PRIVATE);
    }

    private boolean restorePendingFlag() {
        return restorePrefs().getBoolean(KEY_RESTORE_PENDING, false);
    }

    private String readOrigProp() {
        String orig = restorePrefs().getString(KEY_ORIG_PROP, "false");
        return orig == null || orig.isEmpty() ? "false" : orig;
    }

    private void persistRestore(boolean pending, String orig) {
        restorePrefs().edit()
                .putBoolean(KEY_RESTORE_PENDING, pending)
                .putString(KEY_ORIG_PROP, orig)
                .commit();
    }

    // ---- status ----------------------------------------------------------

    private void setStatus(String state, String message, int engineProfile,
                           boolean modelPatched, boolean restorePending) {
        status = new EyeStatus(isEnabled(), state, message, engineProfile,
                modelPatched, restorePending);
        Log.i(TAG, "Eye: " + state + " - " + message);
        if (statusListener != null) statusListener.onEyeStatusChanged();
    }

    // ---- root execution --------------------------------------------------

    private static final class RootResult {
        final int exit;
        final String out;

        RootResult(int exit, String out) {
            this.exit = exit;
            this.out = out;
        }
    }

    private boolean mountRootAvailable() {
        RootResult result = rootAllow("id", 15);
        return result.exit == 0 && result.out.contains("uid=0");
    }

    /** Run a command under mount-master, throwing on non-zero exit or timeout. */
    private String rootChecked(String command, int timeoutSeconds) throws Exception {
        byte[][] streams = new byte[2][];
        int exit = execMountMaster(command, timeoutSeconds, true, streams);
        String text = new String(streams[0], StandardCharsets.UTF_8);
        if (exit != 0) {
            throw new EyeException("Root command failed (" + exit + "): "
                    + command + " -> " + text.trim());
        }
        return text;
    }

    /** Run a command under mount-master, never throwing (best effort). */
    private RootResult rootAllow(String command, int timeoutSeconds) {
        try {
            byte[][] streams = new byte[2][];
            int exit = execMountMaster(command, timeoutSeconds, true, streams);
            return new RootResult(exit, new String(streams[0], StandardCharsets.UTF_8));
        } catch (Exception failure) {
            return new RootResult(-1, "");
        }
    }

    /** Run a command under mount-master and return raw stdout bytes. */
    private byte[] rootBinary(String command, int timeoutSeconds) throws Exception {
        byte[][] streams = new byte[2][];
        int exit = execMountMaster(command, timeoutSeconds, false, streams);
        if (exit != 0) {
            throw new EyeException("Root command failed (" + exit + "): " + command
                    + " -> " + new String(streams[1], StandardCharsets.UTF_8).trim());
        }
        return streams[0];
    }

    /**
     * Execute {@code su --mount-master -c command}. When {@code merge} is true
     * stderr is folded into stdout (text commands); otherwise stdout and stderr
     * are captured separately (binary reads such as the model {@code cat}).
     * Fills {@code streams[0]} with stdout and {@code streams[1]} with stderr,
     * and returns the exit code. Always times out.
     */
    private int execMountMaster(String command, int timeoutSeconds, boolean merge,
                                byte[][] streams) throws Exception {
        Process process = new ProcessBuilder("su", "--mount-master", "-c", command)
                .redirectErrorStream(merge)
                .start();
        ByteArrayOutputStream stdout = new ByteArrayOutputStream();
        ByteArrayOutputStream stderr = new ByteArrayOutputStream();
        Thread outReader = drain(process.getInputStream(), stdout);
        Thread errReader = merge ? null : drain(process.getErrorStream(), stderr);
        // Wait through interrupts (the service's executor is shut down with
        // shutdownNow): returning early would let the next command start while
        // this one still runs, and restore depends on stop/setprop/umount/start
        // happening in order. The interrupt is re-asserted afterwards.
        boolean interrupted = false;
        long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(timeoutSeconds);
        try {
            while (true) {
                long remaining = deadline - System.nanoTime();
                try {
                    if (remaining <= 0 || !process.waitFor(remaining, TimeUnit.NANOSECONDS)) {
                        process.destroyForcibly();
                        throw new EyeException("Root command timed out: " + command);
                    }
                    break;
                } catch (InterruptedException interruption) {
                    interrupted = true;
                }
            }
            interrupted |= joinQuietly(outReader);
            if (errReader != null) interrupted |= joinQuietly(errReader);
        } finally {
            if (interrupted) Thread.currentThread().interrupt();
        }
        streams[0] = stdout.toByteArray();
        streams[1] = stderr.toByteArray();
        return process.exitValue();
    }

    /** Joins a drain thread for up to a second; returns true if interrupted. */
    private static boolean joinQuietly(Thread thread) {
        try {
            thread.join(1000);
            return false;
        } catch (InterruptedException interruption) {
            return true;
        }
    }

    private static Thread drain(InputStream input, ByteArrayOutputStream sink) {
        Thread thread = new Thread(() -> {
            try {
                byte[] buffer = new byte[8192];
                int count;
                while ((count = input.read(buffer)) != -1) sink.write(buffer, 0, count);
            } catch (IOException ignored) {
                // stream closed; drain ends
            }
        }, "root-output");
        thread.setDaemon(true);
        thread.start();
        return thread;
    }

    // ---- helpers ---------------------------------------------------------

    private static void writeFile(File file, byte[] data) throws IOException {
        File parent = file.getParentFile();
        if (parent != null && !parent.exists() && !parent.mkdirs()) {
            throw new IOException("Cannot create " + parent);
        }
        try (FileOutputStream output = new FileOutputStream(file)) {
            output.write(data);
        }
    }

    private static String quote(String path) {
        return "'" + path.replace("'", "'\\''") + "'";
    }

    private static String firstToken(String text) {
        String trimmed = text.trim();
        if (trimmed.isEmpty()) return "";
        String[] parts = trimmed.split("\\s+");
        return parts.length > 0 ? parts[0] : "";
    }

    private static String lastToken(String text) {
        String trimmed = text.trim();
        if (trimmed.isEmpty()) return "";
        String[] lines = trimmed.split("\\R");
        String last = lines[lines.length - 1].trim();
        String[] parts = last.split("\\s+");
        return parts.length > 0 ? parts[parts.length - 1] : "";
    }

    /** Sleeps the full duration even if interrupted, then re-asserts the interrupt. */
    private static void sleepMillis(long millis) {
        boolean interrupted = false;
        long deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(millis);
        long remaining;
        while ((remaining = deadline - System.nanoTime()) > 0) {
            try {
                TimeUnit.NANOSECONDS.sleep(remaining);
            } catch (InterruptedException interruption) {
                interrupted = true;
            }
        }
        if (interrupted) Thread.currentThread().interrupt();
    }

    private static final class EyeException extends Exception {
        private static final long serialVersionUID = 1L;

        EyeException(String message) { super(message); }
    }
}
