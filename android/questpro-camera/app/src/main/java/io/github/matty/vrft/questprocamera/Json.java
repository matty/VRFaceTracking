package io.github.matty.vrft.questprocamera;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * Minimal, dependency-free JSON reader/writer.
 *
 * <p>Pure Java (no Android and no org.json) so it can be unit-tested with a
 * plain JDK. {@link #parseFirst(String)} mirrors Python's
 * {@code json.JSONDecoder().raw_decode}: it decodes a single JSON value at the
 * start of the text and ignores any trailing bytes, which is how the Seacliff
 * graph is recovered from the surrounding pickle payload.
 *
 * <p>Values map to: {@link Map}{@code <String,Object>} (objects, insertion
 * ordered), {@link List}{@code <Object>} (arrays), {@link String},
 * {@link Long} or {@link Double} (numbers), {@link Boolean}, or {@code null}.
 */
public final class Json {
    private final String text;
    private int index;

    private Json(String text) {
        this.text = text;
    }

    /** Parse the whole string as one JSON value; trailing junk is an error. */
    public static Object parse(String text) {
        Json parser = new Json(text);
        parser.skipWhitespace();
        Object value = parser.readValue();
        parser.skipWhitespace();
        if (parser.index != text.length()) {
            throw new IllegalArgumentException("Trailing data after JSON value");
        }
        return value;
    }

    /** Parse one JSON value at the start of the text; ignore the remainder. */
    public static Object parseFirst(String text) {
        Json parser = new Json(text);
        parser.skipWhitespace();
        return parser.readValue();
    }

    private Object readValue() {
        if (index >= text.length()) throw error("Unexpected end of input");
        char c = text.charAt(index);
        switch (c) {
            case '{': return readObject();
            case '[': return readArray();
            case '"': return readString();
            case 't': return readLiteral("true", Boolean.TRUE);
            case 'f': return readLiteral("false", Boolean.FALSE);
            case 'n': return readLiteral("null", null);
            default:
                if (c == '-' || (c >= '0' && c <= '9')) return readNumber();
                throw error("Unexpected character '" + c + "'");
        }
    }

    private Map<String, Object> readObject() {
        Map<String, Object> object = new LinkedHashMap<>();
        index++; // '{'
        skipWhitespace();
        if (peek() == '}') { index++; return object; }
        while (true) {
            skipWhitespace();
            if (peek() != '"') throw error("Expected object key");
            String key = readString();
            skipWhitespace();
            if (peek() != ':') throw error("Expected ':'");
            index++;
            skipWhitespace();
            object.put(key, readValue());
            skipWhitespace();
            char c = peek();
            if (c == ',') { index++; continue; }
            if (c == '}') { index++; return object; }
            throw error("Expected ',' or '}'");
        }
    }

    private List<Object> readArray() {
        List<Object> array = new ArrayList<>();
        index++; // '['
        skipWhitespace();
        if (peek() == ']') { index++; return array; }
        while (true) {
            skipWhitespace();
            array.add(readValue());
            skipWhitespace();
            char c = peek();
            if (c == ',') { index++; continue; }
            if (c == ']') { index++; return array; }
            throw error("Expected ',' or ']'");
        }
    }

    private String readString() {
        StringBuilder builder = new StringBuilder();
        index++; // opening quote
        while (index < text.length()) {
            char c = text.charAt(index++);
            if (c == '"') return builder.toString();
            if (c == '\\') {
                if (index >= text.length()) throw error("Unterminated escape");
                char esc = text.charAt(index++);
                switch (esc) {
                    case '"': builder.append('"'); break;
                    case '\\': builder.append('\\'); break;
                    case '/': builder.append('/'); break;
                    case 'b': builder.append('\b'); break;
                    case 'f': builder.append('\f'); break;
                    case 'n': builder.append('\n'); break;
                    case 'r': builder.append('\r'); break;
                    case 't': builder.append('\t'); break;
                    case 'u':
                        if (index + 4 > text.length()) throw error("Bad \\u escape");
                        builder.append((char) Integer.parseInt(
                                text.substring(index, index + 4), 16));
                        index += 4;
                        break;
                    default: throw error("Bad escape '\\" + esc + "'");
                }
            } else {
                builder.append(c);
            }
        }
        throw error("Unterminated string");
    }

    private Object readNumber() {
        int start = index;
        boolean floating = false;
        if (peek() == '-') index++;
        while (index < text.length()) {
            char c = text.charAt(index);
            if (c >= '0' && c <= '9') { index++; }
            else if (c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-') {
                floating = true; index++;
            } else break;
        }
        String token = text.substring(start, index);
        if (floating) return Double.parseDouble(token);
        try {
            return Long.parseLong(token);
        } catch (NumberFormatException overflow) {
            return Double.parseDouble(token);
        }
    }

    private Object readLiteral(String literal, Object value) {
        if (!text.regionMatches(index, literal, 0, literal.length())) {
            throw error("Expected '" + literal + "'");
        }
        index += literal.length();
        return value;
    }

    private char peek() {
        if (index >= text.length()) throw error("Unexpected end of input");
        return text.charAt(index);
    }

    private void skipWhitespace() {
        while (index < text.length()) {
            char c = text.charAt(index);
            if (c == ' ' || c == '\t' || c == '\n' || c == '\r') index++;
            else break;
        }
    }

    private IllegalArgumentException error(String message) {
        return new IllegalArgumentException(message + " at offset " + index);
    }

    /** Serialize a value tree (Map/List/String/Number/Boolean/null) to JSON. */
    public static String write(Object value) {
        StringBuilder builder = new StringBuilder();
        writeValue(builder, value);
        return builder.toString();
    }

    @SuppressWarnings("unchecked")
    private static void writeValue(StringBuilder builder, Object value) {
        if (value == null) { builder.append("null"); return; }
        if (value instanceof String) { writeString(builder, (String) value); return; }
        if (value instanceof Boolean || value instanceof Number) {
            builder.append(value.toString());
            return;
        }
        if (value instanceof Map) {
            builder.append('{');
            boolean first = true;
            for (Map.Entry<String, Object> entry : ((Map<String, Object>) value).entrySet()) {
                if (!first) builder.append(',');
                first = false;
                writeString(builder, entry.getKey());
                builder.append(':');
                writeValue(builder, entry.getValue());
            }
            builder.append('}');
            return;
        }
        if (value instanceof Iterable) {
            builder.append('[');
            boolean first = true;
            for (Object item : (Iterable<Object>) value) {
                if (!first) builder.append(',');
                first = false;
                writeValue(builder, item);
            }
            builder.append(']');
            return;
        }
        throw new IllegalArgumentException("Cannot serialize " + value.getClass());
    }

    private static void writeString(StringBuilder builder, String value) {
        builder.append('"');
        for (int i = 0; i < value.length(); i++) {
            char c = value.charAt(i);
            switch (c) {
                case '"': builder.append("\\\""); break;
                case '\\': builder.append("\\\\"); break;
                case '\n': builder.append("\\n"); break;
                case '\r': builder.append("\\r"); break;
                case '\t': builder.append("\\t"); break;
                case '\b': builder.append("\\b"); break;
                case '\f': builder.append("\\f"); break;
                default:
                    if (c < 0x20) builder.append(String.format("\\u%04x", (int) c));
                    else builder.append(c);
            }
        }
        builder.append('"');
    }
}
