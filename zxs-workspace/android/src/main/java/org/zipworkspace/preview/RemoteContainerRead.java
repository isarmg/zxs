package org.zipworkspace.preview;

import android.util.JsonReader;
import android.util.JsonToken;

import java.io.IOException;
import java.io.StringReader;
import java.math.BigInteger;
import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.regex.Pattern;

/** Strict explicit metadata selection for Android's single-document remote editor. */
final class RemoteContainerRead {
    static final int METADATA_LIMIT = 1 << 20;
    private static final int PAGE = 256;
    private static final int MAX_HEAD_PAGES = 8;
    private static final String ROOT = "/zxs/v1/containers";
    private static final Pattern ID = Pattern.compile("[0-9a-f]{32}");
    private static final Pattern SHA = Pattern.compile("[0-9a-f]{64}");
    private static final Pattern UINT64 = Pattern.compile("0|[1-9][0-9]{0,19}");
    private static final BigInteger MAX = BigInteger.ONE.shiftLeft(64).subtract(BigInteger.ONE);
    private static final String ZERO = "0".repeat(32);
    private static final Set<String> CATALOG_ROW = Set.of("container_id", "sequence", "name");
    private static final Set<String> DETAIL = Set.of("container_id", "sequence", "name", "tags", "heads", "next_after");
    private static final Set<String> HEAD = Set.of("branch", "head");
    private static final Set<String> NODE = Set.of(
            "node_id", "operation_id", "parent_node", "original_branch", "sequence", "length", "sha256");

    interface Reader {
        RemoteSaveJournal.Authority authority();
        byte[] metadata(String path, Map<String, String> headers) throws IOException;
        byte[] node(String container, String node, int length, String sha256) throws IOException;
    }

    record Item(String container, BigInteger sequence, String name) {}
    record Catalog(List<Item> items, String next) {}
    record Head(String branch, String node) {}
    record Detail(Item item, List<String> tags, List<Head> heads, String next) {}
    record Revision(String node, String parent, BigInteger sequence, BigInteger length, String sha256) {}

    record Selection(RemoteSaveJournal.Authority authority, Item item, Revision revision) {
        boolean differs(ByteBuffer bytes) {
            if (!revision.length().equals(BigInteger.valueOf(bytes.remaining()))) return true;
            var digest = ArchiveIndex.sha256();
            digest.update(bytes.duplicate());
            return !revision.sha256().equals(ArchiveIndex.hex(digest.digest()));
        }

        RemoteSaveJournal.Plan changedMain(String operation, String node, byte[] bytes) {
            String digest = ArchiveIndex.hex(ArchiveIndex.sha256().digest(bytes));
            if (bytes.length > PlainText.LIMIT || (BigInteger.valueOf(bytes.length).equals(revision.length())
                    && digest.equals(revision.sha256()))) {
                throw new IllegalArgumentException("no changed bounded body");
            }
            var save = new RemoteSaveContract.Save(item.container(), operation, node, item.sequence(),
                    new RemoteSaveContract.BranchHead("main", revision.node()), bytes.length, digest);
            return new RemoteSaveJournal.Plan(authority, save);
        }

        byte[] readExact(Reader peer) throws IOException {
            require(authority.equals(peer.authority()) && revision.length().compareTo(
                    BigInteger.valueOf(PlainText.LIMIT)) <= 0);
            byte[] raw = peer.node(item.container(), revision.node(), revision.length().intValueExact(), revision.sha256());
            require(authority.equals(peer.authority()) && raw != null && raw.length == revision.length().intValueExact()
                    && revision.sha256().equals(ArchiveIndex.hex(ArchiveIndex.sha256().digest(raw))));
            return raw;
        }
    }

    static Catalog catalog(Reader peer, RemoteSaveJournal.Authority exact, String after) throws IOException {
        require(exact.equals(peer.authority()) && (after == null || id(after)));
        var headers = after == null ? Map.of("Zxs-View", "compact")
                : Map.of("Zxs-View", "compact", "Zxs-After", after);
        byte[] bytes = peer.metadata(ROOT, headers);
        require(exact.equals(peer.authority()));
        var raw = object(parse(bytes), Set.of("items", "next_after"));
        List<?> rows = list(raw.get("items"));
        var items = new ArrayList<Item>(rows.size());
        String previous = after;
        for (Object entry : rows) {
            Item item = item(object(entry, CATALOG_ROW));
            require(previous == null || item.container().compareTo(previous) > 0);
            previous = item.container();
            items.add(item);
        }
        String next = nullableId(raw.get("next_after"));
        require(java.util.Objects.equals(next, rows.size() == PAGE ? previous : null));
        return new Catalog(List.copyOf(items), next);
    }

    /** Recheck exact selected revision and read a bounded anchored main page. */
    static Selection selectMain(Reader peer, RemoteSaveJournal.Authority exact, Item selected)
            throws IOException {
        require(exact.equals(peer.authority()) && id(selected.container()));
        String next = null;
        String main = null;
        List<String> tags = null;
        String path = ROOT + "/" + selected.container();
        for (int page = 0; page < MAX_HEAD_PAGES; page++) {
            var headers = new HashMap<String, String>();
            headers.put("Zxs-Detail", "paged");
            headers.put("Zxs-Detail-Sequence", selected.sequence().toString());
            if (next != null) headers.put("Zxs-Detail-After", next);
            byte[] bytes = peer.metadata(path, Map.copyOf(headers));
            require(exact.equals(peer.authority()));
            Detail detail = detail(bytes, next);
            require(selected.equals(detail.item()) && (tags == null || tags.equals(detail.tags())));
            tags = detail.tags();
            for (Head head : detail.heads()) {
                if (head.branch().equals("main")) main = head.node();
            }
            next = detail.next();
            if (next == null) break;
        }
        require(next == null && main != null);
        byte[] history = peer.metadata(path + "/branches/main/nodes", Map.of("Zxs-History-Head", main));
        require(exact.equals(peer.authority()));
        var revision = currentRevision(history, main, selected.sequence());
        return new Selection(exact, selected, revision);
    }

    static Detail detail(byte[] bytes, String after) throws IOException {
        var raw = object(parse(bytes), DETAIL);
        var item = item(raw);
        var tagValues = new ArrayList<String>();
        var tags = new HashSet<String>();
        for (Object entry : list(raw.get("tags"))) {
            String tag = string(entry);
            require(!tag.isEmpty() && tags.add(tag));
            tagValues.add(tag);
        }
        List<?> rows = list(raw.get("heads"));
        var heads = new ArrayList<Head>();
        String previous = after;
        for (Object entry : rows) {
            var row = object(entry, HEAD);
            String branch = string(row.get("branch"));
            String node = string(row.get("head"));
            require(branch(branch) && id(node) && (previous == null || branch.compareTo(previous) > 0));
            heads.add(new Head(branch, node));
            previous = branch;
        }
        Object nextRaw = raw.get("next_after");
        String next = nextRaw == null ? null : string(nextRaw);
        require(java.util.Objects.equals(next, rows.size() == PAGE ? previous : null));
        return new Detail(item, List.copyOf(tagValues), List.copyOf(heads), next);
    }

    static Revision currentRevision(byte[] bytes, String head, BigInteger ownerSequence) throws IOException {
        require(id(head));
        var raw = object(parse(bytes), Set.of("head", "nodes", "next"));
        require(head.equals(raw.get("head")));
        var rows = list(raw.get("nodes"));
        require(!rows.isEmpty());
        var seen = new HashSet<String>();
        Revision first = null;
        String expected = head;
        BigInteger ceiling = ownerSequence.add(BigInteger.ONE);
        for (Object entry : rows) {
            var row = object(entry, NODE);
            String node = string(row.get("node_id"));
            String operation = string(row.get("operation_id"));
            String origin = string(row.get("original_branch"));
            String parent = nullableId(row.get("parent_node"));
            String sha256 = string(row.get("sha256"));
            BigInteger sequence = uint(row.get("sequence"));
            BigInteger length = uint(row.get("length"));
            require(id(operation) && branch(origin) && node.equals(expected) && seen.add(node)
                    && SHA.matcher(sha256).matches() && sequence.signum() > 0 && sequence.compareTo(ceiling) < 0);
            if (first == null) first = new Revision(node, parent, sequence, length, sha256);
            expected = parent;
            ceiling = sequence;
        }
        String next = nullableId(raw.get("next"));
        require(java.util.Objects.equals(next, expected)
                && (next == null || (rows.size() == PAGE && !seen.contains(next))));
        return first;
    }

    private static Item item(Map<?, ?> row) throws IOException {
        String id = string(row.get("container_id"));
        BigInteger sequence = uint(row.get("sequence"));
        require(id(id) && sequence.signum() > 0 && sequence.compareTo(MAX) < 0);
        String name = string(row.get("name"));
        try {
            new RemoteSaveContract.FirstMain(name); // Same server display spelling, with no local case fold.
        } catch (IllegalArgumentException invalid) {
            throw new IOException("invalid remote display name");
        }
        return new Item(id, sequence, name);
    }

    private static boolean branch(String value) { return "main".equals(value) || id(value); }

    static boolean id(String value) { return value != null && ID.matcher(value).matches() && !ZERO.equals(value); }

    private static String nullableId(Object value) throws IOException {
        if (value == null) return null;
        String text = string(value);
        require(id(text));
        return text;
    }

    private static String string(Object value) throws IOException {
        require(value instanceof String);
        return (String) value;
    }

    private static BigInteger uint(Object value) throws IOException {
        require(value instanceof BigInteger);
        return (BigInteger) value;
    }

    private static Map<?, ?> object(Object value, Set<String> keys) throws IOException {
        if (!(value instanceof Map<?, ?> map) || !keys.equals(map.keySet())) {
            throw new IOException("invalid remote metadata fields");
        }
        return map;
    }

    private static List<?> list(Object value) throws IOException {
        require(value instanceof List<?>);
        return (List<?>) value;
    }

    private static Object parse(byte[] bytes) throws IOException {
        require(bytes != null && bytes.length <= METADATA_LIMIT);
        String text = StandardCharsets.UTF_8.newDecoder().decode(ByteBuffer.wrap(bytes)).toString();
        try (var reader = new JsonReader(new StringReader(text))) {
            Object result = read(reader, 0);
            require(reader.peek() == JsonToken.END_DOCUMENT);
            return result;
        } catch (IllegalStateException invalid) {
            throw new IOException("invalid remote metadata");
        }
    }

    private static Object read(JsonReader reader, int depth) throws IOException {
        require(depth <= 4);
        return switch (reader.peek()) {
            case BEGIN_OBJECT -> {
                var fields = new HashMap<String, Object>();
                reader.beginObject();
                while (reader.hasNext()) {
                    String name = reader.nextName();
                    require(fields.size() < 8 && !fields.containsKey(name));
                    fields.put(name, read(reader, depth + 1));
                }
                reader.endObject();
                yield fields;
            }
            case BEGIN_ARRAY -> {
                var rows = new ArrayList<>();
                reader.beginArray();
                while (reader.hasNext()) {
                    require(rows.size() < PAGE);
                    rows.add(read(reader, depth + 1));
                }
                reader.endArray();
                yield rows;
            }
            case NUMBER -> {
                String text = reader.nextString();
                require(UINT64.matcher(text).matches());
                BigInteger number = new BigInteger(text);
                require(number.compareTo(MAX) <= 0);
                yield number;
            }
            case STRING -> {
                String text = reader.nextString();
                require(text.length() <= 8192 && StandardCharsets.UTF_8.newEncoder().canEncode(text));
                yield text;
            }
            case NULL -> {
                reader.nextNull();
                yield null;
            }
            default -> throw new IOException("unexpected remote metadata value");
        };
    }

    private static void require(boolean valid) throws IOException {
        if (!valid) throw new IOException("remote metadata did not match the selected original");
    }

    private RemoteContainerRead() {}
}
