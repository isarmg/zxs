package org.zipworkspace.preview;

import java.io.IOException;
import java.io.InputStream;
import java.nio.ByteBuffer;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.HexFormat;
import java.util.List;
import java.util.Set;

import org.apache.hc.core5.http.nio.AsyncEntityProducer;
import org.apache.hc.core5.http.nio.DataStreamChannel;

/** One non-replayable HTTP/2 request body; the caller owns and closes the verified source. */
final class FullBodyProducer implements AsyncEntityProducer {
    private static final int CHUNK = 64 * 1024;

    private final InputStream source;
    private final long length;
    private final byte[] expectedDigest;
    private final MessageDigest digest;
    private final ByteBuffer pending = ByteBuffer.allocate(CHUNK);
    private long unread;
    private volatile boolean sourceVerified;
    private volatile boolean completed;
    private boolean released;
    private volatile boolean failed;
    private volatile long handedOff;

    FullBodyProducer(InputStream source, long length, String sha256) throws IOException {
        if (source == null || length < 0 || sha256 == null || !sha256.matches("[0-9a-f]{64}")) {
            throw new IOException("Invalid complete request body");
        }
        this.source = source;
        this.length = length;
        this.unread = length;
        this.expectedDigest = HexFormat.of().parseHex(sha256);
        try { digest = MessageDigest.getInstance("SHA-256"); }
        catch (NoSuchAlgorithmException unavailable) { throw new IOException(unavailable); }
        pending.limit(0);
        // HttpCore can omit body callbacks for a declared empty entity.
        if (length == 0) verifyEnd();
    }

    @Override public long getContentLength() { return length; }
    @Override public String getContentType() { return "application/octet-stream"; }
    @Override public String getContentEncoding() { return null; }
    @Override public boolean isChunked() { return false; }
    @Override public Set<String> getTrailerNames() { return Set.of(); }
    @Override public boolean isRepeatable() { return false; }

    @Override public synchronized int available() {
        if (released || failed || completed) return 0;
        return pending.hasRemaining() ? pending.remaining() : (int) Math.max(1, Math.min(CHUNK, unread));
    }

    @Override public synchronized void produce(DataStreamChannel channel) throws IOException {
        if (released || failed || completed) throw new IOException("Request body cannot be replayed");
        try {
            if (!pending.hasRemaining() && unread != 0) readNext();
            if (pending.hasRemaining()) {
                int before = pending.remaining();
                int written = channel.write(pending);
                if (written < 0 || written != before - pending.remaining()) {
                    throw new IOException("HTTP/2 channel did not consume the written bytes");
                }
                handedOff += written;
                if (pending.hasRemaining() || unread != 0) return;
            }
            if (!sourceVerified) verifyEnd();
            channel.endStream(List.of());
            completed = true;
        } catch (IOException failure) {
            failed = true;
            throw failure;
        }
    }

    private void readNext() throws IOException {
        int count = source.read(pending.array(), 0, (int) Math.min(CHUNK, unread));
        if (count <= 0 || count > Math.min(CHUNK, unread)) {
            throw new IOException("Complete request body ended early");
        }
        digest.update(pending.array(), 0, count);
        unread -= count;
        if (unread == 0) verifyEnd(); // Never offer the last chunk if length or digest is wrong.
        pending.position(0).limit(count);
    }

    private void verifyEnd() throws IOException {
        if (source.read() != -1 || !MessageDigest.isEqual(expectedDigest, digest.digest())) {
            throw new IOException("Complete request body does not match its original identity");
        }
        sourceVerified = true;
    }

    long handedOff() { return handedOff; }

    boolean bodyComplete() {
        return !failed && sourceVerified && (completed || length == 0);
    }

    @Override public synchronized void failed(Exception cause) { failed = true; }

    @Override public synchronized void releaseResources() { released = true; }
}
