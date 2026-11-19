package org.zipworkspace.preview;

import android.content.Intent;
import android.net.Uri;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;
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
    @Test public void actualStoredMembersEmptyDirectoryAndImplicitParents() throws IOException {
        ArchiveIndex.Item root = read(zip("设计/区域/part.txt", "空/", "设计/")).root;
        assertEquals(2, root.children.size());
        assertEquals(3L, root.children.get("设计").children.get("区域").children.get("part.txt").bytes);
        assertTrue(root.children.get("空").children.isEmpty());
        assertTrue(read(zip()).root.children.isEmpty());
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
