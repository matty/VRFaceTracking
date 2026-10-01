package io.github.matty.vrft.questprocamera;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.net.nsd.NsdManager;
import android.net.nsd.NsdServiceInfo;
import android.net.wifi.WifiManager;
import android.os.IBinder;
import android.os.ParcelFileDescriptor;
import android.system.ErrnoException;
import android.system.Os;
import android.system.OsConstants;
import android.util.Log;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.net.SocketTimeoutException;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

/** Owns the root relay and advertises a LAN stream while any app is foreground. */
public final class CameraStreamService extends Service {
    public static final String ACTION_START = "io.github.matty.vrft.questprocamera.START";
    public static final String ACTION_STOP = "io.github.matty.vrft.questprocamera.STOP";
    public static final int LAN_PORT = 27274;
    private static final String TAG = "VRFTCamera";
    private static final int RELAY_PORT = 27273;
    private static final String ROOT_DIR = "/data/local/tmp/vrft-camera";
    private static final String STREAMER_NAME = "libquestpro-camera-streamer-v9.so";
    private static final String INJECTOR_NAME = "questpro-camera-injector";
    private static final String RELAY_NAME = "questpro-camera-relay-v9";
    private static final String STREAMER = ROOT_DIR + "/" + STREAMER_NAME;
    private static final String INJECTOR = ROOT_DIR + "/" + INJECTOR_NAME;
    private static final String RELAY = ROOT_DIR + "/" + RELAY_NAME;
    /** The injected streamer's log; it runs as the provider's user and only appends. */
    private static final String STREAMER_LOG = "/data/local/tmp/questpro-live-v9.log";
    private static final String CHANNEL = "camera_stream";
    private static final int MOUTH_MASK = 0x0c;
    private static final int EYE_MASK = 0x03;
    private static final String[] HELPERS = { STREAMER_NAME, INJECTOR_NAME, RELAY_NAME };
    /** A write to the PC that takes longer ends that connection; the relay allows 2 s. */
    private static final long WRITE_TIMEOUT_NS = TimeUnit.SECONDS.toNanos(3);
    private static final long RELAY_CHECK_NS = TimeUnit.SECONDS.toNanos(5);
    /** Relay restarts allowed per stream before it counts as failed. */
    private static final int MAX_RELAY_RESTARTS = 5;
    private static final long REGISTRATION_RETRY_NS = TimeUnit.SECONDS.toNanos(30);
    // Linux <netinet/tcp.h> values, for the keepalive timers below.
    private static final int TCP_KEEPIDLE = 4;
    private static final int TCP_KEEPINTVL = 5;
    private static final int TCP_KEEPCNT = 6;
    /** Where a stream is, for the panel. */
    public enum Phase { STOPPED, STARTING, WAITING, CONNECTED, STREAMING, FAILED }

    private static volatile Phase phase = Phase.STOPPED;
    private static volatile String status = "Stopped";
    private static volatile EyePipeline.EyeStatus eyeStatus;
    /** Whether a stream runs, so the panel can restart it to apply eye gaze. */
    private static volatile boolean active;
    /** The threads finishing a stop: the relay stop and the eye model restore. */
    private static final Set<Thread> finishing = ConcurrentHashMap.newKeySet();
    private final ExecutorService worker = Executors.newSingleThreadExecutor();
    /** The newest PC connection. A new one replaces it, so a PC that vanished never blocks the next. */
    private final AtomicReference<Socket> currentClient = new AtomicReference<>();
    /** Guards every whole message written to the current client (frames, gaze, status). */
    private final Object outputLock = new Object();
    /** Serializes starting the relay with taking it in {@link #onDestroy}. */
    private final Object relayLock = new Object();
    private volatile boolean running;
    private volatile ServerSocket server;
    /** The client {@link #clientOut} belongs to; guarded by {@link #outputLock}. */
    private Socket outputOwner;
    private OutputStream clientOut;
    /** The write in progress, watched from the accept loop. */
    private volatile PendingWrite pendingWrite;
    private volatile Process relayProcess;
    private volatile EyePipeline eyePipeline;
    private int cameraFps;
    private int eyePreviewFps;
    private boolean eyeEnabled;
    // Relay supervision, only touched by the worker thread.
    private long nextRelayCheckAt;
    private int relayMisses;
    private int relayRestarts;
    private NsdManager nsd;
    private volatile NsdManager.RegistrationListener registration;
    /** When to register with mDNS again after a failure; 0 when none is due. */
    private volatile long registrationRetryAt;
    private WifiManager.MulticastLock multicastLock;

    private static final class PendingWrite {
        final Socket client;
        final long startedAt;

        PendingWrite(Socket client, long startedAt) {
            this.client = client;
            this.startedAt = startedAt;
        }
    }

    public static Phase getPhase() { return phase; }

    /** The latest step, or why the stream failed, in a line. */
    public static String getStatus() { return status; }

    /** The stream's eye gaze, or null before one has started. */
    public static EyePipeline.EyeStatus getEyeStatus() { return eyeStatus; }

    public static boolean isActive() { return active; }

    /** Whether a stopped stream is still putting things back. */
    public static boolean isFinishing() {
        finishing.removeIf(thread -> !thread.isAlive());
        return !finishing.isEmpty();
    }

    @Override public IBinder onBind(Intent intent) { return null; }

    @Override public int onStartCommand(Intent intent, int flags, int startId) {
        deleteSharedPreferences("pairing");
        if (intent != null && ACTION_STOP.equals(intent.getAction())) {
            stopSelf();
            return START_NOT_STICKY;
        }
        if (running) return START_NOT_STICKY;
        running = true;
        active = true;
        eyeStatus = null;
        setStatus(Phase.STARTING, "Starting");
        cameraFps = Settings.getCameraFps(this);
        eyePreviewFps = Settings.getEyePreviewFps(this);
        eyeEnabled = Settings.isEyeEnabled(this);
        eyePipeline = new EyePipeline(this, this::onEyeStatusChanged, this::sendGazePacket);
        NotificationManager notifications = (NotificationManager) getSystemService(NOTIFICATION_SERVICE);
        notifications.createNotificationChannel(new NotificationChannel(
                CHANNEL, "Quest Pro camera stream", NotificationManager.IMPORTANCE_LOW));
        Intent open = new Intent(this, MainActivity.class);
        PendingIntent pending = PendingIntent.getActivity(this, 0, open,
                PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
        Notification notification = new Notification.Builder(this, CHANNEL)
                .setSmallIcon(R.drawable.ic_mark)
                .setContentTitle("Streaming Quest Pro cameras")
                .setContentText("Your PC can connect over Wi-Fi.")
                .setContentIntent(pending)
                .setOngoing(true)
                .build();
        startForeground(1, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE);
        WifiManager wifi = getApplicationContext().getSystemService(WifiManager.class);
        if (wifi != null) {
            // Wi-Fi can drop multicast while idle; the PC's mDNS queries must get through.
            multicastLock = wifi.createMulticastLock("vrft-mdns");
            multicastLock.setReferenceCounted(false);
            multicastLock.acquire();
        }
        worker.execute(this::runServer);
        return START_NOT_STICKY;
    }

    private void runServer() {
        try {
            setStatus(Phase.STARTING, "Getting root access. Allow it in Magisk if asked");
            copyHelpers();
            String root = runRoot("id", 60);
            if (!root.contains("uid=0")) throw new IOException("Magisk did not grant root");

            // Crash recovery: undo a stale eye-model mount left by a previous run.
            eyePipeline.recoverIfNeeded();
            // Eye pipeline runs before camera injection so the patched model is
            // active before the tracking service is otherwise disturbed.
            if (eyeEnabled) {
                setStatus(Phase.STARTING, "Preparing eye gaze");
                eyePipeline.start();
            } else {
                onEyeStatusChanged();
            }

            String source = quote(new File(getFilesDir(), "native").getAbsolutePath());
            StringBuilder install = new StringBuilder("mkdir -p " + ROOT_DIR);
            for (String name : HELPERS) {
                String destination = ROOT_DIR + "/" + name;
                install.append(" && cp ").append(source).append('/').append(name).append(' ')
                        .append(destination).append(".new && mv -f ")
                        .append(destination).append(".new ").append(destination);
            }
            install.append(" && chmod 755 ").append(INJECTOR).append(' ').append(RELAY)
                    .append(" && chmod 644 ").append(STREAMER)
                    .append(" && touch ").append(STREAMER_LOG)
                    .append(" && chmod 666 ").append(STREAMER_LOG);
            runRoot(install.toString(), 20);
            if (!running) return;
            setStatus(Phase.STARTING, "Starting the cameras");
            runRoot(INJECTOR + " " + STREAMER, 30);
            startRelay();
            if (!running) return;

            ServerSocket listener = new ServerSocket();
            listener.setReuseAddress(true);
            listener.bind(new InetSocketAddress(LAN_PORT));
            listener.setSoTimeout(1000);
            server = listener;
            advertise();
            setStatus(Phase.WAITING, "Listening on port " + LAN_PORT);
            nextRelayCheckAt = System.nanoTime() + RELAY_CHECK_NS;
            // accept() wakes at least once a second for these checks.
            while (running) {
                superviseRelay();
                closeStuckWrite();
                retryRegistrationIfDue();
                try {
                    replaceClient(listener.accept());
                } catch (SocketTimeoutException ignored) { }
            }
        } catch (Exception error) {
            if (running) {
                setStatus(Phase.FAILED, String.valueOf(error.getMessage()));
                stopSelf();
            }
        }
    }

    /** Stops any earlier relay and starts ours; it re-injects the streamer if the provider loses it. */
    private void startRelay() throws Exception {
        runRoot(RELAY + " --stop", 10);
        synchronized (relayLock) {
            if (!running) return;
            Process relay = new ProcessBuilder("su", "-c", RELAY
                    + " --mode mouth --max-fps " + cameraFps
                    + " --eye-fps " + eyePreviewFps
                    + " --injector " + INJECTOR + " --streamer " + STREAMER)
                    .redirectErrorStream(true).start();
            relayProcess = relay;
            Thread logs = new Thread(() -> readRelayLog(relay), "relay-log");
            logs.setDaemon(true);
            logs.start();
        }
    }

    /** Restarts the relay after it has been found stopped twice in a row. */
    private void superviseRelay() throws Exception {
        long now = System.nanoTime();
        if (now - nextRelayCheckAt < 0) return;
        nextRelayCheckAt = now + RELAY_CHECK_NS;
        Process relay = relayProcess;
        if (!running || (relay != null && relay.isAlive())) {
            relayMisses = 0;
            return;
        }
        if (++relayMisses < 2) return;
        relayMisses = 0;
        if (++relayRestarts > MAX_RELAY_RESTARTS) {
            throw new IOException("The camera relay keeps stopping");
        }
        Log.i(TAG, "Relay stopped; restarting it (" + relayRestarts + ")");
        startRelay();
    }

    /** Ends a connection whose write has not finished in time, so a dead PC can't hold {@link #outputLock}. */
    private void closeStuckWrite() {
        PendingWrite write = pendingWrite;
        if (write != null && System.nanoTime() - write.startedAt > WRITE_TIMEOUT_NS) {
            Log.i(TAG, "A write to the PC stalled; closing that connection");
            closeQuietly(write.client);
        }
    }

    private void replaceClient(Socket client) {
        Socket previous = currentClient.getAndSet(client);
        if (previous != null) {
            Log.i(TAG, "A new PC connection replaces the previous one");
            closeQuietly(previous);
        }
        enableKeepAlive(client);
        new Thread(() -> {
            try { serveClient(client); }
            catch (IOException error) { if (running) Log.i(TAG, "Client left: " + error); }
            finally {
                closeQuietly(client);
                releaseOutput(client);
                if (currentClient.compareAndSet(client, null) && running) {
                    setStatus(Phase.WAITING, "Listening on port " + LAN_PORT);
                }
            }
        }, "camera-client").start();
    }

    private void serveClient(Socket client) throws IOException {
        Socket relay = new Socket();
        try {
            relay.connect(new InetSocketAddress("127.0.0.1", RELAY_PORT), 3000);
            relay.setSoTimeout(3000);
            client.setSoTimeout(0);
            client.setTcpNoDelay(true);
            OutputStream toPc = client.getOutputStream();
            InputStream fromRelay = relay.getInputStream();
            synchronized (outputLock) {
                // A newer connection may have replaced this one meanwhile.
                if (currentClient.get() != client) return;
                outputOwner = client;
                clientOut = toPc;
            }
            watchForClose(client);
            sendStatus(); // QPSTAT1 immediately on connect
            byte[] header = new byte[64];
            byte[] pixels = new byte[800 * 400];
            long lastSequence = 0;
            setStatus(Phase.CONNECTED, "PC connected");
            while (running && !client.isClosed()) {
                if (!readFully(fromRelay, header, true)) continue;
                ByteBuffer fields = ByteBuffer.wrap(header).order(ByteOrder.LITTLE_ENDIAN);
                int mask = fields.getInt(52);
                if (!"QPLIVE3".equals(new String(header, 0, 7, StandardCharsets.US_ASCII))
                        || fields.getInt(8) != 3 || fields.getInt(12) != 64
                        || fields.getInt(32) != 800 || fields.getInt(36) != 400
                        || fields.getInt(40) != 800 || fields.getInt(44) != 1
                        || fields.getInt(48) != pixels.length
                        || (mask != MOUTH_MASK && mask != EYE_MASK))
                    throw new IOException("Unexpected relay frame format");
                readFully(fromRelay, pixels, false);
                if (!writeMessage(client, header, pixels)) break;
                if (mask == MOUTH_MASK) {
                    long sequence = fields.getLong(16);
                    if (lastSequence == 0 || sequence - lastSequence >= 30) {
                        setStatus(Phase.STREAMING, "Streaming, frame " + sequence);
                        lastSequence = sequence;
                    }
                }
            }
        } finally {
            relay.close();
        }
    }

    /**
     * Writes one whole message to {@code owner} if it still owns the output.
     * A failed write ends that connection. Returns false once it has gone.
     */
    private boolean writeMessage(Socket owner, byte[]... parts) {
        synchronized (outputLock) {
            if (owner == null || owner != outputOwner || clientOut == null) return false;
            pendingWrite = new PendingWrite(owner, System.nanoTime());
            try {
                for (byte[] part : parts) clientOut.write(part);
                clientOut.flush();
                return true;
            } catch (IOException error) {
                Log.i(TAG, "Write to the PC failed: " + error);
                outputOwner = null;
                clientOut = null;
                closeQuietly(owner);
                return false;
            } finally {
                pendingWrite = null;
            }
        }
    }

    private void releaseOutput(Socket owner) {
        synchronized (outputLock) {
            if (outputOwner != owner) return;
            outputOwner = null;
            clientOut = null;
        }
    }

    /** Write an encoded QPGAZE1 packet to the current client, or drop it. */
    private void sendGazePacket(byte[] packet) {
        synchronized (outputLock) {
            writeMessage(outputOwner, packet);
        }
    }

    /** Encode and send QPSTAT1 to the current client, if any. */
    private void sendStatus() {
        byte[] packet = GazePackets.encodeStatus(buildStatusJson());
        synchronized (outputLock) {
            writeMessage(outputOwner, packet);
        }
    }

    private String buildStatusJson() {
        Map<String, Object> root = new LinkedHashMap<>();
        root.put("apk_version", BuildConfig.VERSION_NAME);
        root.put("protocol", (long) GazePackets.PROTOCOL);
        root.put("camera_fps", (long) cameraFps);
        root.put("eye_preview_fps", (long) eyePreviewFps);
        EyePipeline pipeline = eyePipeline;
        if (pipeline != null) root.put("eye", pipeline.getStatus().toMap());
        return Json.write(root);
    }

    /** Eye status changed: refresh the on-screen line and resend QPSTAT1. */
    private void onEyeStatusChanged() {
        EyePipeline pipeline = eyePipeline;
        if (pipeline != null) {
            eyeStatus = pipeline.getStatus();
        }
        sendStatus();
    }

    /**
     * The PC never sends anything, so end-of-stream or an error on its socket
     * means it has gone; closing the socket then ends {@link #serveClient}.
     */
    private static void watchForClose(Socket client) {
        Thread watcher = new Thread(() -> {
            try {
                InputStream input = client.getInputStream();
                byte[] discard = new byte[256];
                while (input.read(discard) >= 0) { }
            } catch (IOException ignored) {
                // closed here or by the stream; either way it has ended
            } finally {
                closeQuietly(client);
            }
        }, "camera-client-watch");
        watcher.setDaemon(true);
        watcher.start();
    }

    /**
     * Keepalive probes after 10 s idle, every 3 s, 3 tries, instead of Linux's
     * two-hour default, so a PC that vanished is noticed while no frames flow.
     */
    private static void enableKeepAlive(Socket client) {
        try {
            client.setKeepAlive(true);
        } catch (IOException error) {
            Log.i(TAG, "Keepalive not enabled: " + error);
            return;
        }
        try (ParcelFileDescriptor descriptor = ParcelFileDescriptor.fromSocket(client)) {
            if (descriptor == null) return;
            Os.setsockoptInt(descriptor.getFileDescriptor(), OsConstants.IPPROTO_TCP, TCP_KEEPIDLE, 10);
            Os.setsockoptInt(descriptor.getFileDescriptor(), OsConstants.IPPROTO_TCP, TCP_KEEPINTVL, 3);
            Os.setsockoptInt(descriptor.getFileDescriptor(), OsConstants.IPPROTO_TCP, TCP_KEEPCNT, 3);
        } catch (IOException | ErrnoException error) {
            Log.i(TAG, "Keepalive timers not set: " + error);
        }
    }

    private static void closeQuietly(Socket socket) {
        if (socket == null) return;
        try { socket.close(); } catch (IOException ignored) { }
    }

    private static boolean readFully(InputStream stream, byte[] data, boolean allowIdle) throws IOException {
        int offset = 0;
        while (offset < data.length) {
            try {
                int read = stream.read(data, offset, data.length - offset);
                if (read < 0) throw new IOException("Relay disconnected");
                offset += read;
            } catch (SocketTimeoutException timeout) {
                if (allowIdle && offset == 0) return false;
            }
        }
        return true;
    }

    private void advertise() {
        nsd = (NsdManager) getSystemService(Context.NSD_SERVICE);
        NsdServiceInfo info = new NsdServiceInfo();
        info.setServiceName("VRFT Quest Pro Camera");
        info.setServiceType("_vrftcam._tcp.");
        info.setPort(LAN_PORT);
        info.setAttribute("protocol", String.valueOf(GazePackets.PROTOCOL));
        info.setAttribute("apk_version", BuildConfig.VERSION_NAME);
        info.setAttribute("format", "gray8");
        // A listener can only be registered once, so every attempt gets its own.
        NsdManager.RegistrationListener listener = new NsdManager.RegistrationListener() {
            @Override public void onServiceRegistered(NsdServiceInfo service) {
                Log.i(TAG, "mDNS registered: " + service.getServiceName());
            }
            @Override public void onRegistrationFailed(NsdServiceInfo service, int error) {
                Log.e(TAG, "mDNS registration failed: " + error + "; retrying in 30 s");
                if (registration == this) registration = null;
                registrationRetryAt = System.nanoTime() + REGISTRATION_RETRY_NS;
            }
            @Override public void onServiceUnregistered(NsdServiceInfo service) { }
            @Override public void onUnregistrationFailed(NsdServiceInfo service, int error) { }
        };
        registration = listener;
        try {
            nsd.registerService(info, NsdManager.PROTOCOL_DNS_SD, listener);
        } catch (RuntimeException error) {
            Log.e(TAG, "mDNS registration error: " + error + "; retrying in 30 s");
            registration = null;
            registrationRetryAt = System.nanoTime() + REGISTRATION_RETRY_NS;
        }
    }

    private void retryRegistrationIfDue() {
        long retryAt = registrationRetryAt;
        if (retryAt == 0 || System.nanoTime() - retryAt < 0) return;
        registrationRetryAt = 0;
        advertise();
    }

    private void copyHelpers() throws IOException {
        File folder = new File(getFilesDir(), "native");
        if (!folder.exists() && !folder.mkdirs()) throw new IOException("Cannot create helper directory");
        for (String name : HELPERS) {
            try (InputStream input = getAssets().open("native/" + name);
                 FileOutputStream output = new FileOutputStream(new File(folder, name))) {
                byte[] buffer = new byte[16384];
                int count;
                while ((count = input.read(buffer)) != -1) output.write(buffer, 0, count);
            }
        }
    }

    private static void readRelayLog(Process relay) {
        try (InputStream input = relay.getInputStream()) {
            byte[] buffer = new byte[512];
            int count;
            while ((count = input.read(buffer)) != -1)
                Log.i(TAG, "Relay: " + new String(buffer, 0, count, StandardCharsets.UTF_8).trim());
        } catch (IOException ignored) { }
    }

    private static String runRoot(String command, int timeoutSeconds) throws Exception {
        Process process = new ProcessBuilder("su", "-c", command).redirectErrorStream(true).start();
        ByteArrayOutputStream output = new ByteArrayOutputStream();
        Thread reader = new Thread(() -> {
            try (InputStream input = process.getInputStream()) {
                byte[] buffer = new byte[1024];
                int count;
                while ((count = input.read(buffer)) != -1) output.write(buffer, 0, count);
            } catch (IOException ignored) { }
        }, "root-output");
        reader.start();
        if (!process.waitFor(timeoutSeconds, TimeUnit.SECONDS)) {
            process.destroyForcibly();
            throw new IOException("Root command timed out");
        }
        reader.join(1000);
        String result = output.toString(StandardCharsets.UTF_8);
        if (process.exitValue() != 0) throw new IOException("Root command failed: " + result.trim());
        return result;
    }

    private static String quote(String path) { return "'" + path.replace("'", "'\\''") + "'"; }

    private static void setStatus(Phase next, String message) {
        phase = next;
        status = message;
        Log.i(TAG, next + ": " + message);
    }

    private static void finish(Thread thread) {
        finishing.add(thread);
        thread.start();
    }

    @Override public void onDestroy() {
        running = false;
        if (nsd != null && registration != null) {
            try { nsd.unregisterService(registration); } catch (Exception ignored) { }
        }
        if (multicastLock != null && multicastLock.isHeld()) multicastLock.release();
        try { if (server != null) server.close(); } catch (IOException ignored) { }
        closeQuietly(currentClient.getAndSet(null));
        Process relay;
        synchronized (relayLock) {
            relay = relayProcess;
            relayProcess = null;
        }
        final EyePipeline pipeline = eyePipeline;
        // The desktop app waits for the "stop-relay" and "stop-eye" threads to
        // finish before it replaces this app, so keep those names.
        if (relay != null) {
            finish(new Thread(() -> {
                try { runRoot(RELAY + " --stop", 5); }
                catch (Exception ignored) { }
                relay.destroy();
            }, "stop-relay"));
        }
        // Restore the stock eye model on a background thread, like the relay stop.
        if (pipeline != null) {
            finish(new Thread(pipeline::stop, "stop-eye"));
        }
        active = false;
        worker.shutdownNow();
        if (phase != Phase.FAILED) setStatus(Phase.STOPPED, "Stopped");
        super.onDestroy();
    }
}
