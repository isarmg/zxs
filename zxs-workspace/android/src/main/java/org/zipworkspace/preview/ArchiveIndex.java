package org.zipworkspace.preview;

import android.icu.lang.UCharacter;
import java.io.IOException;
import java.util.Arrays;
import java.nio.ByteBuffer;
import java.nio.charset.CharacterCodingException;
import java.nio.charset.StandardCharsets;
import java.text.Normalizer;
import java.util.Locale;
import java.util.TreeMap;

/** Bounded, untrusted directory declarations. No tree identity or automatic member reads. */
final class ArchiveIndex {
    static final long MAX_ARCHIVE = (256L << 30) + (32L << 20);
    static final int MAX_CD = 2 << 20, MAX_ENTRIES = 8192, MAX_NODES = 12288;
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
        final int record;          // original central record, not a reconstructed filename
        final long offset, minimumEnd;
        final boolean centralZip64;
        long limit;                // next physical local header, or central directory
        Slot(int record, long offset, long minimumEnd, boolean centralZip64) {
            this.record = record; this.offset = offset; this.minimumEnd = minimumEnd;
            this.centralZip64 = centralZip64;
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
    final Item root = new Item("", 0, null, true);
    private final long centralAt;
    private final byte[] central, ending;
    private ArchiveIndex(long centralAt, byte[] central, byte[] ending) {
        this.centralAt = centralAt; this.central = central; this.ending = ending;
    }

    StoredMember.Result checkMember(MemberSource src, Item file) throws IOException {
        require(file != null && file.children == null && file.slot != null);
        Item ancestor = file;
        while (ancestor.parent != null) ancestor = ancestor.parent;
        require(ancestor == root); // a row from a former selection is not a capability on this index
        byte[] block = new byte[64 * 1024];
        checkDirectory(src, block);
        StoredMember.Result result = StoredMember.check(src, central, file.slot, file.bytes, block);
        checkDirectory(src, block); src.check();
        return result;
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
        ArchiveIndex index = new ArchiveIndex(at, cd, ending);
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
            require(v[0] == v[1] && v[0] <= (128L << 30) && v[2] <= at - overhead
                    && v[0] <= at - overhead - v[2]);
            Slot slot = new Slot(p, v[2], v[2] + overhead + v[0], zip64);
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
            if (!folder) row.slot = slot;
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
