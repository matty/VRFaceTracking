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
import android.os.IBinder;
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
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;

/** Owns the root relay and advertises a LAN stream while VD is foreground. */
public final class CameraStreamService extends Service {
    public static final String ACTION_START = "io.github.matty.vrft.questprocamera.START";
    public static final String ACTION_STOP = "io.github.matty.vrft.questprocamera.STOP";
    public static final int LAN_PORT = 27274;
    private static final int RELAY_PORT = 27273;
    private static final String ROOT_DIR = "/data/local/tmp/vrft-camera";
    private static final String CHANNEL = "camera_stream";
    private static final int MOUTH_MASK = 0x0c;
    private static final int EYE_MASK = 0x03;
    private static final String[] HELPERS = {
            "libquestpro-camera-streamer-v8.so", "questpro-camera-injector", "questpro-camera-relay-v8"
    };
    private static volatile String status = "Stopped";
    private static volatile String eyeStatusLine = "";
    private final ExecutorService worker = Executors.newSingleThreadExecutor();
    private final AtomicBoolean clientBusy = new AtomicBoolean(false);
    /** Guards every whole message written to the current client (frames, gaze, status). */
    private final Object outputLock = new Object();
    private volatile boolean running;
    private volatile ServerSocket server;
    private volatile Socket currentClient;
    private volatile OutputStream clientOut;
    private volatile Process relayProcess;
    private volatile EyePipeline eyePipeline;
    private int cameraFps;
    private int eyePreviewFps;
    private boolean eyeEnabled;
    private NsdManager nsd;
    private NsdManager.RegistrationListener registration;

    public static String getStatus() {
        String eye = eyeStatusLine;
        return eye.isEmpty() ? status : status + "\nEye: " + eye;
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
                .setSmallIcon(android.R.drawable.presence_video_online)
                .setContentTitle("VRFT camera stream")
                .setContentText("Camera stream available on local network")
                .setContentIntent(pending)
                .setOngoing(true)
                .build();
        startForeground(1, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE);
        worker.execute(this::runServer);
        return START_NOT_STICKY;
    }

    private void runServer() {
        try {
            setStatus("Requesting root and preparing camera relay...");
            copyHelpers();
            String root = runRoot("id", 60);
            if (!root.contains("uid=0")) throw new IOException("Magisk did not grant root");

            // Crash recovery: undo a stale eye-model mount left by a previous run.
            eyePipeline.recoverIfNeeded();
            // Eye pipeline runs before camera injection so the patched model is
            // active before the tracking service is otherwise disturbed.
            if (eyeEnabled) {
                setStatus("Preparing independent eye gaze...");
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
            install.append(" && chmod 755 ").append(ROOT_DIR).append("/questpro-camera-injector ")
                    .append(ROOT_DIR).append("/questpro-camera-relay-v8 && chmod 644 ")
                    .append(ROOT_DIR).append("/libquestpro-camera-streamer-v8.so")
                    .append(" && touch /data/local/tmp/questpro-live-v8.log")
                    .append(" && chmod 666 /data/local/tmp/questpro-live-v8.log");
            runRoot(install.toString(), 20);
            if (!running) return;
            setStatus("Injecting camera helper...");
            runRoot(ROOT_DIR + "/questpro-camera-injector "
                    + ROOT_DIR + "/libquestpro-camera-streamer-v8.so", 30);
            runRoot(ROOT_DIR + "/questpro-camera-relay-v8 --stop", 10);
            if (!running) return;
            Process relay = new ProcessBuilder("su", "-c", ROOT_DIR
                    + "/questpro-camera-relay-v8 --mode mouth --max-fps " + cameraFps
                    + " --eye-fps " + eyePreviewFps)
                    .redirectErrorStream(true).start();
            relayProcess = relay;
            Thread logs = new Thread(() -> readRelayLog(relay), "relay-log");
            logs.setDaemon(true);
            logs.start();

            ServerSocket listener = new ServerSocket();
            listener.setReuseAddress(true);
            listener.bind(new InetSocketAddress(LAN_PORT));
            listener.setSoTimeout(1000);
            server = listener;
            advertise();
            setStatus("Listening on port " + LAN_PORT + ". Open Virtual Desktop, then connect vrft_d.");
            while (running) {
                try {
                    Socket client = listener.accept();
                    if (!clientBusy.compareAndSet(false, true)) {
                        client.close();
                        continue;
                    }
                    currentClient = client;
                    new Thread(() -> {
                        try { serveClient(client); }
                        catch (IOException error) { if (running) Log.i("VRFTCamera", "Client left: " + error); }
                        finally {
                            synchronized (outputLock) { clientOut = null; }
                            try { client.close(); } catch (IOException ignored) { }
                            currentClient = null;
                            clientBusy.set(false);
                            if (running) setStatus("Waiting for PC on port " + LAN_PORT);
                        }
                    }, "camera-client").start();
                } catch (SocketTimeoutException ignored) { }
            }
        } catch (Exception error) {
            if (running) {
                setStatus("Camera stream failed: " + error.getMessage());
                stopSelf();
            }
        }
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
            synchronized (outputLock) { clientOut = toPc; }
            sendStatus(); // QPSTAT1 immediately on connect
            byte[] header = new byte[64];
            byte[] pixels = new byte[800 * 400];
            long lastSequence = 0;
            setStatus("PC connected; waiting for Virtual Desktop cameras...");
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
                synchronized (outputLock) {
                    if (clientOut == null) break;
                    clientOut.write(header);
                    clientOut.write(pixels);
                    clientOut.flush();
                }
                if (mask == MOUTH_MASK) {
                    long sequence = fields.getLong(16);
                    if (lastSequence == 0 || sequence - lastSequence >= 30) {
                        setStatus("Streaming cameras 2 + 3 to PC | frame " + sequence);
                        lastSequence = sequence;
                    }
                }
            }
        } finally {
            relay.close();
        }
    }

    /** Write an encoded QPGAZE1 packet to the current client, or drop it. */
    private void sendGazePacket(byte[] packet) {
        synchronized (outputLock) {
            OutputStream out = clientOut;
            if (out == null) return;
            try {
                out.write(packet);
                out.flush();
            } catch (IOException error) {
                Log.i("VRFTCamera", "Gaze send failed: " + error);
            }
        }
    }

    /** Encode and send QPSTAT1 to the current client, if any. */
    private void sendStatus() {
        byte[] packet = GazePackets.encodeStatus(buildStatusJson());
        synchronized (outputLock) {
            OutputStream out = clientOut;
            if (out == null) return;
            try {
                out.write(packet);
                out.flush();
            } catch (IOException error) {
                Log.i("VRFTCamera", "Status send failed: " + error);
            }
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
            EyePipeline.EyeStatus eye = pipeline.getStatus();
            eyeStatusLine = eye.state + " - " + eye.message;
        }
        sendStatus();
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
        registration = new NsdManager.RegistrationListener() {
            @Override public void onServiceRegistered(NsdServiceInfo service) {
                Log.i("VRFTCamera", "mDNS registered: " + service.getServiceName());
            }
            @Override public void onRegistrationFailed(NsdServiceInfo service, int error) {
                Log.e("VRFTCamera", "mDNS registration failed: " + error);
            }
            @Override public void onServiceUnregistered(NsdServiceInfo service) { }
            @Override public void onUnregistrationFailed(NsdServiceInfo service, int error) { }
        };
        nsd.registerService(info, NsdManager.PROTOCOL_DNS_SD, registration);
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
                Log.i("VRFTCamera", "Relay: " + new String(buffer, 0, count, StandardCharsets.UTF_8).trim());
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

    private static void setStatus(String message) {
        status = message;
        Log.i("VRFTCamera", message);
    }

    @Override public void onDestroy() {
        running = false;
        if (nsd != null && registration != null) {
            try { nsd.unregisterService(registration); } catch (Exception ignored) { }
        }
        try { if (server != null) server.close(); } catch (IOException ignored) { }
        try { if (currentClient != null) currentClient.close(); } catch (IOException ignored) { }
        Process relay = relayProcess;
        relayProcess = null;
        final EyePipeline pipeline = eyePipeline;
        // The desktop app waits for the "stop-relay" and "stop-eye" threads to
        // finish before it replaces this app, so keep those names.
        if (relay != null) {
            new Thread(() -> {
                try { runRoot(ROOT_DIR + "/questpro-camera-relay-v8 --stop", 5); }
                catch (Exception ignored) { }
                relay.destroy();
            }, "stop-relay").start();
        }
        // Restore the stock eye model on a background thread, like the relay stop.
        if (pipeline != null) {
            new Thread(pipeline::stop, "stop-eye").start();
        }
        worker.shutdownNow();
        if (!status.startsWith("Camera stream failed:")) setStatus("Stopped");
        super.onDestroy();
    }
}
