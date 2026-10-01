package io.github.matty.vrft.questprocamera;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;

/**
 * Encoders for the wire messages produced by the APK service: the per-eye gaze
 * sample {@code QPGAZE1} and the status object {@code QPSTAT1}. All integers are
 * little-endian, matching the camera {@code QPLIVE3} frames on the same stream.
 *
 * <p>Pure Java (no Android) so the byte layout can be unit-tested on a plain
 * JDK.
 */
public final class GazePackets {
    /**
     * Stream protocol this app speaks, advertised over mDNS and in every
     * {@code QPSTAT1}. VRFT refuses a protocol it can't read, so bump this only
     * when an existing message changes. A new message type keeps it if it puts
     * its payload length at byte 12, as {@code QPSTAT1} does: VRFT skips
     * messages it doesn't know by that length.
     */
    public static final int PROTOCOL = 3;
    public static final int GAZE_BYTES = 64;
    public static final int GAZE_VERSION = 1;
    public static final int STAT_VERSION = 1;

    /** flag bit 0: tag 0 vector is valid (finite, with a squared length within [0.25, 2.25]). */
    public static final int FLAG_TAG0_VALID = 0x1;
    /** flag bit 1: tag 1 vector is valid (finite, with a squared length within [0.25, 2.25]). */
    public static final int FLAG_TAG1_VALID = 0x2;
    /** flag bit 2: the patched independent-axes model is active. */
    public static final int FLAG_MODEL_ACTIVE = 0x4;

    private static final byte[] GAZE_MAGIC = magic("QPGAZE1");
    private static final byte[] STAT_MAGIC = magic("QPSTAT1");

    private GazePackets() { }

    private static byte[] magic(String tag) {
        byte[] magic = new byte[8];
        byte[] ascii = tag.getBytes(StandardCharsets.US_ASCII);
        System.arraycopy(ascii, 0, magic, 0, ascii.length);
        return magic; // trailing bytes remain 0
    }

    /** Encode a 64-byte {@code QPGAZE1} sample. */
    public static byte[] encodeGaze(long sequence, long kernelTimeNs, int flags,
                                    int engineProfileId, float[] tag0, float[] tag1) {
        ByteBuffer buffer = ByteBuffer.allocate(GAZE_BYTES).order(ByteOrder.LITTLE_ENDIAN);
        buffer.put(GAZE_MAGIC);
        buffer.putInt(GAZE_VERSION);
        buffer.putInt(GAZE_BYTES);
        buffer.putLong(sequence);
        buffer.putLong(kernelTimeNs);
        buffer.putInt(flags);
        buffer.putInt(engineProfileId);
        buffer.putFloat(tag0[0]);
        buffer.putFloat(tag0[1]);
        buffer.putFloat(tag0[2]);
        buffer.putFloat(tag1[0]);
        buffer.putFloat(tag1[1]);
        buffer.putFloat(tag1[2]);
        return buffer.array();
    }

    /** Encode a {@code QPSTAT1} status message from a UTF-8 JSON string. */
    public static byte[] encodeStatus(String json) {
        byte[] payload = json.getBytes(StandardCharsets.UTF_8);
        if (payload.length < 1 || payload.length > 16384) {
            throw new IllegalArgumentException(
                    "Status JSON length out of range: " + payload.length);
        }
        ByteBuffer buffer = ByteBuffer.allocate(16 + payload.length)
                .order(ByteOrder.LITTLE_ENDIAN);
        buffer.put(STAT_MAGIC);
        buffer.putInt(STAT_VERSION);
        buffer.putInt(payload.length);
        buffer.put(payload);
        return buffer.array();
    }
}
