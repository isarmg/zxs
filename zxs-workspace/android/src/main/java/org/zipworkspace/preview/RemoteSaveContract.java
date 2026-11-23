package org.zipworkspace.preview;

import android.util.JsonReader;
import android.util.JsonToken;

import java.io.IOException;
import java.io.StringReader;
import java.math.BigInteger;
import java.nio.ByteBuffer;
import java.nio.charset.CodingErrorAction;
import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.HexFormat;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import java.util.regex.Pattern;

/** R3 whole-file saves, independent of transport, credentials and local draft ownership. */
final class RemoteSaveContract {
    private static final String ROOT = "/zxs/v1/containers/";
    private static final int MAX_REPLY_BYTES = 16 * 1024;
    private static final Pattern ID = Pattern.compile("[0-9a-f]{32}");
    private static final Pattern SHA256 = Pattern.compile("[0-9a-f]{64}");
    private static final Pattern UINT64 = Pattern.compile("0|[1-9][0-9]{0,19}");
    private static final BigInteger RESERVED_SEQUENCE = BigInteger.ONE.shiftLeft(64).subtract(BigInteger.ONE);
    private static final String ZERO_ID = "0".repeat(32);

    sealed interface Target permits FirstMain, BranchHead, HistoricalMain {
        String branch();
    }

    record FirstMain(String name) implements Target {
        FirstMain {
            if (name == null || name.isEmpty() || !StandardCharsets.UTF_8.newEncoder().canEncode(name)
                    || name.endsWith(".") || name.endsWith(" ") || name.length() > 255
                    || name.codePoints().anyMatch(c -> Character.isISOControl(c) || "<>:\"/\\|?*".indexOf(c) >= 0)
                    || name.getBytes(StandardCharsets.UTF_8).length > 255) {
                throw new IllegalArgumentException("not a single valid display name");
            }
        }

        @Override
        public String branch() { return "main"; }
    }

    record BranchHead(String branch, String expectedHead) implements Target {
        BranchHead {
            if (!"main".equals(branch)) requireId(branch);
            requireId(expectedHead);
        }
    }

    record HistoricalMain(String branch, String fromNode) implements Target {
        HistoricalMain {
            requireId(branch);
            requireId(fromNode);
        }
    }

    /** Metadata only: the eventual sender must hold and check the frozen complete bytes. */
    record Save(String container, String operation, String node, BigInteger baseSequence,
                Target target, long length, String sha256) {
        Save {
            requireId(container);
            requireId(operation);
            requireId(node);
            Objects.requireNonNull(baseSequence);
            Objects.requireNonNull(target);
            if (baseSequence.signum() < 0 || baseSequence.compareTo(RESERVED_SEQUENCE) >= 0
                    || (baseSequence.signum() == 0) != (target instanceof FirstMain)
                    || length < 0 || sha256 == null || !SHA256.matcher(sha256).matches()
                    || (target instanceof BranchHead head && node.equals(head.expectedHead()))
                    || (target instanceof HistoricalMain old && node.equals(old.fromNode()))) {
                throw new IllegalArgumentException("invalid original whole-file save");
            }
        }

        String lookupPath() {
            return ROOT + container + "/operations/" + operation;
        }

        String submitPath() {
            return ROOT + container + "/branches/" + target.branch() + "/nodes";
        }

        Map<String, String> submitHeaders() {
            var headers = new HashMap<String, String>();
            headers.put("Content-Type", "application/octet-stream");
            headers.put("Content-Length", Long.toString(length));
            headers.put("Zxs-Operation", operation);
            headers.put("Zxs-Node", node);
            headers.put("Zxs-Sha256", sha256);
            if (target instanceof FirstMain first) {
                headers.put("Zxs-Base-Node", "0");
                headers.put("Zxs-Initial-Name", HexFormat.of().formatHex(
                        first.name().getBytes(StandardCharsets.UTF_8)));
            } else if (target instanceof BranchHead head) {
                headers.put("Zxs-Base-Node", head.expectedHead());
            } else if (target instanceof HistoricalMain old) {
                headers.put("Zxs-Fork-From-Main", old.fromNode());
            }
            return Map.copyOf(headers);
        }
    }

    enum Outcome { CONFIRMED, ABSENT, CONFLICT, UNKNOWN }

    record Reply(Outcome outcome, BigInteger sequence) {}

    private static final Reply ABSENT = new Reply(Outcome.ABSENT, null);
    private static final Reply CONFLICT = new Reply(Outcome.CONFLICT, null);
    private static final Reply UNKNOWN = new Reply(Outcome.UNKNOWN, null);

    /** Only a fully received authenticated lookup can report absence; not a POST or generic 404. */
    static Reply project(Save save, boolean lookup, int status, byte[] raw) {
        if (status == 409) return CONFLICT;
        if (raw == null || raw.length > MAX_REPLY_BYTES
                || (status != 200 && (status != 404 || !lookup))) return UNKNOWN;
        try {
            String decoded = StandardCharsets.UTF_8.newDecoder()
                    .onMalformedInput(CodingErrorAction.REPORT)
                    .onUnmappableCharacter(CodingErrorAction.REPORT)
                    .decode(ByteBuffer.wrap(raw)).toString();
            try (var reader = new JsonReader(new StringReader(decoded))) {
                Map<String, Object> object = readObject(reader, true);
                if (reader.peek() != JsonToken.END_DOCUMENT) return UNKNOWN;
                if (status == 404) {
                    return object.equals(Map.of("state", "not_found")) ? ABSENT : UNKNOWN;
                }
                return confirm(save, object);
            }
        } catch (IOException | IllegalStateException invalid) {
            return UNKNOWN;
        }
    }

    private static Reply confirm(Save save, Map<String, Object> object) {
        if (!object.keySet().equals(Set.of("kind", "container_id", "operation_id", "sequence",
                "node_id", "length", "sha256", "target"))
                || !"save_node".equals(object.get("kind"))
                || !save.container().equals(object.get("container_id"))
                || !save.operation().equals(object.get("operation_id"))
                || !save.node().equals(object.get("node_id"))
                || !save.sha256().equals(object.get("sha256"))
                || !BigInteger.valueOf(save.length()).equals(object.get("length"))
                || !expectedTarget(save.target()).equals(object.get("target"))
                || !(object.get("sequence") instanceof BigInteger seq)
                || seq.compareTo(save.baseSequence()) <= 0 || seq.compareTo(RESERVED_SEQUENCE) >= 0) {
            return UNKNOWN;
        }
        return new Reply(Outcome.CONFIRMED, seq);
    }

    private static Map<String, String> expectedTarget(Target target) {
        if (target instanceof FirstMain first) {
            return Map.of("kind", "first_main", "name", first.name());
        }
        if (target instanceof BranchHead head) {
            return Map.of("kind", "branch", "branch", head.branch(),
                    "expected_head", head.expectedHead());
        }
        var old = (HistoricalMain) target;
        return Map.of("kind", "historical_main", "branch", old.branch(), "from_node", old.fromNode());
    }

    private static Map<String, Object> readObject(JsonReader reader, boolean outer) throws IOException {
        var fields = new HashMap<String, Object>();
        reader.beginObject();
        while (reader.hasNext()) {
            String name = reader.nextName();
            if (fields.containsKey(name) || fields.size() == 8) throw new IOException("invalid reply fields");
            Object value = switch (reader.peek()) {
                case STRING -> reader.nextString();
                case NUMBER -> readUnsigned(reader);
                case BEGIN_OBJECT -> {
                    if (!outer || !"target".equals(name)) throw new IOException("invalid reply shape");
                    yield readObject(reader, false);
                }
                default -> throw new IOException("invalid reply value");
            };
            fields.put(name, value);
        }
        reader.endObject();
        return fields;
    }

    private static BigInteger readUnsigned(JsonReader reader) throws IOException {
        String raw = reader.nextString(); // Caller only admits a NUMBER token, never a quoted value.
        if (!UINT64.matcher(raw).matches()) throw new IOException("invalid unsigned integer");
        BigInteger value = new BigInteger(raw);
        if (value.compareTo(RESERVED_SEQUENCE) > 0) throw new IOException("unsigned integer overflow");
        return value;
    }

    private static void requireId(String value) {
        if (value == null || !ID.matcher(value).matches() || ZERO_ID.equals(value)) {
            throw new IllegalArgumentException("invalid R3 identity");
        }
    }

    private RemoteSaveContract() {}
}
