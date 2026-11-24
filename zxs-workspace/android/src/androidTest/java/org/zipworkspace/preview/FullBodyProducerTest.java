package org.zipworkspace.preview;

import org.apache.hc.core5.http.Header;
import org.apache.hc.core5.http.nio.DataStreamChannel;
import org.junit.Test;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.nio.ByteBuffer;
import java.security.MessageDigest;
import java.util.HexFormat;
import java.util.List;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

public final class FullBodyProducerTest {
    @Test public void streamsAboveTheTextLimitThroughBackpressureOnlyOnce() throws Exception {
        byte[] bytes = new byte[400_001];
        for (int n = 0; n < bytes.length; n++) bytes[n] = (byte) n;
        var producer = new FullBodyProducer(new ByteArrayInputStream(bytes), bytes.length, sha(bytes));
        var output = new Sink();
        assertEquals(bytes.length, producer.getContentLength());
        assertFalse(producer.isRepeatable());
        for (int n = 0; n < 1_000 && !producer.bodyComplete(); n++) {
            assertTrue(producer.available() > 0 && producer.available() <= 65_536);
            producer.produce(output);
        }
        assertTrue(producer.bodyComplete());
        assertArrayEquals(bytes, output.received.toByteArray());
        assertTrue(output.ended);
        producer.releaseResources();
        assertTrue(producer.bodyComplete());
        assertThrows(IOException.class, () -> producer.produce(output));
    }

    @Test public void wrongSizeOrDigestNeverEndsRequest() throws Exception {
        byte[] body = { 1, 2, 3 };
        assertRejected(body, 4, sha(body));
        assertRejected(body, 2, sha(new byte[] { 1, 2 }));
        assertRejected(body, 3, sha(new byte[] { 1, 2, 4 }));
        assertThrows(IOException.class,
                () -> new FullBodyProducer(new ByteArrayInputStream(body), 0, sha(new byte[0])));
    }

    @Test public void emptyBodyHasProofEvenWhenEngineNeedsNoBodyCallback() throws Exception {
        var producer = new FullBodyProducer(new ByteArrayInputStream(new byte[0]), 0, sha(new byte[0]));
        assertTrue(producer.bodyComplete());
        producer.failed(new IOException("transport failed"));
        assertFalse(producer.bodyComplete());
    }

    private static void assertRejected(byte[] bytes, long size, String hash) throws Exception {
        var producer = new FullBodyProducer(new ByteArrayInputStream(bytes), size, hash);
        var output = new Sink();
        assertThrows(IOException.class, () -> {
            for (int calls = 0; calls < 10; calls++) producer.produce(output);
        });
        assertFalse(producer.bodyComplete());
        assertFalse(output.ended);
        assertTrue(output.received.size() < size);
    }

    private static String sha(byte[] bytes) throws Exception {
        return HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(bytes));
    }

    private static final class Sink implements DataStreamChannel {
        final ByteArrayOutputStream received = new ByteArrayOutputStream();
        boolean ended;
        int calls;

        @Override public void requestOutput() {}

        @Override public int write(ByteBuffer source) {
            if (calls++ % 3 == 0) return 0;
            int count = Math.min(2_048, source.remaining());
            for (int n = 0; n < count; n++) received.write(source.get());
            return count;
        }

        @Override public void endStream(List<? extends Header> trailers) {
            assertTrue(trailers.isEmpty());
            ended = true;
        }

        @Override public void endStream() { endStream(List.of()); }
    }
}
