package org.zipworkspace.preview;

import android.os.CancellationSignal;
import android.os.OperationCanceledException;
import java.io.IOException;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.zip.CRC32;
import org.junit.Test;
import static org.junit.Assert.*;

public final class StoredMemberTest {
    static class Bytes implements ArchiveIndex.MemberSource {
        final byte[] bytes;
        final CancellationSignal stop = new CancellationSignal();
        Bytes(byte[] bytes) { this.bytes = bytes; }
        public void check() { stop.throwIfCanceled(); }
        public byte[] read(long off, int n) throws IOException {
            check();
            if (off < 0 || n < 0 || off > bytes.length - n) throw new IOException("short read");
            return Arrays.copyOfRange(bytes, (int)off, (int)off + n);
        }
        public void fill(long off, byte[] b, int n) throws IOException {
            check();
            if (off < 0 || n < 0 || off > bytes.length - n) throw new IOException("short read");
            System.arraycopy(bytes, (int)off, b, 0, n);
        }
    }

    // Ordinary, streaming and forced ZIP64 encodings of the same single Store file.
    private static final byte[] NAME = "笔记/part.txt".getBytes(StandardCharsets.UTF_8);
    private static byte[] archive(byte[] body, boolean wide, boolean streaming, boolean signed) {
        CRC32 checksum = new CRC32(); checksum.update(body); int crc = (int)checksum.getValue();
        int flags = 0x800 | (streaming ? 8 : 0), version = wide ? 45 : 10, n = body.length;
        int xl = wide ? 20 : 0, dd = !streaming ? 0 : (wide ? 20 : 12) + (signed ? 4 : 0);
        int cd = 30 + NAME.length + xl + n + dd, clen = 46 + NAME.length + xl;
        ByteBuffer z = ByteBuffer.allocate(cd + clen + 22).order(ByteOrder.LITTLE_ENDIAN);
        z.putInt(0x04034b50).putShort((short)version).putShort((short)flags).putShort((short)0).putInt(0);
        z.putInt(streaming ? 0 : crc);
        int small = wide ? -1 : (streaming ? 0 : n);
        z.putInt(small).putInt(small).putShort((short)NAME.length).putShort((short)xl).put(NAME);
        if (wide) z.putShort((short)1).putShort((short)16).putLong(streaming ? 0 : n).putLong(streaming ? 0 : n);
        z.put(body);
        if (streaming) {
            if (signed) z.putInt(0x08074b50);
            z.putInt(crc);
            if (wide) z.putLong(n).putLong(n); else z.putInt(n).putInt(n);
        }
        z.putInt(0x02014b50).putShort((short)version).putShort((short)version).putShort((short)flags)
                .putShort((short)0).putInt(0).putInt(crc).putInt(wide ? -1 : n).putInt(wide ? -1 : n)
                .putShort((short)NAME.length).putShort((short)xl).putShort((short)0).putShort((short)0)
                .putShort((short)0).putInt(0).putInt(0).put(NAME);
        if (wide) z.putShort((short)1).putShort((short)16).putLong(n).putLong(n);
        z.putInt(0x06054b50).putInt(0).putShort((short)1).putShort((short)1)
                .putInt(clen).putInt(cd).putShort((short)0);
        return z.array();
    }
    private static ArchiveIndex.Item row(ArchiveIndex index) {
        return index.root.children.get("笔记").children.get("part.txt");
    }
    private static StoredMember.Result check(byte[] bytes) throws IOException {
        Bytes src = new Bytes(bytes);
        ArchiveIndex index = ArchiveIndex.read(src, bytes.length);
        return index.checkMember(src, row(index));
    }
    @Test public void crcMatchesTheStandardVectorAndEmptyFileIncludingZIP64Descriptors() throws IOException {
        byte[] vector = "123456789".getBytes(StandardCharsets.US_ASCII);
        for (boolean wide : new boolean[]{false, true}) {
            for (boolean streaming : new boolean[]{false, true}) {
                for (boolean signed : new boolean[]{false, true}) {
                    StoredMember.Result yes = check(archive(vector, wide, streaming, signed));
                    assertTrue(yes.matches()); assertEquals(0xcbf43926L, yes.actual);
                    StoredMember.Result empty = check(archive(new byte[0], wide, streaming, signed));
                    assertTrue(empty.matches()); assertEquals(0, empty.actual);
                }
            }
        }
    }
    @Test public void damagedBodyReturnsMismatchButUnreconciledHeadersGiveNoCRC() throws IOException {
        byte[] body = {1, 2, 3, 4}, good = archive(body, false, true, true);
        byte[] damage = good.clone(); damage[30 + NAME.length + 1] ^= 2;
        assertFalse(check(damage).matches());
        // Different local path, flags, method, or descriptor CRC is not a completed content comparison.
        for (int off : new int[]{30 + NAME.length - 1, 6, 8, 30 + NAME.length + body.length + 4}) {
            byte[] different = good.clone(); different[off] ^= 1;
            assertThrows(IOException.class, () -> check(different));
        }
        byte[] wide = archive(body, true, false, false);
        wide[30 + NAME.length + 4] ^= 1; // local ZIP64 uncompressed length only
        assertThrows(IOException.class, () -> check(wide));
    }
    private static byte[] offsetOnly(byte[] body, boolean signed, boolean wideTuple, long origin) {
        byte[] old = archive(body, false, true, signed);
        int start = 30 + NAME.length + body.length, oldCD = start + (signed ? 16 : 12);
        int extraSize = wideTuple ? 8 : 0, cd = oldCD + extraSize, clen = 46 + NAME.length + 12;
        boolean large = origin + cd > ArchiveIndex.U32;
        ByteBuffer z = ByteBuffer.allocate(old.length + 12 + extraSize + (large ? 76 : 0)).order(ByteOrder.LITTLE_ENDIAN);
        z.put(old, 0, start + (signed ? 8 : 4)); // optional signature and CRC
        if (wideTuple) {
            z.putLong(body.length).putLong(body.length);
        } else {
            z.putShort(4, (short)20); // ordinary local v2.0 for the small streamed member
            z.putInt(body.length).putInt(body.length);
        }
        z.put(old, oldCD, 46 + NAME.length);
        z.putShort(cd + 6, (short)45).putShort(cd + 30, (short)12).putInt(cd + 42, -1);
        z.putShort((short)1).putShort((short)8).putLong(origin);
        if (large) {
            long zip64 = origin + z.position();
            z.putInt(0x06064b50).putLong(44).putShort((short)45).putShort((short)45).putLong(0);
            z.putLong(1).putLong(1).putLong(clen).putLong(origin + cd);
            z.putInt(0x07064b50).putInt(0).putLong(zip64).putInt(1);
        }
        z.putInt(0x06054b50).putInt(0).putShort((short)1).putShort((short)1)
                .putInt(clen).putInt(large ? -1 : (int)(origin + cd)).putShort((short)0);
        return z.array();
    }
    @Test public void offsetOnlyCentralZIP64AcceptsTheWholeWideDescriptorAndRequiresCentralVersion45() throws IOException {
        for (byte[] body : new byte[][]{new byte[0], new byte[]{1, 2, 3}}) {
            for (boolean signed : new boolean[]{false, true}) {
                byte[] b = offsetOnly(body, signed, true, 0);
                assertTrue(check(b).matches());
                int cd = ByteBuffer.wrap(b).order(ByteOrder.LITTLE_ENDIAN).getInt(b.length - 6);
                byte[] bad = b.clone(); bad[cd - 1] ^= 1; // upper bytes of wide uncompressed size, even for zero-byte body
                assertThrows(IOException.class, () -> check(bad));
                byte[] low = b.clone(); low[cd + 6] = 10;
                assertThrows(IOException.class, () -> ArchiveIndexTest.read(low));
            }
        }
    }
    @Test public void narrowDescriptorForSmallStreamedMemberBeyondFourGiB() throws IOException {
        // CPython can write a signed narrow descriptor before the central directory needs ZIP64 offsets.
        final long origin = (1L << 32) + 37;
        for (byte[] body : new byte[][]{new byte[0], new byte[]{1, 2, 3}}) {
            for (boolean signed : new boolean[]{false, true}) {
                final byte[] tail = offsetOnly(body, signed, false, origin);
                final long eof = origin + tail.length;
                Bytes sparse = new Bytes(tail) {
                    @Override public byte[] read(long off, int n) throws IOException {
                        if (n < 0 || n > 65557) throw new IOException("unbounded read");
                        byte[] out = new byte[n]; fill(off, out, n); return out;
                    }
                    @Override public void fill(long off, byte[] out, int n) throws IOException {
                        check();
                        if (n < 0 || n > 65557 || off < eof - 65557 || off > eof - n) throw new IOException("not the tail");
                        int gap = (int)Math.min(n, Math.max(0L, origin - off));
                        Arrays.fill(out, 0, gap, (byte)0);
                        if (gap != n) System.arraycopy(tail, (int)(off + gap - origin), out, gap, n - gap);
                    }
                };
                ArchiveIndex index = ArchiveIndex.read(sparse, eof);
                assertTrue(index.checkMember(sparse, row(index)).matches());
                tail[30 + NAME.length + body.length + (signed ? 16 : 12) - 1] ^= 1;
                assertThrows(IOException.class, () -> index.checkMember(sparse, row(index))); // malformed, not CRC mismatch
            }
        }
    }
    private static StoredMember.Result preview(byte[] zip) throws IOException {
        Bytes src = new Bytes(zip);
        ArchiveIndex index = ArchiveIndex.read(src, zip.length);
        return index.checkMember(src, row(index), true);
    }
    @Test public void previewUsesOneCrcStreamAndRendersLiteralUnicodeWithoutActivatingMarkup() throws IOException {
        String sample = "读我\t𠮷 / é / e\u0301\r\n<b>https://example.invalid</b>\n";
        byte[] body = sample.getBytes(StandardCharsets.UTF_8), zip = archive(body, true, true, true);
        final int[] payloadReads = {0};
        Bytes src = new Bytes(zip) {
            @Override public void fill(long off, byte[] buf, int n) throws IOException {
                if (off == 30 + NAME.length + 20) { payloadReads[0]++; assertEquals(body.length, n); }
                super.fill(off, buf, n);
            }
        };
        ArchiveIndex index = ArchiveIndex.read(src, zip.length);
        StoredMember.Result r = index.checkMember(src, row(index), true);
        assertTrue(r.matches()); assertEquals(sample.replace("\r\n", "\n"), r.text);
        assertEquals(1, payloadReads[0]);
        assertNull(index.checkMember(src, row(index)).text); // CRC-only does not return a preview
    }
    @Test public void previewByteBudgetIncludesZeroAndExactly32KiBRejectsOneMoreBeforeIO() throws IOException {
        assertEquals("", preview(archive(new byte[0], false, false, false)).text);
        byte[] body = new byte[PlainText.LIMIT]; Arrays.fill(body, (byte)'a');
        assertEquals(PlainText.LIMIT, preview(archive(body, false, false, false)).text.length());
        byte[] large = archive(Arrays.copyOf(body, PlainText.LIMIT + 1), false, false, false);
        ArchiveIndex index = ArchiveIndex.read(new Bytes(large), large.length);
        Bytes noRead = new Bytes(large) {
            @Override public byte[] read(long at, int n) { throw new AssertionError("oversize preview opened input"); }
            @Override public void fill(long at, byte[] buf, int n) { throw new AssertionError("oversize preview read bytes"); }
        };
        assertThrows(IOException.class, () -> index.checkMember(noRead, row(index), true));
        assertTrue(check(large).matches()); // original streaming CRC keeps its independent limit
    }
    @Test public void crcAndTextEligibilityHaveDistinctOutcomes() throws IOException {
        for (String suffix : new String[]{"\0", "\r", "\u001b", "\u0085", "\u202e", "\u2066", "\u034f",
                "\ufe0f", "\ufeff", "\u2028", "\ue000"}) {
            byte[] zip = archive(("start" + suffix + "!").getBytes(StandardCharsets.UTF_8), false, true, false);
            StoredMember.Result r = preview(zip); assertTrue(r.matches()); assertNull(r.text);
        }
        for (byte[] bad : new byte[][]{{(byte)0xc0, (byte)0xaf}, {(byte)0xe2, (byte)0x82},
                {(byte)0xed, (byte)0xa0, (byte)0x80}}) {
            StoredMember.Result r = preview(archive(bad, false, false, false));
            assertTrue(r.matches()); assertNull(r.text);
        }
        byte[] damaged = archive(new byte[]{'a', 'b'}, false, true, true);
        damaged[30 + NAME.length] = 'c';
        StoredMember.Result mismatch = preview(damaged);
        assertFalse(mismatch.matches()); assertNull(mismatch.text);
    }
    @Test public void cancelledReplacedOrChangedObservationNeverReturnsDisplayText() throws IOException {
        byte[] zip = archive(new byte[]{'o', 'k'}, false, true, false);
        ArchiveIndex a = ArchiveIndex.read(new Bytes(zip), zip.length), b = ArchiveIndex.read(new Bytes(zip), zip.length);
        assertThrows(IOException.class, () -> b.checkMember(new Bytes(zip), row(a), true));
        for (boolean cancel : new boolean[]{false, true}) {
            Bytes src = new Bytes(zip.clone()) {
                @Override public void fill(long at, byte[] buf, int n) throws IOException {
                    super.fill(at, buf, n);
                    if (at == 30 + NAME.length) {
                        if (cancel) stop.cancel(); else bytes[bytes.length - 22 + 12] ^= 1;
                    }
                }
            };
            if (cancel) assertThrows(OperationCanceledException.class, () -> a.checkMember(src, row(a), true));
            else assertThrows(IOException.class, () -> a.checkMember(src, row(a), true));
        }
    }
    @Test public void optionalSignatureIsNotChosenFromFirstWordAlone() throws IOException {
        long sig = 0x08074b50;
        ByteBuffer z = ByteBuffer.allocate(16).order(ByteOrder.LITTLE_ENDIAN);
        z.putInt((int)sig).putInt(7).putInt(7).putInt(0);
        assertEquals(12, StoredMember.descriptorLength(z.array(), false, sig, 7));
        z.clear(); z.putInt((int)sig).putInt((int)sig).putInt(7).putInt(7);
        assertEquals(16, StoredMember.descriptorLength(z.array(), false, sig, 7));
        z.clear(); for (int n = 0; n < 4; n++) z.putInt((int)sig);
        assertThrows(IOException.class, () -> StoredMember.descriptorLength(z.array(), false, sig, sig));
    }
    @Test public void rowMustBelongToCurrentIndexAndCannotBeAFolder() throws IOException {
        byte[] b = archive(new byte[0], false, false, false); Bytes src = new Bytes(b);
        ArchiveIndex one = ArchiveIndex.read(src, b.length), two = ArchiveIndex.read(src, b.length);
        Bytes noRead = new Bytes(b) {
            @Override public byte[] read(long o, int n) { throw new AssertionError("unexpected metadata read"); }
            @Override public void fill(long o, byte[] tmp, int n) { throw new AssertionError("unexpected stream"); }
        };
        assertThrows(IOException.class, () -> two.checkMember(noRead, row(one)));
        assertThrows(IOException.class, () -> one.checkMember(noRead, one.root.children.get("笔记")));
    }
    @Test public void localExtraCannotPushTheClaimedBodyIntoAnotherListedHeader() throws IOException {
        byte[] b = ArchiveIndexTest.zip("first", "second");
        Bytes src = new Bytes(b); ArchiveIndex index = ArchiveIndex.read(src, b.length);
        ByteBuffer.wrap(b).order(ByteOrder.LITTLE_ENDIAN).putShort(28, (short)50);
        assertThrows(IOException.class, () -> index.checkMember(src, index.root.children.get("first")));
    }
    @Test public void shortReadOrCancellationDoesNotReturnAPartialCRC() throws IOException {
        byte[] b = archive(new byte[150000], false, false, false);
        int first = 30 + NAME.length;
        for (boolean cancel : new boolean[]{true, false}) {
            Bytes src = new Bytes(b) {
                byte[] previous;
                @Override public void fill(long off, byte[] target, int n) throws IOException {
                    if (off >= first && off < first + 150000) {
                        if (previous != null) {
                            assertSame(previous, target);
                            if (cancel) { stop.cancel(); check(); }
                            throw new IOException("truncated read");
                        }
                        previous = target; assertEquals(65536, n);
                    }
                    super.fill(off, target, n);
                }
            };
            ArchiveIndex index = ArchiveIndex.read(src, b.length);
            if (cancel) assertThrows(OperationCanceledException.class, () -> index.checkMember(src, row(index)));
            else assertThrows(IOException.class, () -> index.checkMember(src, row(index)));
        }
    }
    @Test public void changedDirectoryEndingOrLocalHeaderAfterBodyInvalidatesEvenAMatchingCRC() throws IOException {
        for (int which = 0; which < 3; which++) {
            byte[] b = archive(new byte[25], false, false, false);
            int body = 30 + NAME.length;
            int target = new int[]{body + 25 + 16, b.length - 2, 30 + NAME.length - 1}[which];
            Bytes src = new Bytes(b) {
                @Override public void fill(long off, byte[] tmp, int n) throws IOException {
                    super.fill(off, tmp, n); if (off == body) bytes[target] ^= 1;
                }
            };
            ArchiveIndex index = ArchiveIndex.read(src, b.length);
            assertThrows(IOException.class, () -> index.checkMember(src, row(index)));
        }
    }
}
