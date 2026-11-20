package org.zipworkspace.preview;

import java.io.IOException;
import java.nio.ByteBuffer;
import java.nio.CharBuffer;
import java.nio.charset.CharsetDecoder;
import java.security.DigestException;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.Arrays;
import static org.zipworkspace.preview.ArchiveIndex.require;

/** In-memory witnesses of one full Store body read. No content, URI or filesystem paths in the index. */
final class IndexedText {
    static final int BLOCK = 1 << 20;
    private static final int HASH = 32;
    private final long bytes;
    private final byte[] hashes, rewind;
    private final int[] first;
    private IndexedText(long bytes, byte[] hashes, byte[] rewind, int[] first) {
        this.bytes = bytes; this.hashes = hashes; this.rewind = rewind; this.first = first;
    }

    int pages() { return first[first.length - 1]; }
    private static long start(int block) { return (long)block * BLOCK; }

    private static MessageDigest sha256() {
        try { return MessageDigest.getInstance("SHA-256"); }
        catch (NoSuchAlgorithmException invalidPlatform) { throw new IllegalStateException("SHA-256 unavailable", invalidPlatform); }
    }

    /** Strictly decodes a complete bounded slice, reusing the same buffers during the whole initial scan. */
    private static final class Decode {
        final byte[] raw = new byte[BLOCK + 3];
        final ByteBuffer input = ByteBuffer.wrap(raw);
        final CharBuffer text = CharBuffer.allocate(raw.length); // UTF-8 needs no more UTF-16 units than input bytes
        final CharsetDecoder codec = PlainText.decoder();

        boolean read(int len) {
            input.clear().limit(len); text.clear(); codec.reset();
            if (!codec.decode(input, text, true).isUnderflow() || input.hasRemaining()
                    || !codec.flush(text).isUnderflow()) return false;
            text.flip();
            return true;
        }
    }

    static final class Build {
        private final IndexedText proof;
        private final Decode decoded = new Decode();
        private final MessageDigest digest = sha256();
        private int blocks;
        private boolean valid = true;

        Build(long size) throws IOException {
            require(size > PlainText.LIMIT && size <= ArchiveIndex.MAX_MEMBER);
            int count = (int)((size + BLOCK - 1) / BLOCK);
            proof = new IndexedText(size, new byte[count * HASH], new byte[count + 1], new int[count + 1]);
        }

        void accept(long relative, byte[] block, int len, Runnable check) throws IOException {
            if (!valid) return; // finish the independent CRC even when text is rejected
            int i = blocks++;
            require(relative == start(i) && i < proof.first.length - 1 && len == Math.min(BLOCK, proof.bytes - relative));
            digest.update(block, 0, len);
            try { require(digest.digest(proof.hashes, i * HASH, HASH) == HASH); }
            catch (DigestException failed) { throw new IOException("Cannot index this body", failed); }

            // Adjacent nominal blocks remain fixed for hashing. Logical text sections instead
            // move a crossing scalar or CRLF into the next section, so no page splits it.
            int carried = proof.rewind[i], combined = carried + len;
            System.arraycopy(block, 0, decoded.raw, carried, len);
            int next = relative + len == proof.bytes ? 0 : PlainText.trailingUnit(decoded.raw, combined);
            int complete = combined - next;
            require(complete > 0 && next <= 3);
            if (!decoded.read(complete)) { valid = false; return; }
            int pages = PlainText.scanPages(decoded.text, check, null);
            if (pages <= 0) { valid = false; return; }
            proof.first[i + 1] = Math.addExact(proof.first[i], pages);
            proof.rewind[i + 1] = (byte)next;
            System.arraycopy(decoded.raw, complete, decoded.raw, 0, next);
        }

        IndexedText finish() throws IOException {
            if (!valid) return null;
            require(blocks == proof.first.length - 1);
            return proof;
        }
    }

    private void block(ArchiveIndex.MemberSource src, long bodyAt, int i, byte[] raw, int len,
                       MessageDigest digest) throws IOException {
        src.fill(bodyAt + start(i), raw, len);
        digest.update(raw, 0, len);
        byte[] actual = digest.digest();
        require(Arrays.equals(actual, 0, actual.length, hashes, i * HASH, (i + 1) * HASH));
    }

    String page(ArchiveIndex.MemberSource src, long bodyAt, int page) throws IOException {
        require(page >= 0 && page < pages());
        int found = Arrays.binarySearch(first, page), i = found >= 0 ? found : -found - 2;
        int head = rewind[i], tail = rewind[i + 1], len = (int)Math.min(BLOCK, bytes - start(i));
        Decode decoded = new Decode();
        MessageDigest digest = sha256();
        byte[] prefix = new byte[head];
        if (head != 0) {
            block(src, bodyAt, i - 1, decoded.raw, BLOCK, digest);
            System.arraycopy(decoded.raw, BLOCK - head, prefix, 0, head);
        }
        // Keep at most three leading bytes across this exact read/digest, then move the
        // just-verified bytes within the same buffer. There is no unchecked reread.
        block(src, bodyAt, i, decoded.raw, len, digest);
        System.arraycopy(decoded.raw, 0, decoded.raw, head, len - tail);
        System.arraycopy(prefix, 0, decoded.raw, 0, head);
        require(decoded.read(head + len - tail));
        int wanted = page - first[i];
        int[] progress = {0, 0}; // previous cut and ordinal
        String[] text = {null};
        int pages = PlainText.scanPages(decoded.text, src::check, end -> {
            if (progress[1]++ == wanted) text[0] = PlainText.literalPage(decoded.text, progress[0], end);
            progress[0] = end;
        });
        require(pages == first[i + 1] - first[i] && text[0] != null);
        return text[0];
    }
}
