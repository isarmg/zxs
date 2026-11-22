package org.zipworkspace.preview;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.zip.ZipEntry;
import java.util.zip.ZipInputStream;
import org.junit.Test;
import static org.junit.Assert.*;

public final class LocalWorkspaceDraftTest {
    private static final class Spool extends ByteArrayOutputStream {
        boolean closed;
        @Override public void close() { closed = true; }
    }

    private record Bytes(byte[] body) implements ArchiveIndex.MemberSource {
        @Override public void check() {}
        @Override public byte[] read(long at, int length) throws IOException {
            byte[] result = new byte[length];
            fill(at, result, length);
            return result;
        }
        @Override public void fill(long at, byte[] result, int length) throws IOException {
            if (at < 0 || length < 0 || at > body.length - length) throw new IOException("Test EOF");
            System.arraycopy(body, (int)at, result, 0, length);
        }
    }

    private static byte[] archive(LocalWorkspaceDraft.Frozen candidate) throws IOException {
        Spool spool = new Spool();
        candidate.writeTo(spool);
        assertFalse(spool.closed);
        return spool.toByteArray();
    }

    @Test public void manuallyFreezesTheExactEditedTreeInTheOrdinaryStoreFormat() throws IOException {
        LocalWorkspaceDraft draft = new LocalWorkspaceDraft();
        draft.putText("a.txt", "before");
        draft.addFolder("资料");
        draft.putText("资料/笔记.txt", "你好\r\n");
        draft.addFolder("空");
        draft.putText("𠀀.txt", "尾");
        draft.putText("ａ.txt", "");
        draft.putText("a.txt", "abc\n");
        LocalWorkspaceDraft.Frozen candidate = draft.freeze();
        assertThrows(IllegalStateException.class, () -> draft.putText("a.txt", "late"));
        assertThrows(IllegalStateException.class, draft::freeze);

        byte[] raw = archive(candidate);
        assertThrows(IllegalStateException.class, () -> candidate.writeTo(new Spool()));
        Bytes source = new Bytes(raw);
        ArchiveIndex index = ArchiveIndex.read(source, raw.length);
        ArchiveIndex.SnapshotMarker mark = index.verifySnapshot(source);
        // Derived independently from desktop model.py's JSON/scalar order and domain (not either Java digest).
        assertEquals("c0f5a2ba5ae98f2fdc481b6993a8deabb81d62135ea6e24802487390bfe7880c", mark.tree);
        assertEquals(1L, mark.generation);
        assertNull(mark.parent);
        assertEquals(candidate.repo, mark.repo);
        assertEquals(candidate.branch, mark.branch);
        assertEquals(candidate.operation, mark.operation);
        assertEquals(mark.repo + "." + mark.branch + ".0000000000000001.zxs", candidate.filename());
        assertEquals(6, index.entryCount());
        assertEquals(15L, index.bodyBytes());

        List<String> order = new ArrayList<>();
        try (ZipInputStream zip = new ZipInputStream(new ByteArrayInputStream(raw))) {
            for (ZipEntry member; (member = zip.getNextEntry()) != null;) {
                order.add(member.getName());
                assertEquals(ZipEntry.STORED, member.getMethod());
                byte[] read = zip.readAllBytes();
                if (member.getName().equals("a.txt")) assertArrayEquals("abc\n".getBytes(StandardCharsets.UTF_8), read);
                if (member.getName().equals("资料/笔记.txt"))
                    assertArrayEquals("你好\r\n".getBytes(StandardCharsets.UTF_8), read);
                if (member.isDirectory()) assertEquals(0, read.length);
            }
        }
        assertEquals(Arrays.asList("a.txt", "空/", "资料/", "资料/笔记.txt", "ａ.txt", "𠀀.txt"), order);
    }

    @Test public void rejectsAliasesAndUndisplayableEditsWithoutChangingTheDraft() throws IOException {
        LocalWorkspaceDraft draft = new LocalWorkspaceDraft();
        draft.putText("x.txt", "wanted");
        assertThrows(IllegalArgumentException.class, () -> draft.putText("X.TXT", "alias"));
        assertThrows(IllegalArgumentException.class, () -> draft.addFolder("x.txt"));
        assertThrows(IllegalArgumentException.class, () -> draft.putText("unselected/x.txt", "no parent"));
        assertThrows(IllegalArgumentException.class, () -> draft.addFolder("x.txt/also"));
        assertThrows(IllegalArgumentException.class, () -> draft.addFolder("AUX.jpg"));
        assertThrows(IllegalArgumentException.class, () -> draft.addFolder("x\uD800"));
        assertThrows(IllegalArgumentException.class, () -> draft.putText("x.txt", "isolated\r"));
        assertThrows(IllegalArgumentException.class, () -> draft.putText("x.txt", "中".repeat(PlainText.LIMIT)));
        byte[] raw = archive(draft.freeze());
        Bytes source = new Bytes(raw);
        ArchiveIndex index = ArchiveIndex.read(source, raw.length);
        index.verifySnapshot(source);
        assertEquals(1, index.entryCount());
        assertEquals("wanted", index.checkMember(source, index.root.children.get("x.txt"), true).text.page(0));
    }

    @Test public void emptyTreeUsesTheSameFormatAndFailedStreamIsNotRetried() throws IOException {
        LocalWorkspaceDraft.Frozen empty = new LocalWorkspaceDraft().freeze();
        byte[] raw = archive(empty);
        Bytes source = new Bytes(raw);
        ArchiveIndex index = ArchiveIndex.read(source, raw.length);
        assertEquals(0, index.entryCount());
        assertEquals("ead0d74a9b1110a6e8aa7b227316376cc0883c3fb6b8404f31b0b2719d015657",
                index.verifySnapshot(source).tree);

        LocalWorkspaceDraft draft = new LocalWorkspaceDraft();
        draft.putText("one.txt", "x");
        LocalWorkspaceDraft.Frozen once = draft.freeze();
        assertThrows(IOException.class, () -> once.writeTo(new OutputStream() {
            @Override public void write(int b) throws IOException { throw new IOException("Test spool failure"); }
        }));
        assertThrows(IllegalStateException.class, () -> once.writeTo(new Spool()));
    }
}
