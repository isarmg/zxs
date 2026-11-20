package org.zipworkspace.preview;

import java.io.IOException;
import java.util.Arrays;
import java.util.zip.CRC32;
import static org.zipworkspace.preview.ArchiveIndex.*;

/** One stored body from one displayed central record. Neither authentication nor extraction. */
final class StoredMember {
    static final class Result {
        final long expected, actual;
        final PlainText.Preview text; // null for CRC-only, nonmatching CRC, or unsupported display text
        Result(long expected, long actual) { this(expected, actual, null); }
        Result(long expected, long actual, PlainText.Preview text) {
            this.expected = expected; this.actual = actual; this.text = text;
        }
        boolean matches() { return expected == actual; }
    }

    static void match(MemberSource src, long at, byte[] expected, byte[] block) throws IOException {
        for (int i = 0; i < expected.length;) {
            src.check();
            int n = Math.min(block.length, expected.length - i);
            src.fill(at + i, block, n);
            require(Arrays.equals(block, 0, n, expected, i, i + n));
            i += n;
        }
    }

    static Result check(MemberSource src, byte[] cd, Slot slot, long bytes, byte[] block, byte[] capture) throws IOException {
        final int p = slot.record, flags = u16(cd, p + 8), nameLength = u16(cd, p + 28);
        final long crc = u32(cd, p + 16);
        final boolean streaming = (flags & 8) != 0;
        require(slot.limit - slot.offset >= 30L + nameLength);
        byte[] h = src.read(slot.offset, 30);
        int version = u16(h, 4), name = u16(h, 26), extra = u16(h, 28);
        // The central version may be raised to 4.5 solely for a ZIP64 offset.
        require(u32(h, 0) == 0x04034b50 && version >= 10 && version <= 45
                && (version == u16(cd, p + 6) || (u16(cd, p + 6) == 45 && u32(cd, p + 42) == U32))
                && u16(h, 6) == flags && u16(h, 8) == 0 && u32(h, 10) == u32(cd, p + 12)
                && name == nameLength && 30L + name + extra <= slot.limit - slot.offset);
        long body = slot.offset + 30L + name + extra;
        require(bytes <= slot.limit - body);
        byte[] fields = src.read(slot.offset + 30, name + extra);
        require(Arrays.equals(fields, 0, name, cd, p + 46, p + 46 + name));
        long packed = u32(h, 18), plain = u32(h, 22);
        boolean wide = packed == U32 || plain == U32, seenWide = false;
        for (int x = name; x < fields.length;) {
            require(fields.length - x >= 4);
            int tag = u16(fields, x), n = u16(fields, x + 2); x += 4;
            require(n <= fields.length - x);
            if (tag == 1) {
                require(!seenWide && packed == U32 && plain == U32 && n == 16 && version >= 45);
                seenWide = true;
                plain = u64(fields, x); packed = u64(fields, x + 8);
            } else require(tag == 0x5455 || tag == 0x7875);
            x += n;
        }
        require(wide == seenWide);
        require(u32(h, 14) == (streaming ? 0 : crc) && plain == (streaming ? 0 : bytes) && packed == plain);
        byte[] descriptor = new byte[0];
        long end = body + bytes;
        if (streaming) {
            // An offset-only ZIP64 central extra says nothing about the two streamed sizes.
            // Accept the complete physical tuple; never reinterpret just the prefix of a wide one.
            long remaining = slot.limit - end;
            boolean sizeFields64 = wide || u32(cd, p + 20) == U32 || u32(cd, p + 24) == U32;
            boolean offsetOnly = slot.centralZip64 && !sizeFields64 && u32(cd, p + 42) == U32;
            boolean sizes64 = sizeFields64 || (offsetOnly && (remaining == 20 || remaining == 24));
            int base = sizes64 ? 20 : 12;
            require(remaining == base || remaining == base + 4);
            descriptor = src.read(end, (int)remaining);
            require(descriptorLength(descriptor, sizes64, crc, bytes) == remaining);
        }

        CRC32 computed = new CRC32();
        for (long pos = body; pos < end;) {
            src.check();
            int n = (int)Math.min(block.length, end - pos);
            src.fill(pos, block, n);
            computed.update(block, 0, n);
            long relative = pos - body;
            if (capture != null && relative < capture.length) {
                int at = (int)relative; // only after the bound: the CRC may cover well over 2 GiB
                System.arraycopy(block, 0, capture, at, Math.min(n, capture.length - at));
            }
            pos += n;
        }
        src.check();
        match(src, slot.offset, h, block);
        match(src, slot.offset + 30, fields, block);
        match(src, end, descriptor, block);
        return new Result(crc, computed.getValue());
    }

    static int descriptorLength(byte[] d, boolean wide, long crc, long bytes) throws IOException {
        int base = wide ? 20 : 12;
        boolean bare = tuple(d, 0, wide, crc, bytes);
        boolean signed = d.length >= base + 4 && u32(d, 0) == 0x08074b50 && tuple(d, 4, wide, crc, bytes);
        require(bare != signed); // the CRC itself can equal the optional signature
        return signed ? base + 4 : base;
    }
    private static boolean tuple(byte[] d, int p, boolean wide, long crc, long bytes) {
        if (d.length - p < (wide ? 20 : 12) || u32(d, p) != crc) return false;
        if (!wide) return u32(d, p + 4) == bytes && u32(d, p + 8) == bytes;
        long low = bytes & U32, high = bytes >>> 32;
        return u32(d, p + 4) == low && u32(d, p + 8) == high
                && u32(d, p + 12) == low && u32(d, p + 16) == high;
    }
}
