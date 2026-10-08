package io.github.matty.vrft.questprocamera;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.Enumeration;
import java.util.List;
import java.util.TreeSet;
import java.util.zip.CRC32;
import java.util.zip.Deflater;
import java.util.zip.ZipEntry;
import java.util.zip.ZipException;
import java.util.zip.ZipFile;

/**
 * Plain-JDK test harness for the Android-free classes: {@link ModelPatcher},
 * {@link TraceParser}, {@link GazePackets}, {@link CameraFrames} (and the
 * {@link Json} parser they use). Compile with a plain javac against the pure-Java sources only and run
 * with java; a nonzero exit code means a failure.
 */
public final class PureTests {
    private static int failures = 0;
    private static int checks = 0;

    public static void main(String[] args) throws Exception {
        testPatchStoredZip();
        testPatchDataDescriptor();
        testRefuseDeflatedMember();
        testRefuseMissingOld();
        testRefuseDuplicateOld();
        testRefuseAlreadyPatched();
        testRefuseContractMismatch();
        testTraceParser();
        testPacketEncoders();
        testCameraFrames();

        System.out.println();
        System.out.println("Checks: " + checks + "  Failures: " + failures);
        if (failures != 0) {
            System.out.println("RESULT: FAIL");
            System.exit(1);
        }
        System.out.println("RESULT: PASS");
    }

    // ---- model patcher --------------------------------------------------

    private static void testPatchStoredZip() throws Exception {
        Zip.Built built = buildArchive(dataPkl(graph(50, "[2, 2]"), "", ""), 0, false, false);
        ModelPatcher.Result result = ModelPatcher.patch(built.bytes);
        section("STORED zip patch");

        // Only the two graph digits and the CRC fields changed.
        TreeSet<Integer> diff = diffOffsets(built.bytes, result.patched);
        TreeSet<Integer> expected = new TreeSet<>();
        int oldAt = indexOf(built.bytes, "[[50, 0]".getBytes(StandardCharsets.US_ASCII),
                built.dataStart);
        expected.add(oldAt + 2); // '5' -> '1'
        expected.add(oldAt + 3); // '0' -> '8'
        for (int i = 0; i < 4; i++) expected.add(built.lfhCrcOffset + i);
        for (int i = 0; i < 4; i++) expected.add(built.cdhCrcOffset + i);
        check("only target bytes + CRCs changed", diff.equals(expected));
        check("length preserved", built.bytes.length == result.patched.length);

        // CRC fields hold the recomputed member CRC.
        check("LFH CRC updated", readU32(result.patched, built.lfhCrcOffset) == result.memberCrc);
        check("CDH CRC updated", readU32(result.patched, built.cdhCrcOffset) == result.memberCrc);

        // java.util.zip reads it back and the member CRC validates.
        byte[] member = readMemberWithZipFile(result.patched, ModelPatcher.GRAPH_MEMBER);
        String text = new String(member, StandardCharsets.UTF_8);
        check("member now redirects to node 18", text.contains("[[18, 0], [51, 0]]"));
        check("member no longer contains node 50 input", !text.contains("[[50, 0], [51, 0]]"));
        CRC32 crc = new CRC32();
        crc.update(member);
        check("read-back CRC matches recomputed", crc.getValue() == result.memberCrc);
    }

    private static void testPatchDataDescriptor() throws Exception {
        Zip.Built built = buildArchive(dataPkl(graph(50, "[2, 2]"), "", ""), 0, true, true);
        section("STORED zip with data descriptor (GP bit 3)");
        check("source has data descriptor CRC field", built.ddCrcOffset >= 0);

        ModelPatcher.Result result = ModelPatcher.patch(built.bytes);
        TreeSet<Integer> diff = diffOffsets(built.bytes, result.patched);
        int oldAt = indexOf(built.bytes, "[[50, 0]".getBytes(StandardCharsets.US_ASCII),
                built.dataStart);
        TreeSet<Integer> expected = new TreeSet<>();
        expected.add(oldAt + 2);
        expected.add(oldAt + 3);
        for (int i = 0; i < 4; i++) expected.add(built.lfhCrcOffset + i);
        for (int i = 0; i < 4; i++) expected.add(built.cdhCrcOffset + i);
        for (int i = 0; i < 4; i++) expected.add(built.ddCrcOffset + i);
        // Some CRC bytes may coincide with their old values; the changed set
        // must be a subset of the target data + the three CRC fields.
        check("changes limited to target data + LFH/CDH/DD CRCs", expected.containsAll(diff));
        check("data digits changed", diff.contains(oldAt + 2) && diff.contains(oldAt + 3));
        check("data descriptor CRC updated",
                readU32(result.patched, built.ddCrcOffset) == result.memberCrc);
        check("zero LFH CRC left zero (descriptor holds the CRC)",
                readU32(result.patched, built.lfhCrcOffset) == 0);
        check("CDH CRC updated", readU32(result.patched, built.cdhCrcOffset) == result.memberCrc);

        byte[] member = readMemberWithZipFile(result.patched, ModelPatcher.GRAPH_MEMBER);
        CRC32 crc = new CRC32();
        crc.update(member);
        check("read-back CRC matches recomputed", crc.getValue() == result.memberCrc);
        check("member redirects to node 18",
                new String(member, StandardCharsets.UTF_8).contains("[[18, 0], [51, 0]]"));
    }

    private static void testRefuseDeflatedMember() throws Exception {
        Zip.Built built = buildArchive(dataPkl(graph(50, "[2, 2]"), "", ""), 8, false, false);
        section("refuse deflated member");
        check("deflated member refused", refuses(built.bytes, "compressed"));
    }

    private static void testRefuseMissingOld() throws Exception {
        // Contract-valid graph but the exact OLD byte string is absent (an
        // extra space after the "input" key), so nothing to replace.
        String node52 = "{\"id\": 52, \"name\": \"211_reshape\", \"op\": \"OP_Reshape\", "
                + "\"padding\": \"NN_PAD_NA\", \"input\":  [[50, 0], [51, 0]], "
                + "\"output\": [{\"shape\": [1, 4]}]}";
        String graph = customGraph("[2, 2]", node52);
        Zip.Built built = buildArchive(dataPkl(graph, "", ""), 0, false, false);
        section("refuse missing OLD string");
        check("missing OLD refused", refuses(built.bytes, "not found"));
    }

    private static void testRefuseDuplicateOld() throws Exception {
        // Same OLD string appears once in the pickle prefix and once in the
        // graph, so the replacement is not unique.
        String duplicate = new String(ModelPatcher.OLD_NODE, StandardCharsets.US_ASCII);
        Zip.Built built = buildArchive(dataPkl(graph(50, "[2, 2]"), duplicate + " ", ""),
                0, false, false);
        section("refuse duplicate OLD string");
        check("duplicate OLD refused", refuses(built.bytes, "not unique"));
    }

    private static void testRefuseAlreadyPatched() throws Exception {
        Zip.Built built = buildArchive(dataPkl(graph(18, "[2, 2]"), "", ""), 0, false, false);
        section("refuse already-patched source");
        check("already-patched refused", refuses(built.bytes, "already patched"));
    }

    private static void testRefuseContractMismatch() throws Exception {
        // Node 18 has the wrong shape, so the contract check fails before patch.
        Zip.Built built = buildArchive(dataPkl(graph(50, "[2, 3]"), "", ""), 0, false, false);
        section("refuse contract mismatch");
        check("contract mismatch refused", refuses(built.bytes, "local gaze node contract"));
    }

    // ---- trace parser ---------------------------------------------------

    private static void testTraceParser() {
        section("trace parser");
        // hex float decode
        check("0x3f800000 decodes to 1.0", TraceParser.floatFromHex("3f800000") == 1.0f);
        check("0xbf800000 decodes to -1.0", TraceParser.floatFromHex("bf800000") == -1.0f);
        check("0x00000000 decodes to 0.0", TraceParser.floatFromHex("00000000") == 0.0f);

        // in-window pair
        TraceParser parser = new TraceParser();
        check("tag0 alone yields no pair",
                parser.parse(traceLine("1000.100000", "3f800000", "00000000", "bf800000", "0")) == null);
        TraceParser.GazePair pair = parser.parse(
                traceLine("1000.100200", "3f800000", "3f000000", "00000000", "1"));
        check("in-window pair emitted", pair != null);
        if (pair != null) {
            check("tag0 vector preserved", pair.tag0[0] == 1.0f && pair.tag0[2] == -1.0f);
            check("tag1 vector preserved", pair.tag1[0] == 1.0f && pair.tag1[1] == 0.5f);
            long expectedNs = Math.round((1000.100000 + 1000.100200) / 2.0 * 1e9);
            check("kernel ns is mean of the pair", pair.kernelTimeNs == expectedNs);
            check("both valid bits set", pair.tag0Valid && pair.tag1Valid);
        }

        // out-of-window: drop the older, keep waiting
        TraceParser drop = new TraceParser();
        drop.parse(traceLine("2000.000000", "3f800000", "3f800000", "3f800000", "0"));
        TraceParser.GazePair none = drop.parse(
                traceLine("2000.010000", "3f800000", "3f800000", "3f800000", "1"));
        check("out-of-window pair dropped", none == null);
        // The dropped older was tag0; a fresh tag0 near the pending tag1 pairs.
        TraceParser.GazePair recovered = drop.parse(
                traceLine("2000.010100", "3f800000", "3f800000", "3f800000", "0"));
        check("older dropped, newer retained then paired", recovered != null);

        // bad tag ignored, does not disturb pending
        TraceParser bad = new TraceParser();
        bad.parse(traceLine("3000.000000", "3f800000", "00000000", "00000000", "0"));
        check("bad tag ignored", bad.parse(
                traceLine("3000.000100", "00000000", "00000000", "00000000", "2")) == null);
        TraceParser.GazePair afterBad = bad.parse(
                traceLine("3000.000200", "00000000", "00000000", "00000000", "1"));
        check("pending survives a bad tag", afterBad != null);

        // non-matching line
        check("garbage line ignored", new TraceParser().parse("not a trace line") == null);

        // a vector far from unit length is not a direction; the other eye stays valid
        TraceParser length = new TraceParser();
        length.parse(traceLine("4000.000000", "00000000", "00000000", "3ecccccd", "0"));
        TraceParser.GazePair shortPair = length.parse(
                traceLine("4000.000100", "00000000", "00000000", "3f800000", "1"));
        check("short vector still pairs", shortPair != null);
        if (shortPair != null) {
            check("short vector invalid, other valid", !shortPair.tag0Valid && shortPair.tag1Valid);
        }
        check("length 1.5 is a direction",
                TraceParser.isDirection(new float[] {0f, 0f, 1.5f}));
        check("length 1.6 is not",
                !TraceParser.isDirection(new float[] {0f, 0f, 1.6f}));
        check("NaN is not",
                !TraceParser.isDirection(new float[] {Float.NaN, 0f, 1f}));

        // both eyes on one line (engine profile 3)
        TraceParser both = new TraceParser();
        TraceParser.GazePair first = both.parse(
                bothLine("5000.250000", "3f800000", "00000000", "00000000",
                        "00000000", "3f000000", "3f000000"));
        check("both-eyes line yields a pair", first != null);
        if (first != null) {
            check("element 0 is tag 0", first.tag0[0] == 1.0f && first.tag0[1] == 0.0f);
            check("element 1 is tag 1", first.tag1[1] == 0.5f && first.tag1[2] == 0.5f);
            check("both-eyes kernel ns", first.kernelTimeNs == Math.round(5000.25 * 1e9));
            check("both-eyes valid bits", first.tag0Valid && first.tag1Valid);
        }
        check("repeated both-eyes line dropped", both.parse(
                bothLine("5000.250100", "3f800000", "00000000", "00000000",
                        "00000000", "3f000000", "3f000000")) == null);
        TraceParser.GazePair next = both.parse(
                bothLine("5000.261000", "3f800000", "00000000", "00000000",
                        "00000000", "00000000", "40000000"));
        check("changed both-eyes line yields a pair", next != null);
        if (next != null) {
            check("long vector invalid", next.tag0Valid && !next.tag1Valid);
        }
        check("a both-eyes line needs no partner", new TraceParser().parse(
                bothLine("5000.300000", "3f800000", "00000000", "00000000",
                        "3f800000", "00000000", "00000000")) != null);

        // engine profile 2 on build 51503870024400340, as traced on a Quest
        // Pro: the tag's upper bytes aren't zero, and the eyes come ~20 µs apart
        TraceParser real = new TraceParser();
        real.parse("         FaceCam-26175 [003] .... 162638.298577: detector_output: "
                + "(0x75195c53e8) x=0x3daaff1c y=0xbd1ec02f z=0x3f7ee9c1 tag=0x61630000");
        TraceParser.GazePair traced = real.parse(
                "         FaceCam-26175 [003] .... 162638.298599: detector_output: "
                + "(0x75195c53e8) x=0xbeac61db y=0xbea9c394 z=0x3f619d73 tag=0x61630001");
        check("profile 2 headset lines pair", traced != null);
        if (traced != null) {
            check("profile 2 headset vectors are directions",
                    traced.tag0Valid && traced.tag1Valid);
        }
    }

    // ---- camera frames --------------------------------------------------

    private static byte[] frameHeader(int mask, long sequence) {
        int width = CameraFrames.views(mask) * CameraFrames.VIEW;
        ByteBuffer header = ByteBuffer.allocate(64).order(ByteOrder.LITTLE_ENDIAN);
        header.put("QPLIVE3".getBytes(StandardCharsets.US_ASCII));
        header.putInt(8, 3).putInt(12, 64).putLong(16, sequence).putLong(24, sequence * 1000)
                .putInt(32, width).putInt(36, 400).putInt(40, width).putInt(44, 1)
                .putInt(48, width * 400).putInt(52, mask).putLong(56, 9);
        return header.array();
    }

    private static void testCameraFrames() {
        section("camera frames");
        check("mouth frame is 800 x 400",
                CameraFrames.payloadBytes(frameHeader(CameraFrames.MASK_MOUTH, 1)) == 320000);
        check("eye frame is 800 x 400",
                CameraFrames.payloadBytes(frameHeader(CameraFrames.MASK_EYES, 1)) == 320000);
        check("five-camera frame is 2000 x 400",
                CameraFrames.payloadBytes(frameHeader(CameraFrames.MASK_ALL, 1)) == 800000);
        byte[] lying = frameHeader(CameraFrames.MASK_ALL, 1);
        ByteBuffer.wrap(lying).order(ByteOrder.LITTLE_ENDIAN).putInt(32, 800);
        check("a width that doesn't match the mask is refused", refusesFrame(lying));
        check("face mode (3 cameras) is refused", refusesFrame(frameHeader(0x1c, 1)));

        // Each pixel holds its camera number, plus its row in the low bits.
        byte[] strip = new byte[CameraFrames.MAX_PIXELS];
        for (int y = 0; y < 400; y++) {
            for (int x = 0; x < 2000; x++) strip[y * 2000 + x] = (byte) ((x / 400) * 40 + (y % 40));
        }
        byte[] header = frameHeader(CameraFrames.MASK_ALL, 77);
        byte[] outHeader = new byte[64];
        byte[] out = new byte[320000];
        CameraFrames.cutPair(header, strip, CameraFrames.MASK_MOUTH, outHeader, out);
        check("mouth cut is a valid mouth frame", CameraFrames.payloadBytes(outHeader) == 320000
                && CameraFrames.mask(outHeader) == CameraFrames.MASK_MOUTH);
        check("mouth cut keeps the sequence", CameraFrames.sequence(outHeader) == 77);
        check("mouth cut keeps the torn count",
                ByteBuffer.wrap(outHeader).order(ByteOrder.LITTLE_ENDIAN).getLong(56) == 9);
        check("mouth cut starts with camera 2", out[0] == 80 && out[399] == 80);
        check("mouth cut ends with camera 3", out[400] == 120 && out[799] == 120);
        check("mouth cut keeps rows", out[800 * 39 + 5] == 80 + 39);
        CameraFrames.cutPair(header, strip, CameraFrames.MASK_EYES, outHeader, out);
        check("eye cut is cameras 0 and 1", out[0] == 0 && out[400] == 40
                && CameraFrames.mask(outHeader) == CameraFrames.MASK_EYES);

        CameraFrames.Hello hello = new CameraFrames.Hello();
        byte[] message = CameraFrames.Hello.encode("{\"camera_masks\":[12,3,31]}");
        check("hello in pieces is incomplete", hello.feed(message, 10) == null);
        byte[] rest = java.util.Arrays.copyOfRange(message, 10, message.length);
        int[] masks = hello.feed(rest, rest.length);
        check("hello lists five-camera frames", CameraFrames.readsAllCameras(masks));
        check("a hello is read once", hello.feed(message, message.length) == null);
        CameraFrames.Hello mouthOnly = new CameraFrames.Hello();
        byte[] mouthHello = CameraFrames.Hello.encode("{\"camera_masks\":[12]}");
        check("a mouth-only hello doesn't ask for five",
                !CameraFrames.readsAllCameras(mouthOnly.feed(mouthHello, mouthHello.length)));
        CameraFrames.Hello junk = new CameraFrames.Hello();
        byte[] text = "GET / HTTP/1.1\r\n\r\n".getBytes(StandardCharsets.US_ASCII);
        check("anything else is ignored", junk.feed(text, text.length) == null
                && junk.feed(message, message.length) == null);

        CameraFrames.EyeSchedule schedule = new CameraFrames.EyeSchedule(5);
        int sent = 0;
        for (long at = 1; at <= 1_000_000_000L; at += 1_000_000_000L / 24) {
            if (schedule.due(at)) sent++;
        }
        check("eye pairs at 5 fps from 24 fps frames", sent == 5);
        check("no eye pairs at 0 fps", !new CameraFrames.EyeSchedule(0).due(1));
    }

    private static boolean refusesFrame(byte[] header) {
        try {
            CameraFrames.payloadBytes(header);
            return false;
        } catch (IllegalArgumentException expected) {
            return true;
        }
    }

    // ---- packet encoders ------------------------------------------------

    private static void testPacketEncoders() {
        section("packet encoders");
        float[] tag0 = {1.0f, -2.0f, 3.5f};
        float[] tag1 = {-0.25f, 4.0f, 0.0f};
        byte[] gaze = GazePackets.encodeGaze(7L, 123456789L,
                GazePackets.FLAG_TAG0_VALID | GazePackets.FLAG_TAG1_VALID
                        | GazePackets.FLAG_MODEL_ACTIVE, 2, tag0, tag1);
        check("QPGAZE1 is 64 bytes", gaze.length == 64);
        ByteBuffer g = ByteBuffer.wrap(gaze).order(ByteOrder.LITTLE_ENDIAN);
        check("QPGAZE1 magic", "QPGAZE1".equals(new String(gaze, 0, 7, StandardCharsets.US_ASCII)));
        check("QPGAZE1 magic null-padded", gaze[7] == 0);
        check("QPGAZE1 version", g.getInt(8) == 1);
        check("QPGAZE1 message bytes", g.getInt(12) == 64);
        check("QPGAZE1 sequence", g.getLong(16) == 7L);
        check("QPGAZE1 timestamp", g.getLong(24) == 123456789L);
        check("QPGAZE1 flags", g.getInt(32) == 0x7);
        check("QPGAZE1 engine profile", g.getInt(36) == 2);
        check("QPGAZE1 tag0 x", g.getFloat(40) == 1.0f);
        check("QPGAZE1 tag0 y", g.getFloat(44) == -2.0f);
        check("QPGAZE1 tag0 z", g.getFloat(48) == 3.5f);
        check("QPGAZE1 tag1 x", g.getFloat(52) == -0.25f);
        check("QPGAZE1 tag1 y", g.getFloat(56) == 4.0f);
        check("QPGAZE1 tag1 z", g.getFloat(60) == 0.0f);

        String json = "{\"apk_version\":\"0.2\"}";
        byte[] stat = GazePackets.encodeStatus(json);
        ByteBuffer s = ByteBuffer.wrap(stat).order(ByteOrder.LITTLE_ENDIAN);
        check("QPSTAT1 magic", "QPSTAT1".equals(new String(stat, 0, 7, StandardCharsets.US_ASCII)));
        check("QPSTAT1 magic null-padded", stat[7] == 0);
        check("QPSTAT1 version", s.getInt(8) == 1);
        byte[] payload = json.getBytes(StandardCharsets.UTF_8);
        check("QPSTAT1 length", s.getInt(12) == payload.length);
        check("QPSTAT1 total size", stat.length == 16 + payload.length);
        check("QPSTAT1 payload", json.equals(new String(stat, 16, payload.length, StandardCharsets.UTF_8)));

        // Json round-trip used to build the status object.
        Object parsed = Json.parse(json);
        check("Json parses status object", parsed instanceof java.util.Map);
    }

    // ---- graph / archive builders ---------------------------------------

    private static String graph(int reshapeInputNode, String node18Shape) {
        String node52 = "{\"id\": 52, \"name\": \"211_reshape\", \"op\": \"OP_Reshape\", "
                + "\"padding\": \"NN_PAD_NA\", \"input\": [[" + reshapeInputNode
                + ", 0], [51, 0]], \"output\": [{\"shape\": [1, 4]}]}";
        return customGraph(node18Shape, node52);
    }

    private static String customGraph(String node18Shape, String node52) {
        return "{\"version\": \"HEXAGON-1.0\", \"node\": ["
                + "{\"id\": 18, \"name\": \"local\", \"op\": \"OP_L\", \"output\": [{\"shape\": "
                + node18Shape + "}]}, "
                + "{\"id\": 50, \"name\": \"blend\", \"op\": \"OP_B\", \"output\": [{\"shape\": [2, 2]}]}, "
                + "{\"id\": 51, \"name\": \"n51\", \"op\": \"OP_N\", \"output\": [{\"shape\": [1, 1]}]}, "
                + node52
                + "], \"output\": [{\"shape\": [1, 4]}, {\"shape\": [1, 6]}, {\"shape\": [1, 6]}, "
                + "{\"shape\": [1, 1]}, {\"shape\": [1, 1]}]}";
    }

    private static byte[] dataPkl(String graph, String prefix, String suffix) {
        String body = "PICKLE_PREFIX_" + prefix + graph + suffix + "_END";
        return body.getBytes(StandardCharsets.UTF_8);
    }

    private static Zip.Built buildArchive(byte[] graphMember, int method,
                                          boolean useDataDescriptor, boolean withSignature)
            throws IOException {
        Zip zip = new Zip();
        zip.addStored("version", "3\n".getBytes(StandardCharsets.US_ASCII), 0, false, false);
        zip.addTarget(ModelPatcher.GRAPH_MEMBER, graphMember, method,
                useDataDescriptor, withSignature);
        return zip.build();
    }

    // ---- helpers --------------------------------------------------------

    private static String traceLine(String time, String x, String y, String z, String tag) {
        return "          probe-1234  [000] d..1  " + time
                + ": detector_output: (arg) x=0x" + x + " y=0x" + y
                + " z=0x" + z + " tag=0x" + tag;
    }

    private static String bothLine(String time, String lx, String ly, String lz,
                                   String rx, String ry, String rz) {
        return "          probe-1234  [000] d..1  " + time
                + ": detector_output: (arg) lx=0x" + lx + " ly=0x" + ly + " lz=0x" + lz
                + " rx=0x" + rx + " ry=0x" + ry + " rz=0x" + rz;
    }

    private static boolean refuses(byte[] archive, String expectedFragment) {
        try {
            ModelPatcher.patch(archive);
            System.out.println("    expected refusal containing: " + expectedFragment);
            return false;
        } catch (ModelPatcher.ModelPatchException expected) {
            boolean ok = expected.getMessage().toLowerCase().contains(expectedFragment.toLowerCase());
            if (!ok) System.out.println("    wrong message: " + expected.getMessage());
            return ok;
        }
    }

    private static byte[] readMemberWithZipFile(byte[] archive, String name) throws IOException {
        File temp = File.createTempFile("vrft-model", ".ptl");
        temp.deleteOnExit();
        Files.write(temp.toPath(), archive);
        try (ZipFile zip = new ZipFile(temp)) {
            Enumeration<? extends ZipEntry> entries = zip.entries();
            while (entries.hasMoreElements()) {
                ZipEntry entry = entries.nextElement();
                if (!entry.getName().equals(name)) continue;
                try (InputStream input = zip.getInputStream(entry)) {
                    ByteArrayOutputStream out = new ByteArrayOutputStream();
                    byte[] buffer = new byte[4096];
                    int read;
                    while ((read = input.read(buffer)) != -1) out.write(buffer, 0, read);
                    return out.toByteArray(); // throws ZipException on CRC mismatch
                }
            }
        } catch (ZipException badCrc) {
            throw new IOException("java.util.zip rejected the archive: " + badCrc.getMessage());
        }
        throw new IOException("member not found: " + name);
    }

    private static TreeSet<Integer> diffOffsets(byte[] a, byte[] b) {
        TreeSet<Integer> diff = new TreeSet<>();
        int length = Math.min(a.length, b.length);
        for (int i = 0; i < length; i++) if (a[i] != b[i]) diff.add(i);
        for (int i = length; i < Math.max(a.length, b.length); i++) diff.add(i);
        return diff;
    }

    private static int indexOf(byte[] haystack, byte[] needle, int from) {
        outer:
        for (int i = from; i + needle.length <= haystack.length; i++) {
            for (int j = 0; j < needle.length; j++) {
                if (haystack[i + j] != needle[j]) continue outer;
            }
            return i;
        }
        return -1;
    }

    private static long readU32(byte[] data, int offset) {
        return (data[offset] & 0xffL)
                | ((data[offset + 1] & 0xffL) << 8)
                | ((data[offset + 2] & 0xffL) << 16)
                | ((data[offset + 3] & 0xffL) << 24);
    }

    // ---- reporting ------------------------------------------------------

    private static void section(String name) {
        System.out.println("[" + name + "]");
    }

    private static void check(String label, boolean ok) {
        checks++;
        if (!ok) failures++;
        System.out.println("  " + (ok ? "PASS" : "FAIL") + "  " + label);
    }

    /** Minimal STORED-zip builder that tracks the target member's CRC offsets. */
    private static final class Zip {
        private final ByteArrayOutputStream body = new ByteArrayOutputStream();
        private final List<Entry> entries = new ArrayList<>();

        private static final class Entry {
            String name;
            int method;
            long crc;
            int compSize;
            int uncompSize;
            int flags;
            int localOffset;
            int cdhCrcOffset;   // filled during build()
        }

        private static final class TargetInfo {
            String name;
            int lfhCrcOffset;
            int dataStart;
            int dataLen;
            int ddCrcOffset = -1;
        }

        static final class Built {
            byte[] bytes;
            int lfhCrcOffset;
            int cdhCrcOffset;
            int ddCrcOffset;
            int dataStart;
            int dataLen;
        }

        private TargetInfo target;

        void addStored(String name, byte[] data, int method, boolean dd, boolean sig)
                throws IOException {
            addTarget(name, data, method, dd, sig);
            // A non-target member: forget its target bookkeeping.
            if (name.equals("version")) target = null;
        }

        void addTarget(String name, byte[] data, int method, boolean useDd, boolean sig)
                throws IOException {
            Entry entry = new Entry();
            entry.name = name;
            entry.method = method;
            entry.flags = useDd ? 0x08 : 0x00;
            entry.uncompSize = data.length;
            CRC32 crc = new CRC32();
            crc.update(data);
            entry.crc = crc.getValue();

            byte[] stored;
            if (method == 8) {
                Deflater deflater = new Deflater(Deflater.DEFAULT_COMPRESSION, true);
                deflater.setInput(data);
                deflater.finish();
                ByteArrayOutputStream compressed = new ByteArrayOutputStream();
                byte[] buffer = new byte[4096];
                while (!deflater.finished()) {
                    int n = deflater.deflate(buffer);
                    compressed.write(buffer, 0, n);
                }
                deflater.end();
                stored = compressed.toByteArray();
            } else {
                stored = data;
            }
            entry.compSize = stored.length;
            entry.localOffset = body.size();

            byte[] name8 = name.getBytes(StandardCharsets.UTF_8);
            ByteArrayOutputStream lfh = new ByteArrayOutputStream();
            writeU32(lfh, 0x04034b50L);
            writeU16(lfh, 20);
            writeU16(lfh, entry.flags);
            writeU16(lfh, method);
            writeU16(lfh, 0); // time
            writeU16(lfh, 0); // date
            int lfhCrcOffset = entry.localOffset + 14;
            writeU32(lfh, useDd ? 0 : entry.crc);
            writeU32(lfh, useDd ? 0 : entry.compSize);
            writeU32(lfh, useDd ? 0 : entry.uncompSize);
            writeU16(lfh, name8.length);
            writeU16(lfh, 0); // extra length
            lfh.write(name8);
            body.write(lfh.toByteArray());
            int dataStart = body.size();
            body.write(stored);

            int ddCrcOffset = -1;
            if (useDd) {
                ByteArrayOutputStream dd = new ByteArrayOutputStream();
                if (sig) writeU32(dd, 0x08074b50L);
                int ddStart = body.size();
                ddCrcOffset = sig ? ddStart + 4 : ddStart;
                writeU32(dd, entry.crc);
                writeU32(dd, entry.compSize);
                writeU32(dd, entry.uncompSize);
                body.write(dd.toByteArray());
            }

            entries.add(entry);
            if (!name.equals("version")) {
                target = new TargetInfo();
                target.name = name;
                target.lfhCrcOffset = lfhCrcOffset;
                target.dataStart = dataStart;
                target.dataLen = stored.length;
                target.ddCrcOffset = ddCrcOffset;
            }
        }

        Built build() throws IOException {
            byte[] localSection = body.toByteArray();
            ByteArrayOutputStream out = new ByteArrayOutputStream();
            out.write(localSection);
            int cdOffset = out.size();
            for (Entry entry : entries) {
                byte[] name8 = entry.name.getBytes(StandardCharsets.UTF_8);
                ByteArrayOutputStream cdh = new ByteArrayOutputStream();
                writeU32(cdh, 0x02014b50L);
                writeU16(cdh, 20); // version made by
                writeU16(cdh, 20); // version needed
                writeU16(cdh, entry.flags);
                writeU16(cdh, entry.method);
                writeU16(cdh, 0); // time
                writeU16(cdh, 0); // date
                entry.cdhCrcOffset = cdOffset + cdh.size();
                writeU32(cdh, entry.crc);
                writeU32(cdh, entry.compSize);
                writeU32(cdh, entry.uncompSize);
                writeU16(cdh, name8.length);
                writeU16(cdh, 0); // extra
                writeU16(cdh, 0); // comment
                writeU16(cdh, 0); // disk start
                writeU16(cdh, 0); // internal attrs
                writeU32(cdh, 0); // external attrs
                writeU32(cdh, entry.localOffset);
                cdh.write(name8);
                out.write(cdh.toByteArray());
                cdOffset += cdh.size();
            }
            int cdEnd = out.size();
            int cdStart = localSection.length;
            ByteArrayOutputStream eocd = new ByteArrayOutputStream();
            writeU32(eocd, 0x06054b50L);
            writeU16(eocd, 0);
            writeU16(eocd, 0);
            writeU16(eocd, entries.size());
            writeU16(eocd, entries.size());
            writeU32(eocd, cdEnd - cdStart);
            writeU32(eocd, cdStart);
            writeU16(eocd, 0);
            out.write(eocd.toByteArray());

            Built built = new Built();
            built.bytes = out.toByteArray();
            built.lfhCrcOffset = target.lfhCrcOffset;
            built.dataStart = target.dataStart;
            built.dataLen = target.dataLen;
            built.ddCrcOffset = target.ddCrcOffset;
            for (Entry entry : entries) {
                if (entry.name.equals(target.name)) built.cdhCrcOffset = entry.cdhCrcOffset;
            }
            return built;
        }

        private static void writeU16(ByteArrayOutputStream out, int value) {
            out.write(value & 0xff);
            out.write((value >>> 8) & 0xff);
        }

        private static void writeU32(ByteArrayOutputStream out, long value) {
            out.write((int) (value & 0xff));
            out.write((int) ((value >>> 8) & 0xff));
            out.write((int) ((value >>> 16) & 0xff));
            out.write((int) ((value >>> 24) & 0xff));
        }
    }
}
