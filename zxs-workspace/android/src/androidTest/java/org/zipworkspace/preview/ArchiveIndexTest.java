package org.zipworkspace.preview;

import android.content.Intent;
import android.net.Uri;
import android.os.CancellationSignal;
import android.os.OperationCanceledException;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Locale;
import java.util.zip.CRC32;
import java.util.zip.ZipEntry;
import java.util.zip.ZipOutputStream;
import org.junit.Test;
import static org.junit.Assert.*;

public final class ArchiveIndexTest {
    static byte[] zip(String... names) throws IOException {
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        try (ZipOutputStream z = new ZipOutputStream(out)) {
            for (String name : names) {
                byte[] bytes = name.endsWith("/") ? new byte[0] : new byte[]{1, 2, 3};
                CRC32 crc = new CRC32(); crc.update(bytes);
                ZipEntry item = new ZipEntry(name); item.setMethod(ZipEntry.STORED);
                item.setSize(bytes.length); item.setCompressedSize(bytes.length); item.setCrc(crc.getValue());
                z.putNextEntry(item); z.write(bytes); z.closeEntry();
            }
        }
        return out.toByteArray();
    }
    static ArchiveIndex read(byte[] b) throws IOException {
        return ArchiveIndex.read(new ArchiveIndex.Source() {
            public byte[] read(long off, int n) { return Arrays.copyOfRange(b, (int)off, (int)off + n); }
            public void check() {}
        }, b.length);
    }
    private static void rejects(byte[] b) { assertThrows(IOException.class, () -> read(b)); }
    private static byte[] marked(byte[] original, String tree, String generation, String parent) {
        byte[] comment = ("zip-workspace/1\n{\"branch\":\"" + "b".repeat(32) + "\",\"generation\":" + generation
                + ",\"operation\":\"" + "a".repeat(32) + "\",\"parent\":" + parent + ",\"repo\":\"" + "c".repeat(32)
                + "\",\"tree\":\"" + tree + "\"}").getBytes(StandardCharsets.US_ASCII);
        byte[] b = Arrays.copyOf(original, original.length + comment.length);
        ByteBuffer.wrap(b).order(ByteOrder.LITTLE_ENDIAN).putShort(original.length - 2, (short)comment.length);
        System.arraycopy(comment, 0, b, original.length, comment.length);
        return b;
    }
    private static ArchiveIndex.SnapshotMarker snapshot(byte[] b) throws IOException {
        return read(b).verifySnapshot(new StoredMemberTest.Bytes(b));
    }

    @Test public void rawZipDigestIncludesArchiveMetadataAndCanStopOnOneBoundedRead() throws Exception {
        // A real ordinary Store ZIP; the archived body and the opaque comment both cross the read budget together.
        byte[] plain;
        byte[] payload = new byte[65_541]; Arrays.fill(payload, (byte)0x1a);
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        try (ZipOutputStream z = new ZipOutputStream(bytes)) {
            CRC32 crc = new CRC32(); crc.update(payload);
            ZipEntry e = new ZipEntry("unchanged"); e.setMethod(ZipEntry.STORED);
            e.setSize(payload.length); e.setCompressedSize(payload.length); e.setCrc(crc.getValue());
            z.putNextEntry(e); z.write(payload); z.closeEntry(); z.setComment("visible archive comment");
        }
        plain = bytes.toByteArray();
        ArrayList<Long> offsets = new ArrayList<>(), progress = new ArrayList<>();
        String actual = ArchiveIndex.rawDigest(new StoredMemberTest.Bytes(plain) {
            @Override public byte[] read(long off, int n) { throw new AssertionError("no ZIP metadata pass"); }
            @Override public void fill(long off, byte[] dst, int n) throws IOException {
                offsets.add(off); assertEquals(65_536, dst.length); super.fill(off, dst, n);
            }
        }, plain.length, progress::add);
        StringBuilder expected = new StringBuilder();
        for (byte b : MessageDigest.getInstance("SHA-256").digest(plain)) expected.append(String.format(Locale.ROOT, "%02x", b & 255));
        assertEquals(expected.toString(), actual);
        assertEquals(List.of(0L, 65_536L), offsets);
        assertEquals(List.of(65_536L, plain.length - 65_536L), progress);
        byte[] changed = plain.clone(); changed[changed.length - 1] ^= 1; // just the comment
        assertNotEquals(actual, ArchiveIndex.rawDigest(new StoredMemberTest.Bytes(changed), changed.length, null));
        assertEquals(read(plain).bodyBytes(), read(changed).bodyBytes());
        assertThrows(IOException.class, () -> ArchiveIndex.rawDigest(new StoredMemberTest.Bytes(plain), plain.length + 1, null));
        assertThrows(OperationCanceledException.class, () -> ArchiveIndex.rawDigest(new StoredMemberTest.Bytes(plain) {
            @Override public void fill(long off, byte[] dst, int n) throws IOException {
                assertEquals(0L, off); super.fill(off, dst, n); stop.cancel();
            }
        }, plain.length, null));
    }
    @Test public void explicitWholeSnapshotUsesCanonicalFullPathOrderAndReadsOnlyOneBodyPass() throws IOException {
        // A private-use BMP name precedes the supplementary scalar in UTF-8; Java UTF-16 compareTo reverses those two.
        byte[] base = zip("a/图", "空/", "a-.txt", "\uE010", "\uD840\uDC00");
        byte[] b = marked(base, "963268783ba438cc7cd2087eb524fb8ccb60d9dfddfdbc92508abdfe7f27920d", "1", "null");
        ArchiveIndex index = read(b);
        int[] checks = {0};
        ArrayList<Integer> processed = new ArrayList<>();
        ByteBuffer physical = ByteBuffer.wrap(base).order(ByteOrder.LITTLE_ENDIAN);
        int central = physical.getInt(base.length - 6), namedDirectory = -1;
        ArrayList<Long> bodies = new ArrayList<>();
        for (int p = central; p < base.length - 22; p += recordSize(physical, p)) {
            int local = physical.getInt(p + 42);
            if (physical.getInt(p + 24) == 0) namedDirectory = local;
            else bodies.add((long)local + 30 + (physical.getShort(local + 26) & 65535)
                    + (physical.getShort(local + 28) & 65535));
        }
        ArchiveIndex.SnapshotMarker result = index.verifySnapshot(new StoredMemberTest.Bytes(b) {
            @Override public void bodyProcessed(int n) { processed.add(n); }
            @Override public void fill(long off, byte[] dest, int n) throws IOException {
                super.fill(off, dest, n);
                if (bodies.contains(off)) { assertEquals(3, n); checks[0]++; }
            }
        });
        assertEquals("a".repeat(32), result.operation);
        assertEquals(1L, result.generation);
        assertNull(result.parent);
        assertEquals(12L, index.bodyBytes());
        assertEquals(6, index.entryCount());
        assertEquals(4, checks[0]); // one fill at the actual body offset per ordinary file
        assertEquals(List.of(3, 3, 3, 3), processed); // neither empty directories nor metadata
        byte[] badDirectory = b.clone();
        assertTrue(namedDirectory >= 0);
        badDirectory[namedDirectory + 8] = 8;  // still lists the directory; local method is not Store
        assertTrue(read(badDirectory).root.children.get("空").children.isEmpty());
        assertThrows(IOException.class, () -> snapshot(badDirectory));
        assertEquals(1L, snapshot(marked(zip(),
                "ead0d74a9b1110a6e8aa7b227316376cc0883c3fb6b8404f31b0b2719d015657", "1", "null")).generation);
    }
    @Test public void crcMatchedMemberIsNotACompleteTreeAndFooterMustStillBeOriginal() throws IOException {
        String tree = "55b10481df5b6450c7095890b16f73507981d82b55923c290d705c3bff429441";
        byte[] x = zip("x"), b = marked(x, tree, "1", "null");
        ArchiveIndex index = read(b);
        assertEquals(tree, snapshot(b).tree);
        byte[] changed = b.clone();
        ByteBuffer in = ByteBuffer.wrap(changed).order(ByteOrder.LITTLE_ENDIAN);
        int body = 30 + (in.getShort(26) & 65535) + (in.getShort(28) & 65535), central = in.getInt(x.length - 6);
        changed[body] = 9;
        CRC32 crc = new CRC32(); crc.update(changed, body, 3);
        in.putInt(14, (int)crc.getValue()); in.putInt(central + 16, (int)crc.getValue());
        ArchiveIndex newIndex = read(changed);
        assertTrue(newIndex.checkMember(new StoredMemberTest.Bytes(changed), newIndex.root.children.get("x")).matches());
        assertThrows(IOException.class, () -> snapshot(changed));
        assertThrows(IOException.class, () -> snapshot(x));
        assertThrows(IOException.class, () -> snapshot(marked(x, tree, "9223372036854775808", "\""+ "d".repeat(32) + "\"")));
        assertThrows(IOException.class, () -> snapshot(marked(x, tree, "2", "null")));
        byte[] alteredFooter = b.clone(); alteredFooter[alteredFooter.length - 3] ^= 1;
        assertThrows(IOException.class, () -> index.verifySnapshot(new StoredMemberTest.Bytes(alteredFooter) {
            @Override public void fill(long off, byte[] dest, int n) throws IOException {
                assertTrue("must reject changed footer before reading body", off >= central);
                super.fill(off, dest, n);
            }
        }));
        assertThrows(OperationCanceledException.class, () -> index.verifySnapshot(new StoredMemberTest.Bytes(b) {
            @Override public void fill(long off, byte[] dest, int n) throws IOException {
                super.fill(off, dest, n);
                if (off == body) stop.cancel();
            }
        }));
    }
    @Test public void actualStoredMembersEmptyDirectoryAndImplicitParents() throws IOException {
        ArchiveIndex.Item root = read(zip("设计/区域/part.txt", "空/", "设计/")).root;
        assertEquals(2, root.children.size());
        assertEquals(3L, root.children.get("设计").children.get("区域").children.get("part.txt").bytes);
        assertTrue(root.children.get("空").children.isEmpty());
        assertTrue(read(zip()).root.children.isEmpty());
    }
    @Test public void nameSearchLocatesSameSpellingInDifferentParentsEvenBeyondTheFirstPage() throws IOException {
        ArrayList<String> names = new ArrayList<>();
        for (int i = 100; i < 205; i++) names.add("A/item-" + i);
        names.addAll(List.of("A/readme[1].txt", "B/readme[1].txt", "C/README[1].TXT",
                "Empty/", "Implicit/SubName/file"));
        byte[] packed = zip(names.toArray(new String[0]));
        ArchiveIndex index = read(packed);
        List<ArchiveIndex.Match> hits = index.findNames("readme[1]", () -> {});
        assertEquals(2, hits.size());  // Literal brackets, case-sensitive, not a regex or a path lookup.
        for (int i = 0; i < 2; i++) {
            ArchiveIndex.Match hit = hits.get(i);
            ArchiveIndex.Item parent = index.root.children.get(i == 0 ? "a" : "b");
            assertSame(parent, hit.item.parent);
            assertSame(parent.children.get("readme[1].txt"), hit.item);
            assertSame(new ArrayList<>(parent.children.values()).get(hit.ordinal), hit.item);
        }
        assertEquals(105, hits.get(0).ordinal);
        assertEquals(1, hits.get(0).ordinal / 100);  // The matching member is on the second directory page.
        assertEquals(0, hits.get(1).ordinal);
        assertTrue(index.findNames("A/readme", () -> {}).isEmpty());
        assertEquals("README[1].TXT", index.findNames("README", () -> {}).get(0).item.name);
        assertFalse(index.findNames("SubName", () -> {}).get(0).item.declared);
        assertTrue(index.findNames("Empty", () -> {}).get(0).item.children.isEmpty());
        ArchiveIndex newlySelected = read(packed);  // Even a byte-identical selection has different node identities.
        assertFalse(newlySelected.owns(hits.get(0).item));
        assertTrue(index.owns(hits.get(0).item));
    }

    @Test public void canceledNameSearchReturnsNoPartialSetAndEmptySearchDoesNotListEverything() throws IOException {
        ArchiveIndex index = read(zip("file-a", "file-b", "file-c"));
        CancellationSignal signal = new CancellationSignal();
        int[] checkpoints = {0};
        assertThrows(OperationCanceledException.class, () -> index.findNames("file", () -> {
            if (++checkpoints[0] == 3) signal.cancel();  // First matching row was already collected.
            signal.throwIfCanceled();
        }));
        assertThrows(IllegalArgumentException.class, () -> index.findNames("", () -> {}));
        assertThrows(IllegalArgumentException.class,
                () -> index.findNames("X".repeat(ArchiveIndex.MAX_QUERY + 1), () -> {}));
        assertTrue(index.findNames("no-hit", () -> {}).isEmpty());
    }

    @Test public void theIndexNeverAuthenticatesPayloadOrArchiveComment() throws IOException {
        byte[] b = zip("x");
        int body = 30 + (b[26] & 255) + (b[28] & 255); b[body] ^= 0x10;
        // The unchanged central declaration is displayable. This is NOT a verified snapshot.
        assertEquals(3L, read(b).root.children.get("x").bytes);
    }
    @Test public void pathCollisionsUnsupportedNamesAndFileParentsRefuseWholeIndex() throws IOException {
        for (String[] paths : new String[][] {
                {"Keep", "keep"}, {"a", "a/b"}, {"../part"}, {"a//b"}, {"C:/bad"},
                {"link\\part"}, {"a/aux.txt"}, {"trailing /b"}, {"file\u2028name"},
                {"SS", "ß"}, {"ok", "e\u0301"}}) rejects(zip(paths));
    }
    @Test public void formatCapsAndConflictingCountsRefuseInsteadOfEmpty() throws IOException {
        byte[] b = zip("valid"), changed;
        int end = b.length - 22;
        int cd = ByteBuffer.wrap(b).order(ByteOrder.LITTLE_ENDIAN).getInt(end + 16);
        changed = b.clone(); changed[cd + 10] = 8; rejects(changed);
        changed = b.clone(); changed[cd + 8] |= 1; rejects(changed);
        changed = b.clone(); changed[end + 8] = 2; rejects(changed);
        changed = b.clone(); changed[cd + 28] = (byte)0xff; rejects(changed);
        changed = b.clone(); changed[cd + 46] = (byte)0xff; rejects(changed);
        changed = b.clone(); changed[end + 15] = 1; rejects(changed);
        rejects(Arrays.copyOf(b, b.length - 1));
        assertThrows(IOException.class, () -> ArchiveIndex.read(null, ArchiveIndex.MAX_ARCHIVE + 1));
    }
    private static int recordSize(ByteBuffer data, int off) {
        return 46 + (data.getShort(off + 28) & 65535) + (data.getShort(off + 30) & 65535)
                  + (data.getShort(off + 32) & 65535);
    }
    @Test public void directoryOrderNeedNotMatchPhysicalOrder() throws IOException {
        byte[] b = zip("a", "bb");
        ByteBuffer data = ByteBuffer.wrap(b).order(ByteOrder.LITTLE_ENDIAN);
        int cd = data.getInt(b.length - 6), first = recordSize(data, cd), second = recordSize(data, cd + first);
        byte[] saved = Arrays.copyOfRange(b, cd, cd + first);
        System.arraycopy(b, cd + first, b, cd, second);
        System.arraycopy(saved, 0, b, cd + second, first);
        ArchiveIndex index = read(b);
        assertEquals(2, index.root.children.size());
        for (ArchiveIndex.Item row : index.root.children.values())
            assertTrue(index.checkMember(new StoredMemberTest.Bytes(b), row).matches());
    }
    @Test public void impossibleRequiredPhysicalSpansRejectWithoutOpeningMembers() throws IOException {
        byte[] b = zip("a", "bb");
        ByteBuffer data = ByteBuffer.wrap(b).order(ByteOrder.LITTLE_ENDIAN);
        int cd = data.getInt(b.length - 6), second = cd + recordSize(data, cd);
        data.putInt(second + 42, 1); // different offsets, but the two mandatory headers overlap
        rejects(b);
        data.putInt(second + 42, cd - 30 - 3); // space for header and bytes but not even the name
        rejects(b);
    }
    @Test public void zip64FiveGiBStoredDeclarationUsesOnlyBoundedDirectoryWindows() throws IOException {
        long length = 5L << 30, at = 31 + length;
        ByteBuffer meta = ByteBuffer.allocate(67 + 56 + 20 + 22).order(ByteOrder.LITTLE_ENDIAN);
        meta.putInt(0x02014b50).putShort((short)45).putShort((short)45).position(20);
        meta.putInt(-1).putInt(-1).putShort((short)1).putShort((short)20).position(46);
        meta.put((byte)'x').putShort((short)1).putShort((short)16).putLong(length).putLong(length);
        meta.putInt(0x06064b50).putLong(44).putShort((short)45).putShort((short)45)
            .putInt(0).putInt(0).putLong(1).putLong(1).putLong(67).putLong(at);
        meta.putInt(0x07064b50).putInt(0).putLong(at + 67).putInt(1);
        meta.putInt(0x06054b50).putInt(0).putShort((short)-1).putShort((short)-1)
            .putInt(-1).putInt(-1).putShort((short)0);
        long size = at + meta.position();
        ArchiveIndex.Source sparse = new ArchiveIndex.Source() {
            public byte[] read(long off, int n) {
                assertTrue(n <= 65557 && off >= size - 65557 && off + n <= size);
                byte[] result = new byte[n]; long from = Math.max(at, off);
                if (off + n > from) System.arraycopy(meta.array(), (int)(from - at),
                        result, (int)(from - off), (int)(off + n - from));
                return result;
            }
            public void check() {}
        };
        assertEquals(length, ArchiveIndex.read(sparse, size).root.children.get("x").bytes);
        meta.putLong(67 + 32, 2L);
        assertThrows(IOException.class, () -> ArchiveIndex.read(sparse, size));
    }
    @Test public void onlyTemporarySingleCanonicalPrimaryDocumentResultIsAccepted() throws IOException {
        Uri ok = android.provider.DocumentsContract.buildDocumentUri(PickedDocument.AUTH, "primary:one.zip");
        assertEquals(ok, PickedDocument.single(new Intent().setData(ok).setFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)));
        for (Uri u : new Uri[]{Uri.parse("file:///x.zip"), ok.buildUpon().appendQueryParameter("x", "y").build(),
                android.provider.DocumentsContract.buildTreeDocumentUri(PickedDocument.AUTH, "primary:one.zip"),
                android.provider.DocumentsContract.buildDocumentUri("example.invalid", "primary:one.zip"),
                android.provider.DocumentsContract.buildDocumentUri(PickedDocument.AUTH, "usb:one.zip")}) {
            assertThrows(IOException.class, () -> PickedDocument.single(
                    new Intent().setData(u).setFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)));
        }
        assertThrows(IOException.class, () -> PickedDocument.single(new Intent().setData(ok)));
        assertNull(PickedDocument.picker().getData());
        assertTrue(PickedDocument.picker().getBooleanExtra(Intent.EXTRA_LOCAL_ONLY, false));
    }
}
