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
 * {@link TraceParser}, {@link GazePackets} (and the {@link Json} parser they
 * use). Compile with a plain javac against the pure-Java sources only and run
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
                traceLine("1000.100200", "40000000", "3f000000", "00000000", "1"));
        check("in-window pair emitted", pair != null);
        if (pair != null) {
            check("tag0 vector preserved", pair.tag0[0] == 1.0f && pair.tag0[2] == -1.0f);
            check("tag1 vector preserved", pair.tag1[0] == 2.0f && pair.tag1[1] == 0.5f);
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
