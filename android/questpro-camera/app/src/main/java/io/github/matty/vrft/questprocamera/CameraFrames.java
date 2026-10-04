package io.github.matty.vrft.questprocamera;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.Map;

/**
 * The relay's {@code QPLIVE3} camera frames, and what each PC is sent of them.
 *
 * <p>The headset's sensor strip is five 400 x 400 views side by side: the eyes
 * (cameras 0 and 1), the mouth (2 and 3) and the brow (4). A frame carries a
 * run of them, named by a camera mask: {@code 0x0c} for the mouth pair,
 * {@code 0x03} for the eye pair and {@code 0x1f} for all five.
 *
 * <p>With the five-camera stream on, the relay sends whole strips, and only a
 * PC that says it reads them (see {@link Hello}) gets them as they are. Every
 * other PC gets the mouth pair cut from each strip and, at the eye snapshot
 * rate, the eye pair, exactly as the relay sends them without the five-camera
 * stream. So a VRFT from before the five-camera stream keeps working.
 *
 * <p>Pure Java (no Android) so the byte layout can be unit-tested on a plain
 * JDK.
 */
public final class CameraFrames {
    public static final int HEADER_BYTES = 64;
    public static final int VIEW = 400;
    public static final int CAMERAS = 5;
    public static final int MASK_EYES = 0x03;
    public static final int MASK_MOUTH = 0x0c;
    public static final int MASK_ALL = 0x1f;
    /** The largest frame: every camera. */
    public static final int MAX_PIXELS = CAMERAS * VIEW * VIEW;

    private CameraFrames() { }

    /** How many views a frame with this mask carries, or 0 for a mask the stream doesn't use. */
    public static int views(int mask) {
        switch (mask) {
            case MASK_EYES:
            case MASK_MOUTH: return 2;
            case MASK_ALL: return CAMERAS;
            default: return 0;
        }
    }

    /** The first camera of a mask's run. */
    public static int firstCamera(int mask) {
        return Integer.numberOfTrailingZeros(mask);
    }

    /**
     * Checks a relay frame header and returns the pixel bytes that follow it.
     *
     * @throws IllegalArgumentException for anything but a mouth, eye or
     *         five-camera {@code QPLIVE3} frame
     */
    public static int payloadBytes(byte[] header) {
        ByteBuffer fields = ByteBuffer.wrap(header).order(ByteOrder.LITTLE_ENDIAN);
        int mask = fields.getInt(52);
        int width = views(mask) * VIEW;
        if (header.length != HEADER_BYTES
                || !"QPLIVE3".equals(new String(header, 0, 7, StandardCharsets.US_ASCII))
                || fields.getInt(8) != 3 || fields.getInt(12) != HEADER_BYTES
                || width == 0
                || fields.getInt(32) != width || fields.getInt(36) != VIEW
                || fields.getInt(40) != width || fields.getInt(44) != 1
                || fields.getInt(48) != width * VIEW) {
            throw new IllegalArgumentException("Unexpected relay frame format");
        }
        return width * VIEW;
    }

    public static int mask(byte[] header) {
        return ByteBuffer.wrap(header).order(ByteOrder.LITTLE_ENDIAN).getInt(52);
    }

    public static long sequence(byte[] header) {
        return ByteBuffer.wrap(header).order(ByteOrder.LITTLE_ENDIAN).getLong(16);
    }

    /**
     * Cuts the pair of cameras {@code mask} names out of a five-camera frame,
     * as the relay would have sent it: the same sequence, timestamp and torn
     * count, with the size fields and mask of the pair.
     *
     * @param header a five-camera frame's header
     * @param pixels its pixels
     * @param mask {@link #MASK_MOUTH} or {@link #MASK_EYES}
     * @param outHeader 64 bytes for the pair's header
     * @param outPixels at least 800 x 400 bytes for the pair's pixels
     */
    public static void cutPair(byte[] header, byte[] pixels, int mask,
                               byte[] outHeader, byte[] outPixels) {
        if (mask(header) != MASK_ALL || views(mask) != 2) {
            throw new IllegalArgumentException("Only a pair is cut from a five-camera frame");
        }
        int width = 2 * VIEW;
        int stride = CAMERAS * VIEW;
        int left = firstCamera(mask) * VIEW;
        for (int y = 0; y < VIEW; y++) {
            System.arraycopy(pixels, y * stride + left, outPixels, y * width, width);
        }
        System.arraycopy(header, 0, outHeader, 0, HEADER_BYTES);
        ByteBuffer fields = ByteBuffer.wrap(outHeader).order(ByteOrder.LITTLE_ENDIAN);
        fields.putInt(32, width);
        fields.putInt(40, width);
        fields.putInt(48, width * VIEW);
        fields.putInt(52, mask);
    }

    /**
     * Reads the {@code QPHELO1} a PC sends when it connects, saying which
     * camera frames it reads. Bytes arrive in pieces, as the socket gives
     * them; a PC from before the hello sends nothing, and anything that isn't
     * a hello is ignored.
     *
     * <p>Layout, little-endian: the magic {@code QPHELO1\0}, a {@code u32}
     * version (1) at byte 8, the JSON payload's length as a {@code u32} at
     * byte 12, then the payload, such as {@code {"camera_masks":[12,3,31]}}.
     */
    public static final class Hello {
        public static final int MAX_PAYLOAD = 4096;
        private static final byte[] MAGIC = "QPHELO1\0".getBytes(StandardCharsets.US_ASCII);
        private final byte[] buffer = new byte[16 + MAX_PAYLOAD];
        private int length;
        private boolean done;

        /**
         * Adds bytes from the PC. Returns the camera masks the PC reads once
         * the whole hello has arrived, else null; after that, or after
         * anything that isn't a hello, it returns null for good.
         */
        public int[] feed(byte[] data, int count) {
            if (done) return null;
            int take = Math.min(count, buffer.length - length);
            System.arraycopy(data, 0, buffer, length, take);
            length += take;
            for (int i = 0; i < Math.min(length, MAGIC.length); i++) {
                if (buffer[i] != MAGIC[i]) {
                    done = true;
                    return null;
                }
            }
            if (length < 16) return null;
            ByteBuffer fields = ByteBuffer.wrap(buffer).order(ByteOrder.LITTLE_ENDIAN);
            int payload = fields.getInt(12);
            if (fields.getInt(8) != 1 || payload < 2 || payload > MAX_PAYLOAD) {
                done = true;
                return null;
            }
            if (length < 16 + payload) return null;
            done = true;
            try {
                Object parsed = Json.parse(new String(buffer, 16, payload, StandardCharsets.UTF_8));
                Object masks = parsed instanceof Map ? ((Map<?, ?>) parsed).get("camera_masks") : null;
                if (!(masks instanceof List)) return new int[0];
                List<?> list = (List<?>) masks;
                int[] result = new int[list.size()];
                for (int i = 0; i < result.length; i++) {
                    Object value = list.get(i);
                    result[i] = value instanceof Long ? (int) (long) (Long) value : -1;
                }
                return result;
            } catch (IllegalArgumentException error) {
                return new int[0];
            }
        }

        /** Encodes a hello, as VRFT sends it; for tests. */
        public static byte[] encode(String json) {
            byte[] payload = json.getBytes(StandardCharsets.UTF_8);
            ByteBuffer out = ByteBuffer.allocate(16 + payload.length).order(ByteOrder.LITTLE_ENDIAN);
            out.put(MAGIC);
            out.putInt(1);
            out.putInt(payload.length);
            out.put(payload);
            return out.array();
        }
    }

    /** Whether {@code masks}, from a hello, include whole five-camera frames. */
    public static boolean readsAllCameras(int[] masks) {
        if (masks == null) return false;
        for (int mask : masks) if (mask == MASK_ALL) return true;
        return false;
    }

    /**
     * When to cut an eye pair from the five-camera frames for a PC that
     * doesn't read them, at the eye snapshot rate, as the relay's
     * {@code --eye-fps} does.
     */
    public static final class EyeSchedule {
        private final long intervalNs;
        private long nextAt;

        /** @param fps snapshots a second; 0 sends none */
        public EyeSchedule(int fps) {
            intervalNs = fps > 0 ? 1_000_000_000L / fps : 0;
        }

        /** Whether the frame arriving at {@code nowNs} gets an eye pair too. */
        public boolean due(long nowNs) {
            if (intervalNs == 0) return false;
            if (nextAt != 0 && nowNs - nextAt < 0) return false;
            if (nextAt == 0) nextAt = nowNs;
            do {
                nextAt += intervalNs;
            } while (nextAt - nowNs <= 0);
            return true;
        }
    }
}
