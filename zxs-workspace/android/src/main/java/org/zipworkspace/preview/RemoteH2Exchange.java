package org.zipworkspace.preview;

import android.util.Base64;

import org.apache.hc.core5.concurrent.FutureCallback;
import org.apache.hc.core5.http.ContentType;
import org.apache.hc.core5.http.Header;
import org.apache.hc.core5.http.HttpException;
import org.apache.hc.core5.http.HttpHost;
import org.apache.hc.core5.http.HttpRequestInterceptor;
import org.apache.hc.core5.http.HttpVersion;
import org.apache.hc.core5.http.HttpResponse;
import org.apache.hc.core5.http.message.BasicHttpRequest;
import org.apache.hc.core5.http.nio.entity.AbstractBinAsyncEntityConsumer;
import org.apache.hc.core5.http.nio.entity.BasicAsyncEntityProducer;
import org.apache.hc.core5.http.nio.ssl.TlsStrategy;
import org.apache.hc.core5.http.nio.support.BasicRequestProducer;
import org.apache.hc.core5.http.nio.support.BasicResponseConsumer;
import org.apache.hc.core5.http.protocol.HttpCoreContext;
import org.apache.hc.core5.http.ssl.TlsCiphers;
import org.apache.hc.core5.http2.HttpVersionPolicy;
import org.apache.hc.core5.http2.config.H2Config;
import org.apache.hc.core5.http2.impl.H2Processors;
import org.apache.hc.core5.http2.impl.nio.bootstrap.H2RequesterBootstrap;
import org.apache.hc.core5.net.NamedEndpoint;
import org.apache.hc.core5.reactor.IOReactorConfig;
import org.apache.hc.core5.reactor.ssl.SSLBufferMode;
import org.apache.hc.core5.reactor.ssl.TlsDetails;
import org.apache.hc.core5.reactor.ssl.TransportSecurityLayer;
import org.apache.hc.core5.util.Timeout;

import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.net.SocketAddress;
import java.net.URI;
import java.nio.ByteBuffer;
import java.nio.CharBuffer;
import java.nio.charset.StandardCharsets;
import java.security.GeneralSecurityException;
import java.security.MessageDigest;
import java.util.Arrays;
import java.util.HexFormat;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.CancellationException;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;
import java.util.regex.Pattern;

import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLHandshakeException;
import javax.net.ssl.SSLSession;

/** Direct HTTPS/HTTP2 R3 exchange. One origin, one connection and one request. */
final class RemoteH2Exchange implements RemoteSaveSubmit.AuthenticatedExchange, RemoteContainerRead.Reader, AutoCloseable {
    private static final Timeout TIMEOUT = Timeout.ofSeconds(20);
    private static final int REPLY_LIMIT = 16 * 1024;
    private static final String ID = "[0-9a-f]{32}";
    private static final Pattern WIRE_ID = Pattern.compile(ID);
    private static final Pattern LOOKUP = Pattern.compile("/zxs/v1/containers/" + ID + "/operations/" + ID);
    private static final Pattern POST = Pattern.compile("/zxs/v1/containers/" + ID + "/branches/(main|" + ID + ")/nodes");
    private static final String CATALOG_PATH = "/zxs/v1/containers";
    private static final Pattern DETAIL = Pattern.compile(CATALOG_PATH + "/" + ID);
    private static final Pattern BRANCH_HISTORY =
            Pattern.compile(CATALOG_PATH + "/" + ID + "/branches/(main|" + ID + ")/nodes");
    private static final Pattern SEQUENCE = Pattern.compile("[1-9][0-9]{0,19}");
    private static final Pattern LOGIN = Pattern.compile("[a-z0-9][a-z0-9._-]{0,63}");
    private static final Pattern DNS = Pattern.compile("[a-z0-9.-]{1,253}");
    private static final Pattern NUM = Pattern.compile("0|[1-9][0-9]{0,8}");
    private static final Pattern HEX_NAME = Pattern.compile("(?:[0-9a-f]{2}){1,255}");
    private static final Pattern SHA = Pattern.compile("[0-9a-f]{64}");
    private static final Set<String> BODY_FIELDS = Set.of(
            "Content-Type", "Content-Length", "Zxs-Operation", "Zxs-Node", "Zxs-Sha256");

    record Origin(String host, int port) {
        static Origin parse(String selected) {
            if (selected == null || selected.length() > 1024
                    || selected.chars().anyMatch(c -> c <= 32 || c >= 127 || "%?#\\".indexOf(c) >= 0)) {
                throw new IllegalArgumentException("invalid HTTPS origin");
            }
            URI origin = URI.create(selected);
            String host = origin.getHost();
            if (!"https".equals(origin.getScheme()) || host == null || origin.getRawUserInfo() != null
                    || origin.getRawAuthority().endsWith(":") || origin.getPort() > 65535 || origin.getPort() == 0
                    || !("".equals(origin.getPath()) || "/".equals(origin.getPath()))) {
                throw new IllegalArgumentException("use one HTTPS host and optional port");
            }
            host = host.toLowerCase(Locale.ROOT);
            if (host.startsWith("[") && host.endsWith("]")) {
                host = host.substring(1, host.length() - 1);
            } else if (!DNS.matcher(host).matches()) {
                throw new IllegalArgumentException("invalid HTTPS host");
            }
            return new Origin(host, origin.getPort() == -1 ? 443 : origin.getPort());
        }

        String identity() {
            String authority = host.indexOf(':') >= 0 ? "[" + host + "]" : host;
            return HexFormat.of().formatHex(sha256(("android-r3-system-ca-v1\0https://" + authority + ":" + port)
                    .getBytes(StandardCharsets.US_ASCII)));
        }
    }

    private final Origin origin;
    private final RemoteSaveJournal.Authority authority;
    private final byte[] basic;
    private boolean closed;

    RemoteH2Exchange(String selected, String account, char[] password) throws IOException {
        origin = Origin.parse(selected);
        if (account == null || !LOGIN.matcher(account).matches() || password == null
                || password.length == 0 || password.length > 1024) {
            throw new IllegalArgumentException("invalid native credentials");
        }
        authority = new RemoteSaveJournal.Authority(origin.identity(), account);
        byte[] prefix = (account + ":").getBytes(StandardCharsets.US_ASCII);
        ByteBuffer encoded = StandardCharsets.UTF_8.newEncoder().encode(CharBuffer.wrap(password));
        byte[] raw = new byte[prefix.length + encoded.remaining()];
        try {
            require(encoded.remaining() > 0 && encoded.remaining() <= 1024);
            System.arraycopy(prefix, 0, raw, 0, prefix.length);
            encoded.get(raw, prefix.length, encoded.remaining());
            for (int i = prefix.length; i < raw.length; i++) {
                int b = Byte.toUnsignedInt(raw[i]);
                require(b >= 32 && b != 127);
            }
            basic = Base64.encode(raw, Base64.NO_WRAP);
        } finally {
            Arrays.fill(raw, (byte) 0);
            if (encoded.hasArray()) Arrays.fill(encoded.array(), (byte) 0);
        }
    }

    @Override public RemoteSaveJournal.Authority authority() { return authority; }

    @Override
    public RemoteSaveSubmit.Response lookup(String exactOriginPath) throws IOException {
        require(exactOriginPath != null && LOOKUP.matcher(exactOriginPath).matches());
        return exchange("GET", exactOriginPath, Map.of(), null, CONTROL);
    }

    @Override
    public RemoteSaveSubmit.Response post(String exactOriginPath, Map<String, String> headers, InputStream completeBody)
            throws IOException {
        require(exactOriginPath != null && POST.matcher(exactOriginPath).matches());
        int length = validatePost(headers);
        require(completeBody != null);
        byte[] payload = new byte[length];
        try {
            require(completeBody.readNBytes(payload, 0, length) == length && completeBody.read() == -1);
            require(headers.get("Zxs-Sha256").equals(HexFormat.of().formatHex(sha256(payload))));
            return exchange("POST", exactOriginPath, headers, payload, CONTROL);
        } finally {
            Arrays.fill(payload, (byte) 0);
        }
    }

    private record ReplyShape(String media, int limit, boolean control, int bodyLength, String digest) {}

    private static final ReplyShape CONTROL = new ReplyShape("application/json", REPLY_LIMIT, true, -1, null);
    private static final ReplyShape METADATA =
            new ReplyShape("application/json", RemoteContainerRead.METADATA_LIMIT, false, -1, null);

    @Override
    public byte[] metadata(String path, Map<String, String> headers) throws IOException {
        require(path != null && headers != null);
        if (CATALOG_PATH.equals(path)) {
            require("compact".equals(headers.get("Zxs-View"))
                    && (headers.size() == 1 || (headers.size() == 2
                    && RemoteContainerRead.id(headers.get("Zxs-After")))));
        } else if (DETAIL.matcher(path).matches()) {
            String after = headers.get("Zxs-Detail-After");
            require("paged".equals(headers.get("Zxs-Detail"))
                    && matches(headers.get("Zxs-Detail-Sequence"), SEQUENCE)
                    && (headers.size() == 2 || (headers.size() == 3
                    && ("main".equals(after) || RemoteContainerRead.id(after)))));
        } else {
            String head = headers.get("Zxs-History-Head");
            String next = headers.get("Zxs-History-Next");
            require(BRANCH_HISTORY.matcher(path).matches() && RemoteContainerRead.id(head)
                    && (headers.size() == 1 || (headers.size() == 2
                    && RemoteContainerRead.id(next) && !next.equals(head))));
        }
        return exchange("GET", path, headers, null, METADATA).data();
    }

    @Override
    public byte[] node(String container, String node, int length, String sha256) throws IOException {
        require(RemoteContainerRead.id(container) && RemoteContainerRead.id(node)
                && length >= 0 && length <= PlainText.LIMIT && matches(sha256, SHA));
        byte[] data = exchange("GET", CATALOG_PATH + "/" + container + "/nodes/" + node + "/data",
                Map.of(), null, new ReplyShape("application/octet-stream", length, false, length, sha256)).data();
        require(sha256.equals(HexFormat.of().formatHex(sha256(data))));
        return data;
    }

    static int validatePost(Map<String, String> headers) throws IOException {
        require(headers != null && headers.keySet().containsAll(BODY_FIELDS)
                && "application/octet-stream".equals(headers.get("Content-Type"))
                && matches(headers.get("Content-Length"), NUM)
                && matches(headers.get("Zxs-Operation"), WIRE_ID)
                && matches(headers.get("Zxs-Node"), WIRE_ID)
                && matches(headers.get("Zxs-Sha256"), SHA));
        int length = Integer.parseInt(headers.get("Content-Length"));
        require(length <= PlainText.LIMIT);
        boolean base = headers.containsKey("Zxs-Base-Node");
        boolean initial = headers.containsKey("Zxs-Initial-Name");
        boolean historical = headers.containsKey("Zxs-Fork-From-Main");
        require(base != historical && headers.size() == BODY_FIELDS.size() + (initial ? 2 : 1));
        require((initial && "0".equals(headers.get("Zxs-Base-Node"))
                    && matches(headers.get("Zxs-Initial-Name"), HEX_NAME))
                || (!initial && matches(headers.get(base ? "Zxs-Base-Node" : "Zxs-Fork-From-Main"),
                    WIRE_ID)));
        return length;
    }

    private RemoteSaveSubmit.Response exchange(String method, String path, Map<String, String> headers,
                                               byte[] body, ReplyShape shape) throws IOException {
        final SSLContext tls;
        try {
            tls = SSLContext.getInstance("TLS");
            tls.init(null, null, null); // Normal Android system trust; no custom bypass.
        } catch (GeneralSecurityException unavailable) {
            throw new IOException("secure HTTPS credentials are unavailable");
        }
        var gate = new H2Gate(tls, origin);
        var host = new HttpHost("https", origin.host(), origin.port());
        HttpRequestInterceptor originalPeer = (request, entity, context) -> {
            SSLSession originalSession = gate.verified.getNow(null);
            require(originalSession != null && HttpVersion.HTTP_2.equals(context.getProtocolVersion())
                    && HttpCoreContext.adapt(context).getSSLSession() == originalSession);
            request.setHeader("Authorization", authorizationAfterProof());
        };
        try (var client = H2RequesterBootstrap.bootstrap().setVersionPolicy(HttpVersionPolicy.FORCE_HTTP_2)
                .setHttpProcessor(H2Processors.customClient("Workspace").addFirst(originalPeer).build())
                .setH2Config(H2Config.custom().setPushEnabled(false).setMaxConcurrentStreams(1)
                        .setMaxHeaderListSize(REPLY_LIMIT).build())
                .setIOReactorConfig(IOReactorConfig.custom().setIoThreadCount(1).setSoTimeout(TIMEOUT).build())
                .setHandshakeTimeout(TIMEOUT).setMaxTotal(1).setDefaultMaxPerRoute(1).setTlsStrategy(gate).create()) {
            client.start();
            var endpoint = waitFor(client.connect(host, TIMEOUT));
            try {
                // The TCP endpoint future alone is not a verified TLS session.
                waitFor(gate.verified);
                var request = new BasicHttpRequest(method, host, path);
                request.setHeader("Accept", shape.media());
                request.setHeader("Accept-Encoding", "identity");
                for (var header : headers.entrySet()) {
                    if (header.getKey().startsWith("Zxs-") || header.getKey().equals("Content-Length")) {
                        request.setHeader(header.getKey(), header.getValue());
                    }
                }
                var producer = new BasicRequestProducer(request,
                        body == null ? null : new BasicAsyncEntityProducer(body, ContentType.APPLICATION_OCTET_STREAM));
                var bounded = new BasicResponseConsumer<>(new BoundedBody(shape.limit())) {
                    @Override public void streamEnd(List<? extends Header> trailers)
                            throws HttpException, IOException {
                        require(trailers == null || trailers.isEmpty());
                        super.streamEnd(trailers);
                    }
                };
                var response = waitFor(endpoint.execute(producer, bounded, null));
                byte[] data = response.getBody();
                HttpResponse head = response.getHead();
                int length = responseLength(head, shape);
                require(data != null ? data.length == length : length == 0);
                ensureLive();
                return new RemoteSaveSubmit.Response(head.getCode(), data != null ? data : new byte[0]);
            } finally {
                endpoint.releaseAndDiscard();
            }
        } catch (IllegalStateException | UnsupportedOperationException unavailable) {
            throw new IOException("verified HTTP/2 exchange unavailable");
        }
    }

    static int responseLength(HttpResponse head) throws IOException {
        return responseLength(head, CONTROL);
    }

    private static int responseLength(HttpResponse head, ReplyShape shape) throws IOException {
        int status = head.getCode();
        require(status == 200 || (shape.control() && (status == 404 || status == 409)));
        for (String forbidden : Set.of("transfer-encoding", "content-encoding", "content-range", "trailer")) {
            require(!head.containsHeader(forbidden));
        }
        var lengths = head.getHeaders("content-length");
        require(lengths.length == 1 && matches(lengths[0].getValue(), NUM));
        int size = Integer.parseInt(lengths[0].getValue());
        require(size <= shape.limit() && (shape.bodyLength() < 0 || shape.bodyLength() == size));
        if (status != 409) {
            var media = head.getHeaders("content-type");
            require(media.length == 1 && shape.media().equals(media[0].getValue()));
        }
        if (shape.digest() != null) {
            var digest = head.getHeaders("zxs-sha256");
            require(digest.length == 1 && shape.digest().equals(digest[0].getValue()));
        }
        return size;
    }

    private synchronized String authorizationAfterProof() throws IOException {
        ensureLive();
        return "Basic " + new String(basic, StandardCharsets.US_ASCII);
    }

    private synchronized void ensureLive() throws IOException {
        require(!closed && !Thread.currentThread().isInterrupted());
    }

    @Override public synchronized void close() {
        closed = true;
        Arrays.fill(basic, (byte) 0);
    }

    private static final class BoundedBody extends AbstractBinAsyncEntityConsumer<byte[]> {
        private final ByteArrayOutputStream buffer = new ByteArrayOutputStream();
        private final int limit;

        BoundedBody(int limit) { this.limit = limit; }

        @Override protected void streamStart(ContentType type) {}
        @Override protected int capacityIncrement() { return Math.max(1, Math.min(16_384, limit)); }

        @Override protected void data(ByteBuffer source, boolean end) throws IOException {
            require(source.remaining() <= limit - buffer.size());
            byte[] next = new byte[source.remaining()];
            source.get(next);
            buffer.write(next);
        }

        @Override protected byte[] generateContent() { return buffer.toByteArray(); }
        @Override public void releaseResources() { buffer.reset(); }
    }

    private static final class H2Gate implements TlsStrategy {
        final CompletableFuture<SSLSession> verified = new CompletableFuture<>();
        private final SSLContext tls;
        private final Origin origin;

        H2Gate(SSLContext tls, Origin origin) {
            this.tls = tls;
            this.origin = origin;
        }

        @Override
        public boolean upgrade(TransportSecurityLayer session, HttpHost host, SocketAddress local,
                               SocketAddress remote, Object attachment, Timeout handshakeTimeout) {
            upgrade(session, host, attachment, handshakeTimeout, null);
            return true;
        }

        @Override
        public void upgrade(TransportSecurityLayer session, NamedEndpoint endpoint, Object attachment,
                            Timeout handshakeTimeout, FutureCallback<TransportSecurityLayer> callback) {
            if (endpoint == null || !origin.host().equalsIgnoreCase(endpoint.getHostName())
                    || origin.port() != endpoint.getPort()) {
                var wrong = new SSLHandshakeException("TLS authority changed");
                verified.completeExceptionally(wrong);
                if (callback != null) callback.failed(wrong);
                return;
            }
            session.startTls(tls, endpoint, SSLBufferMode.STATIC, (remote, engine) -> {
                var parameters = engine.getSSLParameters();
                parameters.setProtocols(new String[] {"TLSv1.3", "TLSv1.2"});
                parameters.setCipherSuites(TlsCiphers.excludeH2Blacklisted(parameters.getCipherSuites()));
                parameters.setEndpointIdentificationAlgorithm("HTTPS");
                parameters.setApplicationProtocols(new String[] {"h2"});
                engine.setSSLParameters(parameters);
            }, (remote, engine) -> {
                try {
                    requireH2(engine.getApplicationProtocol());
                } catch (SSLHandshakeException | UnsupportedOperationException rejected) {
                    verified.completeExceptionally(rejected);
                    throw rejected;
                }
                SSLSession negotiated = engine.getSession();
                if (!verified.complete(negotiated)) throw new SSLHandshakeException("second TLS session was refused");
                return new TlsDetails(negotiated, "h2");
            }, handshakeTimeout, callback);
        }
    }

    static void requireH2(String actual) throws SSLHandshakeException {
        if (!"h2".equals(actual)) throw new SSLHandshakeException("peer did not negotiate HTTP/2");
    }

    private static <T> T waitFor(Future<T> pending) throws IOException {
        try {
            return pending.get(TIMEOUT.toMilliseconds(), TimeUnit.MILLISECONDS);
        } catch (InterruptedException interrupted) {
            pending.cancel(true);
            Thread.currentThread().interrupt();
            throw new IOException("HTTPS exchange interrupted");
        } catch (ExecutionException | TimeoutException | CancellationException notComplete) {
            pending.cancel(true);
            throw new IOException("verified HTTP/2 exchange did not complete");
        }
    }

    private static boolean matches(String value, Pattern pattern) {
        return value != null && pattern.matcher(value).matches();
    }

    private static byte[] sha256(byte[] bytes) {
        try {
            return MessageDigest.getInstance("SHA-256").digest(bytes);
        } catch (GeneralSecurityException impossible) {
            throw new AssertionError(impossible);
        }
    }

    private static void require(boolean expected) throws IOException {
        if (!expected) throw new IOException("unverified HTTP/2 request or reply");
    }
}
