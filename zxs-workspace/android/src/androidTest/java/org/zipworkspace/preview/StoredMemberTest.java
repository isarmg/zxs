package org.zipworkspace.preview;

import android.os.CancellationSignal;
import android.os.OperationCanceledException;
import java.io.IOException;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
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
        assertTrue(r.matches()); assertEquals(sample.replace("\r\n", "\n"), joined(r.text));
        assertEquals(1, payloadReads[0]);
        assertNull(index.checkMember(src, row(index)).text); // CRC-only does not return a preview
    }
    private static String joined(PlainText.Preview text) {
        assertNotNull(text);
        StringBuilder all = new StringBuilder();
        for (int i = 0; i < text.pages(); i++) {
            String page = text.page(i);
            assertTrue(page.length() <= PlainText.PAGE_UNITS);
            assertTrue(page.chars().filter(c -> c == '\n').count() <= PlainText.PAGE_NEWLINES);
            if (!page.isEmpty()) {
                assertFalse(Character.isLowSurrogate(page.charAt(0)));
                assertFalse(Character.isHighSurrogate(page.charAt(page.length() - 1)));
            }
            all.append(page);
        }
        return all.toString();
    }
    @Test public void zeroAndPastTheFormer32KiBLimitRemainCompleteAndBounded() throws IOException {
        PlainText.Preview empty = preview(archive(new byte[0], false, false, false)).text;
        assertTrue(empty.complete()); assertEquals(1, empty.pages()); assertEquals("", joined(empty));
        for (int length : new int[]{32 * 1024 + 1, PlainText.LIMIT}) {
            byte[] body = new byte[length]; Arrays.fill(body, (byte)'a');
            PlainText.Preview text = preview(archive(body, false, false, false)).text;
            assertTrue(text.complete()); assertEquals(length, text.displayedBytes);
            assertTrue(text.pages() > 1); assertEquals(length, joined(text).length());
        }
    }
    @Test public void pagesPreserveSurrogatePairsDenseLinesAndTheEndOfFullText() throws IOException {
        String sample = "a".repeat(PlainText.PAGE_UNITS - 1) + "𠮷\r\n"
                + "row\r\n".repeat(PlainText.PAGE_NEWLINES + 5) + "最后一页-marker";
        PlainText.Preview text = preview(archive(sample.getBytes(StandardCharsets.UTF_8), false, false, false)).text;
        assertTrue(text.complete()); assertEquals(PlainText.PAGE_UNITS - 1, text.page(0).length());
        assertTrue(text.page(1).startsWith("𠮷"));
        assertEquals(sample.replace("\r\n", "\n"), joined(text));
        assertTrue(text.page(text.pages() - 1).endsWith("最后一页-marker"));
    }
    @Test public void largePreviewReadsExactlyOneEntireCrcStreamButRetainsOnlyThePrefix() throws IOException {
        byte[] body = new byte[512 * 1024]; Arrays.fill(body, (byte)'a');
        body[PlainText.LIMIT] = (byte)0x80; // cap ended after complete ASCII; this is unexamined text
        body[body.length - 1] = 'Z';
        byte[] zip = archive(body, false, false, false);
        int start = 30 + NAME.length;
        final long[] cursor = {start};
        Bytes input = new Bytes(zip) {
            @Override public void fill(long off, byte[] buf, int n) throws IOException {
                if (off >= start && off < start + body.length) { assertEquals(cursor[0], off); cursor[0] += n; }
                super.fill(off, buf, n);
            }
        };
        ArchiveIndex index = ArchiveIndex.read(input, zip.length);
        StoredMember.Result r = index.checkMember(input, row(index), true);
        assertTrue(r.matches()); assertFalse(r.text.complete());
        assertEquals((long)body.length, r.text.totalBytes);
        assertEquals(PlainText.LIMIT, r.text.displayedBytes);
        assertEquals("a".repeat(PlainText.LIMIT), joined(r.text));
        assertEquals((long)start + body.length, cursor[0]);
        assertTrue(check(zip).matches());
        zip[start + body.length - 1] ^= 1; // CRC must include bytes never kept in memory.
        StoredMember.Result brokenTail = preview(zip);
        assertFalse(brokenTail.matches()); assertNull(brokenTail.text);
    }
    @Test public void previewCutOnlyOmitsAConfirmedFullCrossingScalarOrCrlf() throws IOException {
        int cap = PlainText.LIMIT;
        byte[] body = new byte[cap + 5];
        Arrays.fill(body, (byte)'a');
        byte[] four = "𠮷".getBytes(StandardCharsets.UTF_8);
        System.arraycopy(four, 0, body, cap - 1, four.length);
        PlainText.Preview prefix = preview(archive(body, false, false, false)).text;
        assertFalse(prefix.complete()); assertEquals(cap - 1, prefix.displayedBytes);
        assertEquals("a".repeat(cap - 1), joined(prefix));

        body[cap] = 'b'; // malformed scalar started INSIDE the budget, not ignorable trailing data
        StoredMember.Result invalid = preview(archive(body, false, false, false));
        assertTrue(invalid.matches()); assertNull(invalid.text);

        byte[] direction = "\u202e".getBytes(StandardCharsets.UTF_8);
        System.arraycopy(direction, 0, body, cap - 1, direction.length);
        StoredMember.Result forbidden = preview(archive(body, false, false, false));
        assertTrue(forbidden.matches()); assertNull(forbidden.text);

        Arrays.fill(body, (byte)'a'); body[cap - 1] = '\r'; body[cap] = '\n';
        prefix = preview(archive(body, false, false, false)).text;
        assertEquals(cap - 1, prefix.displayedBytes); assertEquals("a".repeat(cap - 1), joined(prefix));
        body[cap] = 'b'; // a real unmatched CR, not a CRLF that may be omitted
        StoredMember.Result isolated = preview(archive(body, false, false, false));
        assertTrue(isolated.matches()); assertNull(isolated.text);
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
    private static final class WitnessReads extends Bytes {
        final int body, length;
        final List<Long> starts = new ArrayList<>();
        final List<Integer> lengths = new ArrayList<>();
        WitnessReads(byte[] original, int body, int length) { super(original); this.body = body; this.length = length; }
        @Override public void fill(long off, byte[] buf, int n) throws IOException {
            if (off >= body && off < (long) body + length) {
                assertTrue(off + n <= (long) body + length);
                starts.add(off - body); lengths.add(n);
            }
            super.fill(off, buf, n);
        }
        void resetReads() { starts.clear(); lengths.clear(); }
    }

    @Test public void indexedPagesReadExactlyTheWitnessedBlocksAndKeepCrossingScalarsAndPairs() throws IOException {
        int block = IndexedText.BLOCK;
        byte[] body = new byte[3 * block + 15];
        Arrays.fill(body, (byte)'a');
        byte[] crlf = "\r\nNEXT".getBytes(StandardCharsets.UTF_8);
        System.arraycopy(crlf, 0, body, block - 1, crlf.length);
        byte[] scalar = "𠮷NEXT".getBytes(StandardCharsets.UTF_8);
        System.arraycopy(scalar, 0, body, 2 * block - 3, scalar.length);
        byte[] end = "LAST".getBytes(StandardCharsets.UTF_8);
        System.arraycopy(end, 0, body, body.length - end.length, end.length);
        byte[] zip = archive(body, true, true, true);
        WitnessReads src = new WitnessReads(zip, 30 + NAME.length + 20, body.length);
        ArchiveIndex index = ArchiveIndex.read(src, zip.length);
        ArchiveIndex.Item file = row(index);
        StoredMember.Result result = index.checkMember(src, file, StoredMember.Mode.INDEXED);
        assertTrue(result.matches()); assertNull(result.text); assertNotNull(result.indexed);
        assertEquals(List.of(0L, (long)block, 2L * block, 3L * block), src.starts);
        assertEquals(List.of(block, block, block, 15), src.lengths); // same one pass as the CRC

        src.resetReads();
        String far = index.textPage(src, file, result.indexed, result.indexed.pages() - 1);
        assertTrue(far.endsWith("LAST"));
        assertEquals(List.of(3L * block), src.starts); assertEquals(List.of(15), src.lengths);

        src.resetReads();
        String second = index.textPage(src, file, result.indexed, 128);
        assertTrue(second.startsWith("\nNEXT"));
        assertEquals(PlainText.PAGE_UNITS, second.length());
        assertEquals(List.of(0L, (long)block), src.starts); assertEquals(List.of(block, block), src.lengths);

        src.resetReads();
        String third = index.textPage(src, file, result.indexed, 256);
        assertTrue(third.startsWith("𠮷NEXT"));
        assertEquals(PlainText.PAGE_UNITS, third.length());
        assertEquals(List.of((long)block, 2L * block), src.starts);
        assertThrows(IOException.class, () -> index.textPage(src, file, result.indexed, -1));
        assertThrows(IOException.class, () -> index.textPage(src, file, result.indexed, result.indexed.pages()));
    }

    @Test public void indexedTextRequiresWellFormedWholeBodyAndCrcEvenBeyondThePrefix() throws IOException {
        for (byte[] ending : new byte[][]{ {(byte)0xe2, (byte)0x82}, {'\r'},
                "\u202e".getBytes(StandardCharsets.UTF_8), {(byte)0xc0, (byte)0xaf} }) {
            byte[] body = new byte[IndexedText.BLOCK + ending.length];
            Arrays.fill(body, (byte)'a');
            System.arraycopy(ending, 0, body, IndexedText.BLOCK, ending.length);
            byte[] zip = archive(body, false, false, false);
            Bytes src = new Bytes(zip);
            ArchiveIndex index = ArchiveIndex.read(src, zip.length);
            StoredMember.Result rejected = index.checkMember(src, row(index), StoredMember.Mode.INDEXED);
            assertTrue(rejected.matches()); assertNull(rejected.indexed); assertNull(rejected.text);
            assertNotNull(index.checkMember(src, row(index), true).text); // explicitly chosen prefix remains distinct
        }
        byte[] body = new byte[IndexedText.BLOCK + 7]; Arrays.fill(body, (byte)'\n');
        byte[] zip = archive(body, false, true, false);
        Bytes src = new Bytes(zip);
        ArchiveIndex index = ArchiveIndex.read(src, zip.length);
        ArchiveIndex.Item item = row(index);
        StoredMember.Result good = index.checkMember(src, item, StoredMember.Mode.INDEXED);
        assertEquals(IndexedText.BLOCK / PlainText.PAGE_NEWLINES + 1, good.indexed.pages());
        assertEquals("\n".repeat(PlainText.PAGE_NEWLINES), index.textPage(src, item, good.indexed, 4000));
        assertEquals("\n".repeat(7), index.textPage(src, item, good.indexed, good.indexed.pages() - 1));

        zip[30 + NAME.length + body.length - 1] = 'Z'; // valid UTF-8 outside the prefix, but wrong CRC
        StoredMember.Result bad = index.checkMember(src, item, StoredMember.Mode.INDEXED);
        assertFalse(bad.matches()); assertNull(bad.indexed); assertNull(bad.text);
    }

    @Test public void indexedPageRejectsOtherIndexAlteredGeometryAndChangedUnshownBytesOfItsBlock() throws IOException {
        int block = IndexedText.BLOCK, start = 30 + NAME.length;
        byte[] body = new byte[2 * block + 17]; Arrays.fill(body, (byte)'a');
        body[block - 1] = '\r'; body[block] = '\n';
        byte[] zip = archive(body, false, true, true);
        Bytes src = new Bytes(zip);
        ArchiveIndex index = ArchiveIndex.read(src, zip.length), other = ArchiveIndex.read(src, zip.length);
        ArchiveIndex.Item item = row(index);
        ArchiveIndex.TextIndex proof = index.checkMember(src, item, StoredMember.Mode.INDEXED).indexed;
        assertNotNull(proof);
        assertThrows(IOException.class, () -> other.textPage(src, row(other), proof, 128));

        for (int offset : new int[]{start - 1, start + body.length + 15, zip.length - 22 - 1, zip.length - 1,
                start, start + 2 * block - 1}) {
            byte old = zip[offset]; zip[offset] ^= 1;
            assertThrows(IOException.class, () -> index.textPage(src, item, proof, 128));
            zip[offset] = old;
        }
        assertTrue(index.textPage(src, item, proof, 128).startsWith("\n"));

        Bytes late = new Bytes(zip.clone()) {
            @Override public void fill(long off, byte[] output, int n) throws IOException {
                super.fill(off, output, n);
                if (off == start + block) bytes[bytes.length - 1] = 1; // changed after the selected block
            }
        };
        assertThrows(IOException.class, () -> index.textPage(late, item, proof, 128));
        Bytes stopped = new Bytes(zip.clone()) {
            @Override public void fill(long off, byte[] output, int n) throws IOException {
                super.fill(off, output, n);
                if (off == start + block) stop.cancel();
            }
        };
        assertThrows(OperationCanceledException.class, () -> index.textPage(stopped, item, proof, 128));
        assertThrows(OperationCanceledException.class,
                () -> index.checkMember(stopped, item, StoredMember.Mode.INDEXED));
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
            ArrayList<Integer> processed = new ArrayList<>();
            Bytes src = new Bytes(b) {
                byte[] previous;
                @Override public void bodyProcessed(int n) { processed.add(n); }
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
            assertEquals(List.of(65536), processed); // short or canceled block doesn't advance
        }
    }
    @Test public void bodyCountIsNotIntegrityAndAllModesUseTheSameCounter() throws IOException {
        byte[] text = new byte[135000]; Arrays.fill(text, (byte)'x');
        byte[] b = archive(text, false, true, true);
        b[30 + NAME.length + text.length - 1] = 'y'; // valid UTF-8 and length, wrong CRC
        for (StoredMember.Mode mode : StoredMember.Mode.values()) {
            ArrayList<Integer> processed = new ArrayList<>();
            Bytes src = new Bytes(b) {
                @Override public void bodyProcessed(int n) { processed.add(n); }
            };
            ArchiveIndex index = ArchiveIndex.read(src, b.length);
            assertTrue(processed.isEmpty()); // listing, even if its tail window overlaps body bytes
            StoredMember.Result result = index.checkMember(src, row(index), mode);
            assertFalse(result.matches());
            assertNull(result.text); assertNull(result.indexed);
            assertEquals(mode == StoredMember.Mode.INDEXED ? List.of(text.length) : List.of(65536, 65536, 3928), processed);
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
