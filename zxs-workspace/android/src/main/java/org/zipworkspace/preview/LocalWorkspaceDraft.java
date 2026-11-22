package org.zipworkspace.preview;

import java.io.FilterOutputStream;
import java.io.IOException;
import java.io.OutputStream;
import java.nio.ByteBuffer;
import java.nio.CharBuffer;
import java.nio.charset.CharacterCodingException;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.time.LocalDateTime;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import java.util.UUID;
import java.util.zip.CRC32;
import java.util.zip.ZipEntry;
import java.util.zip.ZipOutputStream;

/** Fresh local draft. Editing has no disk or imported-document side effects. */
final class LocalWorkspaceDraft {
    static final int MAX_ENTRIES = 64, MAX_BYTES = 1 << 20;
    private static final LocalDateTime ZIP_TIME = LocalDateTime.of(1980, 1, 1, 0, 0);

    private record Entry(String path, byte[] encodedPath, byte[] text) {
        boolean folder() { return text == null; }
        int size() { return folder() ? 0 : text.length; }
    }

    private final String repo = id(), branch = id();
    private final Map<String, Entry> entries = new HashMap<>();
    private final Set<String> spellings = new HashSet<>();
    private int bodyBytes;
    private boolean frozen;

    synchronized void addFolder(String path) {
        put(path, null);
    }

    synchronized void putText(String path, String text) {
        requireEditable();
        if (text.length() > PlainText.LIMIT || PlainText.scanPages(text, () -> {}, null) < 0)
            throw new IllegalArgumentException("Not supported editable text");
        byte[] content = utf8(text);
        if (content.length > PlainText.LIMIT) throw new IllegalArgumentException("Text exceeds the edit limit");
        put(path, content);
    }

    private void put(String path, byte[] content) {
        requireEditable();
        if (path.length() > 1024) throw new IllegalArgumentException("Path exceeds the archive limit");
        boolean folder = content == null;
        byte[] encoded = utf8(path);
        if (encoded.length + (folder ? 1 : 0) > 1024)
            throw new IllegalArgumentException("Path exceeds the archive limit");
        String[] parts = path.split("/", -1);
        if (parts.length > 48) throw new IllegalArgumentException("Path exceeds the archive depth");
        StringBuilder folded = new StringBuilder();
        for (String part : parts) {
            if (!ArchiveIndex.safeComponent(part)) throw new IllegalArgumentException("Not a portable name");
            if (folded.length() != 0) folded.append('/');
            folded.append(ArchiveIndex.foldComponent(part));
        }
        int slash = path.lastIndexOf('/');
        if (slash >= 0) {
            Entry parent = entries.get(path.substring(0, slash));
            if (parent == null || !parent.folder()) throw new IllegalArgumentException("Select an original draft folder");
        }
        String key = folded.toString();
        Entry old = entries.get(path);
        if ((old != null && (folder || old.folder())) || (old == null && spellings.contains(key)))
            throw new IllegalArgumentException("Draft name is already occupied");
        if (old == null && entries.size() == MAX_ENTRIES)
            throw new IllegalArgumentException("Too many draft entries");
        Entry next = new Entry(path, encoded, content);
        int nextBytes = bodyBytes - (old == null ? 0 : old.size()) + next.size();
        if (nextBytes > MAX_BYTES) throw new IllegalArgumentException("Draft exceeds the edit limit");
        bodyBytes = nextBytes;
        entries.put(path, next);
        spellings.add(key);
    }

    private void requireEditable() {
        if (frozen) throw new IllegalStateException("The original draft was already frozen");
    }

    private static String id() { return UUID.randomUUID().toString().replace("-", ""); }

    private static byte[] utf8(String value) {
        try {
            ByteBuffer source = StandardCharsets.UTF_8.newEncoder().encode(CharBuffer.wrap(value));
            byte[] bytes = new byte[source.remaining()];
            source.get(bytes);
            return bytes;
        } catch (CharacterCodingException err) {
            throw new IllegalArgumentException("Not valid UTF-8 text or path", err);
        }
    }

    /** The explicit commit decision captures this whole tree once. It does not publish it. */
    synchronized Frozen freeze() {
        requireEditable();
        ArrayList<Entry> ordered = new ArrayList<>(entries.values());
        ordered.sort((a, b) -> Arrays.compareUnsigned(a.encodedPath, b.encodedPath));
        String operation = id();
        MessageDigest tree = ArchiveIndex.newTreeDigest(), content = ArchiveIndex.sha256();
        for (Entry entry : ordered) {
            ArchiveIndex.addTreeRow(tree, entry.encodedPath, entry.folder(), entry.size(),
                    entry.folder() ? null : content.digest(entry.text));
        }
        Frozen result = new Frozen(repo, branch, operation, ArchiveIndex.hex(tree.digest()), List.copyOf(ordered));
        frozen = true;
        return result;
    }

    static final class Frozen {
        final String repo, branch, operation, tree;
        private final List<Entry> entries;
        private boolean written;

        private Frozen(String repo, String branch, String operation, String tree, List<Entry> entries) {
            this.repo = repo;
            this.branch = branch;
            this.operation = operation;
            this.tree = tree;
            this.entries = entries;
        }

        String filename() { return repo + "." + branch + ".0000000000000001.zxs"; }

        String marker() {
            return "zip-workspace/1\n{\"branch\":\"" + branch + "\",\"generation\":1,\"operation\":\"" + operation
                    + "\",\"parent\":null,\"repo\":\"" + repo + "\",\"tree\":\"" + tree + "\"}";
        }

        /** Write once to the caller's private spool. Caller owns flush/sync, readback and publication. */
        synchronized void writeTo(OutputStream destination) throws IOException {
            Objects.requireNonNull(destination);
            if (written) throw new IllegalStateException("The original candidate was already attempted");
            written = true;
            try (ZipOutputStream zip = new ZipOutputStream(new FilterOutputStream(destination) {
                @Override public void write(byte[] b, int at, int length) throws IOException { out.write(b, at, length); }
                @Override public void close() {} // Do not close the caller's original spool.
            }, StandardCharsets.UTF_8)) {
                zip.setComment(marker());
                CRC32 checksum = new CRC32();
                for (Entry entry : entries) {
                    ZipEntry member = new ZipEntry(entry.path + (entry.folder() ? "/" : ""));
                    member.setTimeLocal(ZIP_TIME);
                    member.setMethod(ZipEntry.STORED);
                    member.setSize(entry.size());
                    member.setCompressedSize(entry.size());
                    checksum.reset();
                    if (!entry.folder()) checksum.update(entry.text);
                    member.setCrc(checksum.getValue());
                    zip.putNextEntry(member);
                    if (!entry.folder()) zip.write(entry.text);
                    zip.closeEntry();
                }
            }
        }
    }
}
