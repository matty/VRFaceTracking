package io.github.matty.vrft.questprocamera;

import java.util.HashMap;
import java.util.Map;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * Parses {@code detector_output} uprobe trace lines and pairs the per-eye
 * (tag 0 / tag 1) vectors, a port of the fork's {@code DetectorOutputParser}.
 *
 * <p>Pure Java (no Android) so it is unit-testable on a plain JDK. The APK does
 * not convert to angles, calibrate, filter, or swap eyes: it only pairs tag 0
 * and tag 1 events and forwards the raw traced vectors.
 */
public final class TraceParser {
    /** A window wider than this (in seconds) between the two eyes is rejected. */
    private static final double PAIR_WINDOW_SECONDS = 0.004;

    private static final Pattern LINE = Pattern.compile(
            "(\\d+\\.\\d+): detector_output: .*?"
            + "x=0x([0-9a-fA-F]+) y=0x([0-9a-fA-F]+) "
            + "z=0x([0-9a-fA-F]+) tag=0x([0-9a-fA-F]+)");

    /** One paired left/right (tag 0 / tag 1) gaze sample. */
    public static final class GazePair {
        /** Mean of the two trace timestamps, in nanoseconds. */
        public final long kernelTimeNs;
        public final float[] tag0;
        public final float[] tag1;
        public final boolean tag0Valid;
        public final boolean tag1Valid;

        GazePair(long kernelTimeNs, float[] tag0, float[] tag1,
                 boolean tag0Valid, boolean tag1Valid) {
            this.kernelTimeNs = kernelTimeNs;
            this.tag0 = tag0;
            this.tag1 = tag1;
            this.tag0Valid = tag0Valid;
            this.tag1Valid = tag1Valid;
        }
    }

    private static final class Pending {
        final double timeSeconds;
        final float[] vector;

        Pending(double timeSeconds, float[] vector) {
            this.timeSeconds = timeSeconds;
            this.vector = vector;
        }
    }

    private final Map<Integer, Pending> pending = new HashMap<>();

    /**
     * Feed one trace line. Returns a completed {@link GazePair} when both eyes
     * are present within the pairing window, otherwise {@code null}.
     */
    public GazePair parse(String line) {
        Matcher matcher = LINE.matcher(line);
        if (!matcher.find()) return null;
        int tag = (int) (Long.parseLong(matcher.group(5), 16) & 0xFF);
        if (tag != 0 && tag != 1) return null;
        double time = Double.parseDouble(matcher.group(1));
        float[] vector = {
                floatFromHex(matcher.group(2)),
                floatFromHex(matcher.group(3)),
                floatFromHex(matcher.group(4)),
        };
        pending.put(tag, new Pending(time, vector));
        if (!pending.containsKey(0) || !pending.containsKey(1)) return null;

        Pending zero = pending.get(0);
        Pending one = pending.get(1);
        if (Math.abs(one.timeSeconds - zero.timeSeconds) > PAIR_WINDOW_SECONDS) {
            int older = zero.timeSeconds < one.timeSeconds ? 0 : 1;
            pending.remove(older);
            return null;
        }
        pending.clear();
        long kernelTimeNs = Math.round((zero.timeSeconds + one.timeSeconds) / 2.0 * 1e9);
        return new GazePair(kernelTimeNs, zero.vector, one.vector,
                allFinite(zero.vector), allFinite(one.vector));
    }

    /** Decode a little-endian IEEE-754 float from a trace hex token. */
    public static float floatFromHex(String hex) {
        return Float.intBitsToFloat((int) Long.parseLong(hex, 16));
    }

    private static boolean allFinite(float[] vector) {
        for (float value : vector) {
            if (Float.isNaN(value) || Float.isInfinite(value)) return false;
        }
        return true;
    }
}
