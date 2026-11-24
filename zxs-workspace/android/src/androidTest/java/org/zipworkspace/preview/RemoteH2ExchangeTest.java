package org.zipworkspace.preview;

import org.apache.hc.core5.http.message.BasicHttpResponse;
import org.junit.Test;

import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.math.BigInteger;
import java.util.HashMap;

import javax.net.ssl.SSLHandshakeException;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertThrows;

public final class RemoteH2ExchangeTest {
    private static final String EMPTY_SHA =
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    @Test
    public void onlyOneExplicitHttpsOriginIsAccepted() {
        var original = RemoteH2Exchange.Origin.parse("https://Example.org");
        assertEquals(new RemoteH2Exchange.Origin("example.org", 443), original);
        assertEquals(original.identity(), RemoteH2Exchange.Origin.parse("https://example.org:443/").identity());
        assertNotEquals(original.identity(), RemoteH2Exchange.Origin.parse("https://example.org:1443").identity());
        assertEquals(new RemoteH2Exchange.Origin("2001:db8::1", 9443),
                RemoteH2Exchange.Origin.parse("https://[2001:db8::1]:9443"));
        for (String bad : new String[] {"http://example.org", "https://user@example.org",
                "https://example.org/path", "https://example.org/?", "https://example.org#",
                "https://example.org:", "https://example.org:0", "https://example.org:65536",
                "https://example.org%2f", "https://example.org\\bad", "https://example.org\t"}) {
            assertThrows(bad, IllegalArgumentException.class, () -> RemoteH2Exchange.Origin.parse(bad));
        }
    }

    @Test
    public void aMissingOrDifferentAlpnIsNotH2() throws Exception {
        RemoteH2Exchange.requireH2("h2");
        for (String bad : new String[] {null, "", "H2", "http/1.1", "h3"}) {
            assertThrows(SSLHandshakeException.class, () -> RemoteH2Exchange.requireH2(bad));
        }
    }

    @Test
    public void postCannotSmuggleHeadersOrChangeBodyFraming() throws Exception {
        var save = new RemoteSaveContract.Save("11".repeat(16), "22".repeat(16), "33".repeat(16),
                BigInteger.ZERO, new RemoteSaveContract.FirstMain("a"), 0, EMPTY_SHA);
        var headers = new HashMap<>(save.submitHeaders());
        assertEquals(0, RemoteH2Exchange.validatePost(headers));
        headers.put("Content-Length", "4294967296");
        assertEquals(4_294_967_296L, RemoteH2Exchange.validatePost(headers));
        headers.put("Content-Length", "9223372036854775808");
        assertThrows(IOException.class, () -> RemoteH2Exchange.validatePost(headers));
        headers.put("Content-Length", "00");
        assertThrows(IOException.class, () -> RemoteH2Exchange.validatePost(headers));
        headers.put("Content-Length", "0");
        headers.put("Authorization", "other");
        assertThrows(IOException.class, () -> RemoteH2Exchange.validatePost(headers));
        headers.remove("Authorization");
        headers.put("Zxs-Fork-From-Main", "44".repeat(16));
        assertThrows(IOException.class, () -> RemoteH2Exchange.validatePost(headers));
    }

    @Test
    public void earlySuccessHeadersCannotBeUploadProof() throws Exception {
        var producer = new FullBodyProducer(new ByteArrayInputStream(new byte[] { 1 }), 1, EMPTY_SHA);
        var early = RemoteH2Exchange.responseConsumer(16, producer);
        assertThrows(IOException.class, () -> early.consumeResponse(new BasicHttpResponse(200), null, null, null));
        early.consumeResponse(new BasicHttpResponse(409), null, null, null);
        RemoteH2Exchange.responseConsumer(16, null)
                .consumeResponse(new BasicHttpResponse(200), null, null, null);
        var empty = new FullBodyProducer(new ByteArrayInputStream(new byte[0]), 0, EMPTY_SHA);
        RemoteH2Exchange.responseConsumer(16, empty)
                .consumeResponse(new BasicHttpResponse(200), null, null, null);
    }

    @Test
    public void ambiguousOrNonProtocolResponseCannotBeProof() throws Exception {
        var response = new BasicHttpResponse(200);
        response.addHeader("Content-Type", "application/json");
        response.addHeader("Content-Length", "42");
        assertEquals(42, RemoteH2Exchange.responseLength(response));
        response.addHeader("Content-Length", "42");
        assertThrows(IOException.class, () -> RemoteH2Exchange.responseLength(response));

        var redirected = new BasicHttpResponse(302);
        redirected.addHeader("Content-Type", "application/json");
        redirected.addHeader("Content-Length", "42");
        assertThrows(IOException.class, () -> RemoteH2Exchange.responseLength(redirected));

        var oversized = new BasicHttpResponse(404);
        oversized.addHeader("Content-Type", "application/json");
        oversized.addHeader("Content-Length", "16385");
        assertThrows(IOException.class, () -> RemoteH2Exchange.responseLength(oversized));

        var encoded = new BasicHttpResponse(404);
        encoded.addHeader("Content-Type", "application/json");
        encoded.addHeader("Content-Length", "10");
        encoded.addHeader("Content-Encoding", "gzip");
        assertThrows(IOException.class, () -> RemoteH2Exchange.responseLength(encoded));
    }
}
