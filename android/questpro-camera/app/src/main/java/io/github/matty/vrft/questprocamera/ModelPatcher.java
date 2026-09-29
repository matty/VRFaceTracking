package io.github.matty.vrft.questprocamera;

import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.Map;
import java.util.zip.CRC32;

/**
 * In-place, byte-exact patcher for the Seacliff eye-tracking model archive.
 *
 * <p>The {@code .ptl} file is a ZIP written by PyTorch whose members are stored
 * (compression method 0). This class redirects the public gaze reshape
 * (node 52) from the binocular blend node 50 to the local per-eye node 18, so
 * the two eyes keep independent visual axes. Only the graph member
 * {@code model/data.pkl} and the CRC-32 fields that cover it change; every
 * other byte stays identical.
 *
 * <p>Pure Java (no Android, no org.json) so it is unit-testable on a plain JDK.
 * A port of the fork's {@code patch_seacliff_independent_axes.py}.
 */
public final class ModelPatcher {
    public static final String GRAPH_MEMBER = "model/data.pkl";
    private static final byte[] GRAPH_MARKER =
            "{\"version\": \"HEXAGON".getBytes(StandardCharsets.US_ASCII);
    static final byte[] OLD_NODE = (
            "\"id\": 52, \"name\": \"211_reshape\", \"op\": \"OP_Reshape\", "
            + "\"padding\": \"NN_PAD_NA\", \"input\": [[50, 0], [51, 0]]"
    ).getBytes(StandardCharsets.US_ASCII);
    static final byte[] NEW_NODE = newNode();

    private static final long SIG_LOCAL = 0x04034b50L;
    private static final long SIG_CENTRAL = 0x02014b50L;
    private static final long SIG_EOCD = 0x06054b50L;
    private static final long SIG_DATA_DESCRIPTOR = 0x08074b50L;

    private ModelPatcher() { }

    private static byte[] newNode() {
        String old = new String(OLD_NODE, StandardCharsets.US_ASCII);
        String replaced = old.replaceFirst("\\[\\[50, 0\\]", "[[18, 0]");
        return replaced.getBytes(StandardCharsets.US_ASCII);
    }

    /** Thrown for any archive that cannot be safely patched. */
    public static final class ModelPatchException extends Exception {
        private static final long serialVersionUID = 1L;

        public ModelPatchException(String message) { super(message); }
    }

    /** Result of a successful patch. */
    public static final class Result {
        public final byte[] patched;
        public final long memberCrc;
        public final String sha256;

        Result(byte[] patched, long memberCrc, String sha256) {
            this.patched = patched;
            this.memberCrc = memberCrc;
            this.sha256 = sha256;
        }
    }

    /**
     * Patch a copy of {@code source} in place and return the new bytes with the
     * recomputed CRCs. The input array is not modified.
     */
    public static Result patch(byte[] source) throws ModelPatchException {
        byte[] archive = source.clone();
        MemberLocation member = locate(archive);
        if (member.method != 0) {
            throw new ModelPatchException(
                    "Refusing to patch: " + GRAPH_MEMBER
                    + " is compressed (method " + member.method
                    + "); only stored members are supported");
        }

        byte[] before = new byte[member.dataLength];
        System.arraycopy(archive, member.dataStart, before, 0, member.dataLength);

        // Contract check on the stock (unpatched) graph.
        Map<String, Object> graph = graphFromMember(before);
        List<List<Long>> reshapeInput = reshapeInput(graph);
        if (reshapeInput.equals(patchedInput())) {
            // Already redirected to node 18: never mount someone else's patch.
            throw new ModelPatchException(
                    "Refusing to patch: source is already patched "
                    + "(gaze reshape already reads local node 18)");
        }
        validateContract(graph, false);

        int occurrences = countOccurrences(before, OLD_NODE);
        if (occurrences == 0) {
            throw new ModelPatchException(
                    "Refusing to patch: expected gaze reshape bytes not found");
        }
        if (occurrences > 1) {
            throw new ModelPatchException(
                    "Refusing to patch: gaze reshape bytes are not unique ("
                    + occurrences + " occurrences)");
        }

        byte[] after = replaceFirst(before, OLD_NODE, NEW_NODE);
        if (after.length != before.length) {
            throw new ModelPatchException("Patch is not byte-length preserving");
        }

        // Contract check on the patched graph before touching the archive.
        validateContract(graphFromMember(after), true);

        System.arraycopy(after, 0, archive, member.dataStart, after.length);

        CRC32 crc = new CRC32();
        crc.update(after);
        long value = crc.getValue();
        // With a data descriptor (bit 3) the local header's CRC is normally
        // zero and the descriptor holds the real one; keep a zero as zero so
        // the patch stays minimal. The stock Quest Pro model is written this way.
        if (member.dataDescriptorCrcOffset < 0 || readU32(archive, member.localCrcOffset) != 0) {
            putU32(archive, member.localCrcOffset, value);
        }
        putU32(archive, member.centralCrcOffset, value);
        if (member.dataDescriptorCrcOffset >= 0) {
            putU32(archive, member.dataDescriptorCrcOffset, value);
        }

        return new Result(archive, value, sha256Hex(archive));
    }

    private static final class MemberLocation {
        int method;
        int dataStart;
        int dataLength;
        int localCrcOffset;
        int centralCrcOffset;
        int dataDescriptorCrcOffset; // -1 when general-purpose bit 3 is clear
    }

    private static MemberLocation locate(byte[] archive) throws ModelPatchException {
        int eocd = findEocd(archive);
        long total = readU16(archive, eocd + 10);
        long cdOffset = readU32(archive, eocd + 16);
        int pointer = (int) cdOffset;
        for (long i = 0; i < total; i++) {
            if (pointer + 46 > archive.length || readU32(archive, pointer) != SIG_CENTRAL) {
                throw new ModelPatchException("Corrupt central directory entry");
            }
            int flags = (int) readU16(archive, pointer + 8);
            int method = (int) readU16(archive, pointer + 10);
            long compressedSize = readU32(archive, pointer + 20);
            int nameLength = (int) readU16(archive, pointer + 28);
            int extraLength = (int) readU16(archive, pointer + 30);
            int commentLength = (int) readU16(archive, pointer + 32);
            long localOffset = readU32(archive, pointer + 42);
            String name = new String(archive, pointer + 46, nameLength,
                    StandardCharsets.UTF_8);
            if (name.equals(GRAPH_MEMBER)) {
                MemberLocation location = new MemberLocation();
                location.method = method;
                location.centralCrcOffset = pointer + 16;
                fillLocal(archive, (int) localOffset, (int) compressedSize,
                        flags, location);
                return location;
            }
            pointer += 46 + nameLength + extraLength + commentLength;
        }
        throw new ModelPatchException("Archive does not contain " + GRAPH_MEMBER);
    }

    private static void fillLocal(byte[] archive, int localOffset,
                                  int compressedSize, int flags,
                                  MemberLocation location)
            throws ModelPatchException {
        if (localOffset + 30 > archive.length
                || readU32(archive, localOffset) != SIG_LOCAL) {
            throw new ModelPatchException("Corrupt local header for " + GRAPH_MEMBER);
        }
        int nameLength = (int) readU16(archive, localOffset + 26);
        int extraLength = (int) readU16(archive, localOffset + 28);
        location.dataStart = localOffset + 30 + nameLength + extraLength;
        location.dataLength = compressedSize;
        location.localCrcOffset = localOffset + 14;
        location.dataDescriptorCrcOffset = -1;
        if (location.dataStart + compressedSize > archive.length) {
            throw new ModelPatchException("Member data runs past end of archive");
        }
        if ((flags & 0x08) != 0) {
            int descriptor = location.dataStart + compressedSize;
            if (descriptor + 12 > archive.length) {
                throw new ModelPatchException("Truncated data descriptor");
            }
            if (readU32(archive, descriptor) == SIG_DATA_DESCRIPTOR) {
                location.dataDescriptorCrcOffset = descriptor + 4;
            } else {
                location.dataDescriptorCrcOffset = descriptor;
            }
        }
    }

    private static int findEocd(byte[] archive) throws ModelPatchException {
        int minimum = Math.max(0, archive.length - (22 + 0xffff));
        for (int i = archive.length - 22; i >= minimum; i--) {
            if (readU32(archive, i) == SIG_EOCD) return i;
        }
        throw new ModelPatchException("End of central directory not found");
    }

    private static Map<String, Object> graphFromMember(byte[] member)
            throws ModelPatchException {
        int start = indexOf(member, GRAPH_MARKER, 0);
        if (start < 0) {
            throw new ModelPatchException("Lowered HEXAGON graph was not found");
        }
        String text = new String(member, start, member.length - start,
                StandardCharsets.UTF_8);
        try {
            Object value = Json.parseFirst(text);
            if (!(value instanceof Map)) {
                throw new ModelPatchException("Graph is not a JSON object");
            }
            @SuppressWarnings("unchecked")
            Map<String, Object> graph = (Map<String, Object>) value;
            return graph;
        } catch (RuntimeException error) {
            throw new ModelPatchException("Could not parse graph JSON: "
                    + error.getMessage());
        }
    }

    @SuppressWarnings("unchecked")
    private static Map<Long, Map<String, Object>> nodesById(Map<String, Object> graph)
            throws ModelPatchException {
        Object nodes = graph.get("node");
        if (!(nodes instanceof List)) {
            throw new ModelPatchException("Graph has no node list");
        }
        java.util.Map<Long, Map<String, Object>> byId = new java.util.HashMap<>();
        for (Object item : (List<Object>) nodes) {
            if (!(item instanceof Map)) continue;
            Map<String, Object> node = (Map<String, Object>) item;
            Object id = node.get("id");
            if (id instanceof Number) byId.put(((Number) id).longValue(), node);
        }
        return byId;
    }

    @SuppressWarnings("unchecked")
    private static List<List<Long>> reshapeInput(Map<String, Object> graph)
            throws ModelPatchException {
        Map<String, Object> node = nodesById(graph).get(52L);
        if (node == null) throw new ModelPatchException("Graph is missing node 52");
        Object input = node.get("input");
        if (!(input instanceof List)) {
            throw new ModelPatchException("Node 52 has no input list");
        }
        java.util.List<List<Long>> pairs = new java.util.ArrayList<>();
        for (Object pair : (List<Object>) input) {
            if (!(pair instanceof List)) {
                throw new ModelPatchException("Node 52 input is malformed");
            }
            java.util.List<Long> values = new java.util.ArrayList<>();
            for (Object element : (List<Object>) pair) {
                values.add(((Number) element).longValue());
            }
            pairs.add(values);
        }
        return pairs;
    }

    private static List<List<Long>> patchedInput() {
        return List.of(List.of(18L, 0L), List.of(51L, 0L));
    }

    private static List<List<Long>> stockInput() {
        return List.of(List.of(50L, 0L), List.of(51L, 0L));
    }

    private static void validateContract(Map<String, Object> graph, boolean patched)
            throws ModelPatchException {
        Map<Long, Map<String, Object>> nodes = nodesById(graph);
        Map<String, Object> node18 = nodes.get(18L);
        if (node18 == null || !outputShape(node18, 0).equals(List.of(2L, 2L))) {
            throw new ModelPatchException("Unexpected local gaze node contract");
        }
        List<List<Long>> expected = patched ? patchedInput() : stockInput();
        if (!reshapeInput(graph).equals(expected)) {
            throw new ModelPatchException("Unexpected public gaze reshape input");
        }
        Map<String, Object> node52 = nodes.get(52L);
        if (!outputShape(node52, 0).equals(List.of(1L, 4L))) {
            throw new ModelPatchException("Unexpected public gaze output contract");
        }
        if (!graphOutputShapes(graph).equals(List.of(
                List.of(1L, 4L), List.of(1L, 6L), List.of(1L, 6L),
                List.of(1L, 1L), List.of(1L, 1L)))) {
            throw new ModelPatchException("Unexpected Seacliff model outputs");
        }
    }

    @SuppressWarnings("unchecked")
    private static List<Long> outputShape(Map<String, Object> node, int index)
            throws ModelPatchException {
        Object output = node.get("output");
        if (!(output instanceof List)) {
            throw new ModelPatchException("Node has no output list");
        }
        List<Object> outputs = (List<Object>) output;
        if (index >= outputs.size() || !(outputs.get(index) instanceof Map)) {
            throw new ModelPatchException("Node output is malformed");
        }
        return shape((Map<String, Object>) outputs.get(index));
    }

    @SuppressWarnings("unchecked")
    private static List<List<Long>> graphOutputShapes(Map<String, Object> graph)
            throws ModelPatchException {
        Object output = graph.get("output");
        if (!(output instanceof List)) {
            throw new ModelPatchException("Graph has no output list");
        }
        java.util.List<List<Long>> shapes = new java.util.ArrayList<>();
        for (Object item : (List<Object>) output) {
            if (!(item instanceof Map)) {
                throw new ModelPatchException("Graph output entry is malformed");
            }
            shapes.add(shape((Map<String, Object>) item));
        }
        return shapes;
    }

    @SuppressWarnings("unchecked")
    private static List<Long> shape(Map<String, Object> holder)
            throws ModelPatchException {
        Object shape = holder.get("shape");
        if (!(shape instanceof List)) {
            throw new ModelPatchException("Missing shape");
        }
        java.util.List<Long> values = new java.util.ArrayList<>();
        for (Object element : (List<Object>) shape) {
            values.add(((Number) element).longValue());
        }
        return values;
    }

    private static int countOccurrences(byte[] haystack, byte[] needle) {
        int count = 0;
        int from = 0;
        while (true) {
            int at = indexOf(haystack, needle, from);
            if (at < 0) break;
            count++;
            from = at + 1;
        }
        return count;
    }

    private static byte[] replaceFirst(byte[] haystack, byte[] needle, byte[] value) {
        int at = indexOf(haystack, needle, 0);
        byte[] result = haystack.clone();
        System.arraycopy(value, 0, result, at, value.length);
        return result;
    }

    private static int indexOf(byte[] haystack, byte[] needle, int from) {
        outer:
        for (int i = Math.max(0, from); i + needle.length <= haystack.length; i++) {
            for (int j = 0; j < needle.length; j++) {
                if (haystack[i + j] != needle[j]) continue outer;
            }
            return i;
        }
        return -1;
    }

    private static long readU16(byte[] data, int offset) {
        return (data[offset] & 0xffL) | ((data[offset + 1] & 0xffL) << 8);
    }

    private static long readU32(byte[] data, int offset) {
        return (data[offset] & 0xffL)
                | ((data[offset + 1] & 0xffL) << 8)
                | ((data[offset + 2] & 0xffL) << 16)
                | ((data[offset + 3] & 0xffL) << 24);
    }

    private static void putU32(byte[] data, int offset, long value) {
        data[offset] = (byte) (value & 0xff);
        data[offset + 1] = (byte) ((value >>> 8) & 0xff);
        data[offset + 2] = (byte) ((value >>> 16) & 0xff);
        data[offset + 3] = (byte) ((value >>> 24) & 0xff);
    }

    /** Lowercase hex SHA-256 of the bytes, used for the mount hash check. */
    public static String sha256Hex(byte[] data) {
        try {
            byte[] digest = java.security.MessageDigest.getInstance("SHA-256")
                    .digest(data);
            StringBuilder builder = new StringBuilder(digest.length * 2);
            for (byte b : digest) builder.append(String.format("%02x", b & 0xff));
            return builder.toString();
        } catch (java.security.NoSuchAlgorithmException error) {
            throw new IllegalStateException("SHA-256 unavailable", error);
        }
    }
}
