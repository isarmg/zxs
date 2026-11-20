package org.zipworkspace.preview;

import android.icu.lang.UCharacter;
import java.io.IOException;
import java.util.ArrayList;
import java.util.Arrays;
import java.nio.ByteBuffer;
import java.nio.charset.CharacterCodingException;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import java.text.Normalizer;
import java.util.List;
import java.util.Locale;
import java.util.TreeMap;

/** Bounded untrusted declarations. Whole-snapshot verification is an explicit separate read. */
final class ArchiveIndex {
    static final long MAX_ARCHIVE = (256L << 30) + (32L << 20), MAX_MEMBER = 128L << 30;
    static final int MAX_CD = 2 << 20, MAX_ENTRIES = 8192, MAX_NODES = 12288;
    static final int MAX_QUERY = 128; // UTF-16 code units; name search never interprets a path.
    static final long U32 = 0xffffffffL;
    private static final java.util.regex.Pattern DEVICE = java.util.regex.Pattern.compile(
            "CON|PRN|AUX|NUL|CLOCK\\$|CONIN\\$|CONOUT\\$|(?:COM|LPT)[1-9¹²³]");

    interface Source {
        byte[] read(long offset, int size) throws IOException; // exact positional read
        void check(); // cancellation, including between records in an already-read directory
    }
    interface MemberSource extends Source {
        void fill(long offset, byte[] buffer, int length) throws IOException; // exact, reusable buffer
    }
    static final class Slot {
        final int record, recordLength; // original central bytes, not a reconstructed filename
        final long offset, minimumEnd;
        final boolean centralZip64;
        long limit;                // next physical local header, or central directory
        Slot(int record, int recordLength, long offset, long minimumEnd, boolean centralZip64) {
            this.record = record; this.recordLength = recordLength;
            this.offset = offset; this.minimumEnd = minimumEnd; this.centralZip64 = centralZip64;
        }
    }
    static final class Item {
        final String name;
        final long bytes;
        final Item parent;
        final TreeMap<String, Item> children; // null for files; key = folded component
        boolean declared;
        private Slot slot;
        Item(String name, long bytes, Item parent, boolean folder) {
            this.name = name; this.bytes = bytes; this.parent = parent;
            children = folder ? new TreeMap<>() : null;
        }
    }
    static final class Match {
        final Item item;
        final int ordinal; // zero-based among ALL children of the actual parent
        Match(Item item, int ordinal) { this.item = item; this.ordinal = ordinal; }
    }

    final Item root = new Item("", 0, null, true);
    private final long centralAt;
    private final byte[] central, ending;
    private final int commentLength;
    private ArchiveIndex(long centralAt, byte[] central, byte[] ending, int commentLength) {
        this.centralAt = centralAt; this.central = central; this.ending = ending; this.commentLength = commentLength;
    }
    long bodyBytes() { return logicalSize; }
    int entryCount() { return nodes; }

    List<Match> findNames(String query, Runnable check) {
        if (query.isEmpty() || query.length() > MAX_QUERY)
            throw new IllegalArgumentException("Search query outside the size limit");
        ArrayList<Match> results = new ArrayList<>();
        check.run();
        findNames(root, query, check, results);
        check.run();
        return List.copyOf(results);
    }

    private static void findNames(Item parent, String query, Runnable check, ArrayList<Match> results) {
        // The parsed tree is no longer mutated. Build neither a second index
        // nor path strings for all matches (only the current UI page needs them).
        int ordinal = 0;
        for (Item child : parent.children.values()) {
            check.run();
            if (child.name.contains(query)) results.add(new Match(child, ordinal));
            ordinal++;
            if (child.children != null) findNames(child, query, check, results);
        }
    }

    static final class TextIndex {
        private final Item item;
        private final StoredMember.Layout local;
        private final IndexedText pages;
        private TextIndex(Item item, StoredMember.Layout local, IndexedText pages) {
            this.item = item; this.local = local; this.pages = pages;
        }
        int pages() { return pages.pages(); }
    }

    StoredMember.Result checkMember(MemberSource src, Item file) throws IOException {
        return checkMember(src, file, StoredMember.Mode.CRC);
    }
    StoredMember.Result checkMember(MemberSource src, Item file, boolean previewText) throws IOException {
        return checkMember(src, file, previewText ? StoredMember.Mode.PREVIEW : StoredMember.Mode.CRC);
    }
    StoredMember.Result checkMember(MemberSource src, Item file, StoredMember.Mode mode) throws IOException {
        require(file != null && file.children == null && file.slot != null);
        require(file.bytes >= 0 && file.bytes <= MAX_MEMBER);
        require(owns(file)); // a row from a former selection is not a capability on this index
        byte[] scratch = new byte[64 * 1024];
        checkDirectory(src, scratch);
        StoredMember.Layout local = StoredMember.layout(src, central, file.slot, file.bytes);
        byte[] prefix = mode == StoredMember.Mode.PREVIEW ? new byte[PlainText.captureLength(file.bytes)] : null;
        IndexedText.Build build = mode == StoredMember.Mode.INDEXED ? new IndexedText.Build(file.bytes) : null;
        long actual = StoredMember.crc(src, local, build == null ? scratch : new byte[IndexedText.BLOCK], prefix, build, null);
        checkDirectory(src, scratch); src.check();
        PlainText.Preview text = null;
        IndexedText indexed = null;
        if (local.crc == actual) {
            if (prefix != null) text = PlainText.decode(prefix, file.bytes, src::check);
            if (build != null) indexed = build.finish();
        }
        src.check();
        return new StoredMember.Result(local.crc, actual, text, indexed == null ? null : new TextIndex(file, local, indexed));
    }

    static final class SnapshotMarker {
        private static final Pattern CANONICAL = Pattern.compile(
                "zip-workspace/1\n\\{\"branch\":\"([0-9a-f]{32})\",\"generation\":([1-9][0-9]{0,18}),"
                + "\"operation\":\"([0-9a-f]{32})\",\"parent\":(?:null|\"([0-9a-f]{32})\"),"
                + "\"repo\":\"([0-9a-f]{32})\",\"tree\":\"([0-9a-f]{64})\"\\}");
        final String repo, branch, operation, parent, tree;
        final long generation;
        private SnapshotMarker(Matcher fields, long generation) {
            branch = fields.group(1); operation = fields.group(3); parent = fields.group(4);
            repo = fields.group(5); tree = fields.group(6); this.generation = generation;
        }
    }

    SnapshotMarker snapshotMarker() throws IOException {
        require(commentLength <= 1024);
        Matcher fields = SnapshotMarker.CANONICAL.matcher(new String(
                ending, ending.length - commentLength, commentLength, StandardCharsets.US_ASCII));
        require(fields.matches());
        final long generation;
        try { generation = Long.parseLong(fields.group(2)); }
        catch (NumberFormatException wrongRange) { throw new IOException("Snapshot generation outside the supported range", wrongRange); }
        SnapshotMarker marker = new SnapshotMarker(fields, generation);
        require((generation == 1 || marker.parent != null) && !marker.operation.equals(marker.parent));
        return marker;
    }

    private static final class SnapshotRow {
        final byte[] path;
        final Item item;
        SnapshotRow(String path, Item item) { this.path = path.getBytes(StandardCharsets.UTF_8); this.item = item; }
    }

    private static void collectSnapshotRows(Item parent, String prefix, ArrayList<SnapshotRow> rows, MemberSource src) {
        for (Item child : parent.children.values()) {
            src.check();
            String path = prefix.isEmpty() ? child.name : prefix + "/" + child.name;
            rows.add(new SnapshotRow(path, child));
            if (child.children != null) collectSnapshotRows(child, path, rows, src);
        }
    }

    private static MessageDigest sha256() {
        try { return MessageDigest.getInstance("SHA-256"); }
        catch (NoSuchAlgorithmException unsupported) { throw new IllegalStateException("SHA-256 unavailable", unsupported); }
    }

    private static String hex(byte[] digest) {
        StringBuilder out = new StringBuilder(digest.length * 2);
        for (byte b : digest) out.append(Character.forDigit((b >>> 4) & 15, 16)).append(Character.forDigit(b & 15, 16));
        return out.toString();
    }

    SnapshotMarker verifySnapshot(MemberSource src) throws IOException {
        src.check();
        SnapshotMarker marker = snapshotMarker();
        byte[] block = new byte[64 * 1024];
        checkDirectory(src, block);
        ArrayList<SnapshotRow> rows = new ArrayList<>(nodes);
        collectSnapshotRows(root, "", rows, src);
        require(rows.size() == nodes);
        // UTF-8 byte order is the producer's Unicode scalar order, not Java's
        // UTF-16 ordering, display ordering, or folded directory-map keys.
        rows.sort((a, b) -> Arrays.compareUnsigned(a.path, b.path));
        MessageDigest tree = sha256(), memberHash = sha256();
        tree.update("zip-workspace tree 1\0".getBytes(StandardCharsets.UTF_8));
        for (SnapshotRow row : rows) {
            src.check();
            Item item = row.item;
            boolean folder = item.children != null;
            String checksum = "null";
            if (item.slot != null) {
                StoredMember.Layout local = StoredMember.layout(src, central, item.slot, item.bytes);
                require(StoredMember.crc(src, local, block, null, null, memberHash) == local.crc);
                byte[] digest = memberHash.digest(); // resets the hasher for the next actual member
                if (!folder) checksum = "\"" + hex(digest) + "\"";
            } else require(folder && !item.declared);
            // Admitted path components do not contain JSON escape characters.
            byte[] suffix = ("\",\"" + (folder ? "d" : "f") + "\"," + item.bytes + "," + checksum + "]")
                    .getBytes(StandardCharsets.US_ASCII);
            int length = 2 + row.path.length + suffix.length;
            tree.update((byte)(length >>> 24)); tree.update((byte)(length >>> 16));
            tree.update((byte)(length >>> 8)); tree.update((byte)length);
            tree.update((byte)'['); tree.update((byte)'"'); tree.update(row.path); tree.update(suffix);
        }
        checkDirectory(src, block); src.check();
        require(hex(tree.digest()).equals(marker.tree));
        return marker;
    }

    String textPage(MemberSource src, Item file, TextIndex original, int page) throws IOException {
        require(original != null && original.item == file && owns(file));
        require(page >= 0 && page < original.pages());
        byte[] scratch = new byte[64 * 1024];
        checkSelectedRecord(src, file.slot, original.local, scratch);
        String result = original.pages.page(src, original.local.body, page);
        checkSelectedRecord(src, file.slot, original.local, scratch); src.check();
        return result;
    }

    private void checkSelectedRecord(MemberSource src, Slot slot, StoredMember.Layout local, byte[] scratch) throws IOException {
        StoredMember.match(src, centralAt + slot.record, central, slot.record, slot.recordLength, scratch);
        StoredMember.match(src, centralAt + central.length, ending, scratch);
        local.match(src, scratch);
    }
    boolean owns(Item node) {
        if (node == null) return false;
        while (node.parent != null) node = node.parent;
        return node == root;
    }

    private void checkDirectory(MemberSource src, byte[] block) throws IOException {
        StoredMember.match(src, centralAt, central, block);
        StoredMember.match(src, centralAt + central.length, ending, block);
    }

    private int nodes, names;
    private long logicalSize;

    static void require(boolean ok) throws IOException {
        if (!ok) throw new IOException("Unsupported or inconsistent directory");
    }
    static int u16(byte[] b, int i) {
        return (b[i] & 255) | (b[i + 1] & 255) << 8;
    }
    static long u32(byte[] b, int i) {
        return u16(b, i) | (long)u16(b, i + 2) << 16;
    }
    static long u64(byte[] b, int i) throws IOException {
        long high = u32(b, i + 4);
        require(high < 0x80000000L);
        return high << 32 | u32(b, i);
    }
    private static long wide(long old, long sentinel, long value) throws IOException {
        require(old == sentinel || old == value); return value;
    }

    static ArchiveIndex read(Source src, long size) throws IOException {
        require(size >= 22 && size <= MAX_ARCHIVE);
        int window = (int)Math.min(size, 22 + 65535);
        byte[] tail = src.read(size - window, window);
        int e = -1;
        for (int i = window - 22; i >= 0; i--) {
            if (u32(tail, i) == 0x06054b50 && u16(tail, i + 20) == window - i - 22) {
                require(e == -1); e = i;
            }
        }
        require(e >= 0 && u16(tail, e + 4) == 0 && u16(tail, e + 6) == 0);
        long localCount = u16(tail, e + 8), count = u16(tail, e + 10);
        long length = u32(tail, e + 12), at = u32(tail, e + 16);
        long end = size - window + e;
        byte[] loc = end >= 20 ? src.read(end - 20, 20) : new byte[0];
        byte[] ending;
        if (loc.length != 0 && u32(loc, 0) == 0x07064b50) {
            require(u32(loc, 4) == 0 && u32(loc, 16) == 1 && end >= 76);
            long z = u64(loc, 8); require(z == end - 76);
            byte[] rec = src.read(z, 56);
            require(u32(rec, 0) == 0x06064b50 && u64(rec, 4) == 44
                    && u16(rec, 14) == 45 && u32(rec, 16) == 0 && u32(rec, 20) == 0);
            localCount = wide(localCount, 65535, u64(rec, 24));
            count = wide(count, 65535, u64(rec, 32));
            length = wide(length, U32, u64(rec, 40));
            at = wide(at, U32, u64(rec, 48));
            ending = new byte[(int)(size - z)];
            System.arraycopy(rec, 0, ending, 0, 56);
            System.arraycopy(loc, 0, ending, 56, 20);
            System.arraycopy(tail, e, ending, 76, window - e);
            end = z;
        } else {
            ending = Arrays.copyOfRange(tail, e, window);
            require(localCount != 65535 && count != 65535 && at != U32 && length != U32);
        }
        require(count == localCount && count <= MAX_ENTRIES && length <= MAX_CD
                && at <= end && length == end - at && count <= length / 46);
        byte[] cd = src.read(at, (int)length);
        ArchiveIndex index = new ArchiveIndex(at, cd, ending, u16(tail, e + 20));
        TreeMap<Long, Slot> spans = new TreeMap<>();
        int p = 0;
        for (int n = 0; n < count; n++) {
            src.check();
            require(cd.length - p >= 46 && u32(cd, p) == 0x02014b50);
            int flags = u16(cd, p + 8), host = cd[p + 5] & 255;
            require((host == 0 || host == 3 || host == 10) && u16(cd, p + 6) <= 45
                    && (flags & ~(0x800 | 8)) == 0 && u16(cd, p + 10) == 0 && u16(cd, p + 34) == 0);
            int nl = u16(cd, p + 28), xl = u16(cd, p + 30), cl = u16(cd, p + 32);
            int start = p + 46, extra = start + nl, stop = extra + xl;
            require(nl > 0 && nl <= 1024 && stop + cl <= cd.length);
            long[] v = {u32(cd, p + 24), u32(cd, p + 20), u32(cd, p + 42)};
            boolean zip64 = false;
            for (int x = extra; x < stop;) {
                require(stop - x >= 4);
                int tag = u16(cd, x), k = u16(cd, x + 2); x += 4;
                require(k <= stop - x);
                if (tag == 1) {
                    require(!zip64 && k >= 8 && u16(cd, p + 6) == 45); zip64 = true;
                    int q = x;
                    for (int a = 0; a < v.length; a++) if (v[a] == U32) {
                        require(x + k - q >= 8); v[a] = u64(cd, q); q += 8;
                    }
                    require(q == x + k);
                } else {
                    require(tag == 0x5455 || tag == 0x7875); // time and Unix IDs do not name entries
                }
                x += k;
            }
            require(zip64 || (v[0] != U32 && v[1] != U32 && v[2] != U32));
            // Local headers have a 30-byte minimum and the same encoded name in this profile.
            // With bit 3, even the smallest descriptor without its optional signature uses 12 bytes.
            long overhead = 30L + nl + ((flags & 8) == 0 ? 0 : 12);
            require(v[0] == v[1] && v[0] <= MAX_MEMBER && v[2] <= at - overhead
                    && v[0] <= at - overhead - v[2]);
            Slot slot = new Slot(p, stop + cl - p, v[2], v[2] + overhead + v[0], zip64);
            require(spans.put(v[2], slot) == null);
            if ((flags & 0x800) == 0) for (int k = start; k < extra; k++) require(cd[k] >= 0);
            String name;
            try { name = StandardCharsets.UTF_8.newDecoder().decode(ByteBuffer.wrap(cd, start, nl)).toString(); }
            catch (CharacterCodingException ex) { throw new IOException("Unsupported name encoding", ex); }
            boolean folder = name.endsWith("/");
            long attr = u32(cd, p + 38);
            int type = (int)((attr >>> 28) & 15); // Unix mode's S_IFMT
            require((attr & (8 | 0x40 | 0x400)) == 0 && ((attr & 16) == 0 || folder)
                    && (host == 3 ? type == 0 || type == (folder ? 4 : 8) : type == 0)
                    && (!folder || v[0] == 0));
            Item row = index.add(folder ? name.substring(0, name.length() - 1) : name, folder, v[0], nl);
            row.slot = slot;
            p = stop + cl;
        }
        require(p == cd.length);
        long limit = at;
        for (Slot span : spans.descendingMap().values()) {
            src.check(); require(span.minimumEnd <= limit);
            span.limit = limit; limit = span.offset;
        }
        src.check();
        return index;
    }

    static boolean safeComponent(String s) {
        if (s.isEmpty() || s.getBytes(StandardCharsets.UTF_8).length > 240
                || !Normalizer.isNormalized(s, Normalizer.Form.NFC)
                || s.endsWith(".") || s.endsWith(" ") || s.startsWith(" ")) return false;
        for (int i = 0; i < s.length();) {
            int c = s.codePointAt(i); i += Character.charCount(c);
            int t = Character.getType(c);
            if (Character.isISOControl(c) || t == Character.FORMAT || t == Character.LINE_SEPARATOR
                    || t == Character.PARAGRAPH_SEPARATOR || "<>:\"/\\|?*".indexOf(c) >= 0) return false;
        }
        String prefix = s.split("\\.", 2)[0].stripTrailing().toUpperCase(Locale.ROOT);
        return !DEVICE.matcher(prefix).matches();
    }

    private Item add(String path, boolean folder, long bytes, int encoded) throws IOException {
        names += encoded; logicalSize += bytes;
        require(names <= 512 * 1024 && logicalSize <= (256L << 30));
        String[] parts = path.split("/", -1); require(parts.length <= 48);
        Item parent = root;
        for (int i = 0; i < parts.length; i++) {
            String s = parts[i]; require(safeComponent(s));
            boolean leaf = i + 1 == parts.length, dir = !leaf || folder;
            String key = UCharacter.foldCase(s, true);
            Item item = parent.children.get(key);
            if (item == null) {
                require(++nodes <= MAX_NODES);
                item = new Item(s, leaf ? bytes : 0, parent, dir);
                parent.children.put(key, item);
            } else require(s.equals(item.name) && dir == (item.children != null));
            if (leaf) { require(!item.declared); item.declared = true; }
            parent = item;
        }
        return parent;
    }
}
